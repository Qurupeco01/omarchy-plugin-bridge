//! Requirement checks: are the binaries and session environment present?
//! Consumed as the preflight gate of `opb bootstrap` and re-run before an
//! update flips the pin. opb never fixes the user's system — it reports what
//! is missing and lets the installer decide.

use crate::check::{CheckResult, Report};
use std::process::Command;

/// Runs `<bin> <args>`, capturing stdout. `None` = not spawned
/// (binary missing from PATH or spawn failed for any reason).
fn probe(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// PATH lookup only — never executes the binary. Checks executability (X_OK),
/// unlike a bare `is_file` check (handled by the `which` crate).
fn which_ok(bin: &str) -> bool {
    which::which(bin).is_ok()
}

pub fn dependency_checks() -> Report {
    let qs_out =
        if which_ok("quickshell") { probe("quickshell", &["--version"]) } else { None };
    Report(vec![
        check_quickshell(qs_out.as_deref()),
        check_bin("hyprctl", which_ok("hyprctl")),
        check_bin("git", which_ok("git")),
        check_bin("bash", which_ok("bash")),
        check_wayland(std::env::var("WAYLAND_DISPLAY").ok().as_deref()),
        check_desktop(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref()),
    ])
}

// --- binary checks (pure: probe output in, CheckResult out) ---

/// Semantic version extracted from tool output strings.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Extracts the first `major.minor[.patch]` run found anywhere in `input`.
/// Tolerates prefixes ("Quickshell 0.3.1 (revision ...)") and suffixes
/// ("5.3.15(1)-release", "0.4.1-42-gdeadbeef").
pub fn version_parse(input: &str) -> Option<Version> {
    let mut run = String::new();
    for c in input.chars() {
        if c.is_ascii_digit() || (c == '.' && !run.is_empty()) {
            run.push(c);
        } else if !run.is_empty() {
            if let Some(v) = try_run(&run) {
                return Some(v);
            }
            run.clear();
        }
    }
    try_run(&run)
}

fn try_run(run: &str) -> Option<Version> {
    let parts: Vec<Option<u32>> = run
        .split('.')
        .map(|p| p.parse().ok())
        .take(3)
        .collect();
    match parts.as_slice() {
        [Some(maj), Some(min)] => Some(Version { major: *maj, minor: *min, patch: 0 }),
        [Some(maj), Some(min), Some(pat)] => {
            Some(Version { major: *maj, minor: *min, patch: *pat })
        }
        _ => None,
    }
}

/// Tested floor per CONCEPT §10 (pacman `quickshell`, not AUR `-git`).
const QUICKSHELL_FLOOR: Version = Version { major: 0, minor: 3, patch: 1 };

fn check_quickshell(probe: Option<&str>) -> CheckResult {
    let Some(out) = probe else {
        return CheckResult::fail("quickshell", "not found in PATH");
    };
    match version_parse(out) {
        None => CheckResult::warn(
            "quickshell",
            format!("unparseable version output: {:?}", out.trim()),
        ),
        Some(v) if v < QUICKSHELL_FLOOR => CheckResult::warn(
            "quickshell",
            format!("{v} below tested floor {QUICKSHELL_FLOOR}"),
        ),
        Some(v) => CheckResult::pass_info("quickshell", &v.to_string()),
    }
}

fn check_bin(name: &'static str, present: bool) -> CheckResult {
    if present {
        CheckResult::pass(name)
    } else {
        CheckResult::fail(name, "not found in PATH")
    }
}

// --- session environment checks (pure: env values in, CheckResult out) ---

/// Wayland session present — quickshell cannot attach without it.
fn check_wayland(display: Option<&str>) -> CheckResult {
    match display {
        Some(d) if !d.is_empty() => CheckResult::pass_info("WAYLAND_DISPLAY", d),
        _ => CheckResult::fail("WAYLAND_DISPLAY", "not set (no Wayland session)"),
    }
}

/// Desktop environment detection — informational, hyprctl presence is the real signal.
fn check_desktop(desktop: Option<&str>) -> CheckResult {
    match desktop {
        Some(d) if d.split(':').any(|c| c == "Hyprland") => {
            CheckResult::pass_info("XDG_CURRENT_DESKTOP", d)
        }
        Some(d) => CheckResult::warn(
            "XDG_CURRENT_DESKTOP",
            format!("session is {d:?}, expected Hyprland"),
        ),
        None => CheckResult::warn("XDG_CURRENT_DESKTOP", "not set"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::Status;

    #[test]
    fn missing_binary_fails() {
        assert_eq!(
            check_quickshell(None),
            CheckResult::fail("quickshell", "not found in PATH")
        );
    }

    #[test]
    fn at_floor_passes() {
        let r = check_quickshell(Some("Quickshell 0.3.1 (revision , distributed by Arch Linux)"));
        assert_eq!(r.status, Status::Pass(Some("0.3.1".into())));
    }

    #[test]
    fn below_floor_warns() {
        let r = check_quickshell(Some("Quickshell 0.2.9"));
        assert_eq!(r.status, Status::Warn("0.2.9 below tested floor 0.3.1".into()));
    }

    #[test]
    fn above_floor_passes() {
        let r = check_quickshell(Some("Quickshell 0.4.1-42-gdeadbeef"));
        assert_eq!(r.status, Status::Pass(Some("0.4.1".into())));
    }

    #[test]
    fn unparseable_output_warns() {
        let r = check_quickshell(Some("???"));
        assert!(matches!(r.status, Status::Warn(_)));
    }

    #[test]
    fn simple_bin_presence() {
        assert_eq!(check_bin("git", true).status, Status::Pass(None));
        assert_eq!(
            check_bin("bash", false),
            CheckResult::fail("bash", "not found in PATH")
        );
    }

    fn v(maj: u32, min: u32, pat: u32) -> Version {
        Version { major: maj, minor: min, patch: pat }
    }

    #[test]
    fn real_tool_outputs() {
        assert_eq!(
            version_parse("Quickshell 0.3.1 (revision , distributed by Arch Linux)"),
            Some(v(0, 3, 1))
        );
        assert_eq!(version_parse("git version 2.55.0"), Some(v(2, 55, 0)));
        assert_eq!(
            version_parse("GNU bash, version 5.3.15(1)-release (x86_64-pc-linux-gnu)"),
            Some(v(5, 3, 15))
        );
        assert_eq!(
            version_parse("Hyprland 0.56.2 built from branch v0.56.2 at commit efb5099 clean"),
            Some(v(0, 56, 2))
        );
    }

    #[test]
    fn two_component_and_commit_suffix() {
        assert_eq!(version_parse("0.4"), Some(v(0, 4, 0)));
        assert_eq!(version_parse("0.4.1-42-gdeadbeef"), Some(v(0, 4, 1)));
    }

    #[test]
    fn garbage_yields_none() {
        assert_eq!(version_parse("no version here"), None);
        assert_eq!(version_parse(""), None);
        assert_eq!(version_parse("..."), None);
    }

    #[test]
    fn ordering_is_semantic() {
        assert!(v(0, 10, 0) > v(0, 3, 99));
        assert!(v(1, 0, 0) > v(0, 99, 99));
    }

    #[test]
    fn wayland_set_passes() {
        assert_eq!(
            check_wayland(Some("wayland-1")).status,
            Status::Pass(Some("wayland-1".into()))
        );
        assert!(matches!(check_wayland(Some("")).status, Status::Fail(_)));
    }

    #[test]
    fn wayland_unset_fails() {
        assert_eq!(
            check_wayland(None),
            CheckResult::fail("WAYLAND_DISPLAY", "not set (no Wayland session)")
        );
    }

    #[test]
    fn desktop_hyprland_passes() {
        assert_eq!(
            check_desktop(Some("Hyprland")).status,
            Status::Pass(Some("Hyprland".into()))
        );
        // e.g. Hyprland:sway set by some setups
        assert!(matches!(
            check_desktop(Some("Hyprland:uwsm")).status,
            Status::Pass(_)
        ));
    }

    #[test]
    fn desktop_other_warns() {
        assert_eq!(
            check_desktop(Some("GNOME")).status,
            Status::Warn("session is \"GNOME\", expected Hyprland".into())
        );
        assert_eq!(check_desktop(None).status, Status::Warn("not set".into()));
    }
}
