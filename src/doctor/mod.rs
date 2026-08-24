//! `opb doctor` — assembles tier-1 checks (CONCEPT §10) into a report.

mod bins;
pub(crate) mod conflicts;
mod env;
pub(crate) mod shellcfg;
mod version;

use crate::check::Report;
use std::process::Command;

/// Runs `<bin> <args>`, capturing stdout. `None` = not spawned
/// (binary missing from PATH or spawn failed for any reason).
fn probe(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Live session-bus process names, deduplicated. `None` = busctl unavailable
/// or no session bus (callers decide how to degrade).
pub fn live_bus_processes() -> Option<Vec<String>> {
    let out = probe("busctl", &["--user", "list", "--no-pager"])?;
    Some(conflicts::parse_processes(&out))
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
    // Enabled components come from `~/.config/omarchy/shell.json`, written by
    // bootstrap and thereafter by upstream only (D13). No file → nothing is
    // enabled (pre-bootstrap doctor is tier 1 only). The all-off file enables
    // exactly the bar, so the conflict scan reduces to the `omarchy.bar` INFO
    // row until components are activated via `opb plugin enable`.
    let paths = crate::paths::Paths::from_env();
    if paths.shell_json().exists() {
        let raw = std::fs::read_to_string(paths.shell_json())
            .unwrap_or_else(|_| String::new());
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(config) => {
                let enabled = shellcfg::enabled_components(&config);
                if !enabled.is_empty() {
                    let refs: Vec<&str> = enabled.iter().map(String::as_str).collect();
                    match probe("busctl", &["--user", "list", "--no-pager"]) {
                        Some(out) => checks.extend(conflicts::scan(
                            &conflicts::parse_processes(&out),
                            &refs,
                        )),
                        None => checks.push(crate::check::CheckResult::warn(
                            "conflicts",
                            "session bus scan skipped (busctl unavailable or no bus)",
                        )),
                    }
                }
            }
            Err(_) => checks.push(crate::check::CheckResult::warn(
                "shell.json",
                "exists but is unparseable; conflict checks skipped",
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
