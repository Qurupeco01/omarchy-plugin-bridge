//! Dependency and environment checks — assembled into `opb status` and used
//! as the FAIL gate before an update flips the pin. Conflicts between running
//! desktop apps are deliberately out of scope (ADR-0008): report-only would
//! still be analysis opb does not own.

mod bins;
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

/// PATH lookup only — never executes the binary. Checks executability (X_OK),
/// unlike a bare `is_file` check (handled by the `which` crate).
fn which_ok(bin: &str) -> bool {
    which::which(bin).is_ok()
}

pub fn dependency_checks() -> Report {
    let qs_out =
        if which_ok("quickshell") { probe("quickshell", &["--version"]) } else { None };
    Report(vec![
        bins::check_quickshell(qs_out.as_deref()),
        bins::check_bin("hyprctl", which_ok("hyprctl")),
        bins::check_bin("git", which_ok("git")),
        bins::check_bin("bash", which_ok("bash")),
        env::check_wayland(std::env::var("WAYLAND_DISPLAY").ok().as_deref()),
        env::check_desktop(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref()),
    ])
}
