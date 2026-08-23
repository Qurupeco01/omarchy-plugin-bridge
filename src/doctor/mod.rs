//! `opb doctor` — assembles tier-1 checks (CONCEPT §10) into a report.

mod bins;
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
    Report(vec![
        bins::check_quickshell(qs_out.as_deref()),
        bins::check_bin("hyprctl", hyprctl),
        bins::check_bin("git", which_ok("git")),
        bins::check_bin("bash", which_ok("bash")),
    ])
}

/// PATH lookup only — never executes the binary.
fn which_ok(bin: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .any(|p| p.is_file())
}
