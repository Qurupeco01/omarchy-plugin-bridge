//! Tier-1 binary checks (pure: probe output in, CheckResult out).

use super::version::{self, Version};
use crate::check::CheckResult;

/// Tested floor per CONCEPT §10 (pacman `quickshell`, not AUR `-git`).
pub const QUICKSHELL_FLOOR: Version = Version { major: 0, minor: 3, patch: 1 };

pub fn check_quickshell(probe: Option<&str>) -> CheckResult {
    let Some(out) = probe else {
        return CheckResult::fail("quickshell", "not found in PATH");
    };
    match version::parse(out) {
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

pub fn check_bin(name: &'static str, present: bool) -> CheckResult {
    if present {
        CheckResult::pass(name)
    } else {
        CheckResult::fail(name, "not found in PATH")
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
}
