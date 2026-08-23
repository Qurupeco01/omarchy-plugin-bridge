//! `opb doctor` — assembles tier-1 checks (CONCEPT §10) into a report.

mod bins;
mod conflicts;
mod env;
mod version;

use crate::check::Report;
use std::process::Command;

/// Runs `<bin> <args>`, capturing stdout. `None` = not spawned
/// (binary missing from PATH or spawn failed for any reason).
fn probe(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn report() -> Report {
    let qs_out = if which_ok("quickshell") { probe("quickshell", &["--version"]) } else { None };
    let hyprctl = which_ok("hyprctl");
    let mut checks = vec![
        bins::check_quickshell(qs_out.as_deref()),
        bins::check_bin("hyprctl", hyprctl),
        bins::check_bin("git", which_ok("git")),
        bins::check_bin("bash", which_ok("bash")),
        env::check_wayland(std::env::var("WAYLAND_DISPLAY").ok().as_deref()),
        env::check_desktop(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref()),
    ];
    // Enabled component ids come from `~/.config/omarchy/shell.json`, written by
    // bootstrap and edited by select (Phases 2-3). Until then nothing is enabled,
    // so conflict checks are skipped entirely — bootstrap-time doctor is tier 1 only.
    let enabled: Vec<&str> = Vec::new();
    if !enabled.is_empty() {
        match probe("busctl", &["--user", "list", "--no-pager"]) {
            Some(out) => checks.extend(conflicts::scan(
                &conflicts::parse_processes(&out),
                &enabled,
            )),
            None => checks.push(crate::check::CheckResult::warn(
                "conflicts",
                "session bus scan skipped (busctl unavailable or no bus)",
            )),
        }
    }
    Report(checks)
}

/// PATH lookup only — never executes the binary. Checks executability (X_OK),
/// unlike a bare `is_file` check (handled by the `which` crate).
fn which_ok(bin: &str) -> bool {
    which::which(bin).is_ok()
}
