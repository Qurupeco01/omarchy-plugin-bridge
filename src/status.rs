//! `opb status` — minimal doctor over what opb owns: pin state, session
//! wiring, shell process, the icon font opb installs. No third-party
//! dependency probes (that is the bootstrap preflight) and no forensics on the
//! user's system. One check framework, one rendering, one exit-code
//! convention (0 = no FAIL rows).

use crate::check::{CheckResult, Report};
use crate::paths::Paths;
use crate::pin::{short, PinLock};

pub fn report(paths: &Paths) -> Report {
    Report(vec![
        pin_check(paths),
        wiring_check(paths),
        process_check(paths),
        font_check(),
    ])
}

/// Bootstrapped? Is the recorded pin actually usable?
fn pin_check(paths: &Paths) -> CheckResult {
    let lock = match PinLock::load(paths) {
        Ok(Some(l)) => l,
        Ok(None) => return CheckResult::info("pin", "not bootstrapped — run `opb bootstrap`"),
        Err(e) => return CheckResult::fail("upstream.lock", format!("unreadable: {e:#}")),
    };
    if paths.pin_dir(&lock.commit).is_dir() {
        CheckResult::pass_info("pin", &format!("{} @ {}", lock.reference, short(&lock.commit)))
    } else {
        CheckResult::fail(
            "pin",
            format!(
                "dir missing for {} @ {} — run `opb bootstrap`",
                lock.reference,
                short(&lock.commit)
            ),
        )
    }
}

/// Session persistence is its own consent switch (D15) — report, never judge.
/// Enabled = managed wiring present AND activated in the user's config.
fn wiring_check(paths: &Paths) -> CheckResult {
    let wired = paths.opb_lua().exists();
    let required = crate::hypr::wiring_active(paths);
    match (wired, required) {
        (true, true) => {
            CheckResult::pass_info("session wiring", "enabled (autostarts with Hyprland)")
        }
        (true, false) => CheckResult::warn(
            "session wiring",
            "installed but not activated — run `opb enable` to add require(\"opb\")",
        ),
        _ => CheckResult::info("session wiring", "disabled — `opb enable` to autostart"),
    }
}

fn process_check(paths: &Paths) -> CheckResult {
    if crate::shell::is_running(paths) {
        CheckResult::info("shell process", "running")
    } else {
        CheckResult::info("shell process", "not running")
    }
}

/// opb-owned wiring: the icon font `opb enable` installs. Warning, not a
/// failure — a missing font degrades icons but keeps the shell functional.
fn font_check() -> CheckResult {
    crate::fonts::check(crate::fonts::probe_installed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic;
    use crate::pin::PinLock;
    use std::fs;

    fn status_of(report: &Report, name: &str) -> String {
        let c = report
            .0
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("no check named {name}"));
        format!("{:?}", c.status).split('(').next().unwrap().to_owned()
    }

    /// Fake pin dir + active link + lock.
    fn fixture() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        let commit = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0";
        let pin_dir = paths.pin_dir(commit);
        fs::create_dir_all(pin_dir.join("shell/plugins")).unwrap();
        fs::write(pin_dir.join("version"), "4.0.0.alpha\n").unwrap();
        PinLock::stable("v4.0.0", commit).save(&paths).unwrap();
        atomic::symlink_flip(&pin_dir, &paths.current_dir()).unwrap();
        (dir, paths)
    }

    #[test]
    fn not_bootstrapped_is_info_and_exits_zero() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        let r = report(&paths);
        assert_eq!(status_of(&r, "pin"), "Info");
        assert_eq!(r.exit_code(), 0);
        assert_eq!(r.0.len(), 4, "exactly pin + wiring + process + font rows");
    }

    #[test]
    fn healthy_fixture_passes_pin_and_exits_zero() {
        let (_d, paths) = fixture();
        let r = report(&paths);
        assert_eq!(status_of(&r, "pin"), "Pass");
        assert_eq!(status_of(&r, "session wiring"), "Info");
        assert_eq!(r.exit_code(), 0, "{:#?}", r);
    }

    #[test]
    fn missing_pin_dir_fails() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        // Lock records a commit whose dir was never created.
        PinLock::stable("v4.0.0", "ghost").save(&paths).unwrap();

        let r = report(&paths);
        assert_eq!(r.exit_code(), 1);
        assert_eq!(status_of(&r, "pin"), "Fail");
    }

    #[test]
    fn wired_but_not_activated_warns() {
        let (_d, paths) = fixture();
        let opb_lua = paths.opb_lua();
        fs::create_dir_all(opb_lua.parent().unwrap()).unwrap();
        fs::write(opb_lua, "-- managed\n").unwrap();

        let r = report(&paths);
        assert_eq!(status_of(&r, "session wiring"), "Warn");
        assert_eq!(r.exit_code(), 0, "not activated is a warning, not a failure");
    }
}
