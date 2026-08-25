//! `opb status` — read-only snapshot of dependencies, pin state, generations,
//! and distance from the channel head. One check framework, one rendering,
//! one exit-code convention (0 = no FAIL rows).

use crate::check::{CheckResult, Report};
use crate::git;
use crate::paths::Paths;
use crate::pin::{short, PinLock};
use anyhow::Context;

const SCRATCH_PREFIXES: [&str; 2] = [".clone-tmp", ".update-tmp"];

pub fn report(paths: &Paths) -> Report {
    let mut checks: Vec<CheckResult> = crate::checks::dependency_checks().0;
    let lock = match PinLock::load(paths) {
        Ok(Some(l)) => Some(l),
        Ok(None) => None,
        Err(e) => {
            checks.push(CheckResult::fail("upstream.lock", format!("unreadable: {e:#}")));
            return Report(checks);
        }
    };
    let Some(lock) = lock else {
        checks.push(CheckResult::info("pin", "not bootstrapped — run `opb bootstrap`"));
        return Report(checks);
    };
    checks.push(CheckResult::pass_info(
        "pin",
        &format!("{} @ {}", lock.reference, short(&lock.commit)),
    ));

    // current link ↔ lock agreement (a mismatch means someone flipped by hand)
    match std::fs::read_link(paths.current_dir()) {
        Ok(target) => {
            let expected = paths.pin_dir(&lock.commit);
            if target == expected {
                checks.push(CheckResult::pass("current link"));
            } else {
                checks.push(CheckResult::fail(
                    "current link",
                    format!(
                        "points at {} but lock records {}",
                        target.display(),
                        expected.display()
                    ),
                ));
            }
        }
        Err(_) => {
            checks.push(CheckResult::fail("current link", "missing — run `opb bootstrap`"))
        }
    }

    if paths.pin_dir(&lock.commit).is_dir() {
        checks.push(CheckResult::pass("pin dir"));
    } else {
        checks.push(CheckResult::fail(
            "pin dir",
            format!("missing for {}", short(&lock.commit)),
        ));
    }

    match &lock.previous {
        Some(prev) => {
            if paths.pin_dir(&prev.commit).is_dir() {
                checks.push(CheckResult::info(
                    "previous generation",
                    format!(
                        "{} @ {} — `opb update rollback` available",
                        prev.reference,
                        short(&prev.commit)
                    ),
                ));
            } else {
                checks.push(CheckResult::warn(
                    "previous generation",
                    format!("lock records {} but its dir is gone", short(&prev.commit)),
                ));
            }
        }
        None => {
            checks.push(CheckResult::info("previous generation", "none (bootstrap state)"))
        }
    }

    match upstream_entries(paths) {
        Ok(names) => {
            let pins: Vec<&String> =
                names.iter().filter(|n| n.starts_with(crate::paths::PIN_DIRNAME_PREFIX)).collect();
            if pins.len() > 2 {
                checks.push(CheckResult::warn(
                    "generations",
                    format!(
                        "{} pin dirs on disk (retention keeps 2) — re-run any flip to prune",
                        pins.len()
                    ),
                ));
            } else {
                checks.push(CheckResult::pass_info(
                    "generations",
                    &format!("{} on disk", pins.len()),
                ));
            }
            let scratch: Vec<&String> = names
                .iter()
                .filter(|n| SCRATCH_PREFIXES.iter().any(|p| n.starts_with(p)))
                .collect();
            if scratch.is_empty() {
                checks.push(CheckResult::pass("scratch"));
            } else {
                checks.push(CheckResult::warn(
                    "scratch",
                    format!(
                        "leftover dirs from interrupted runs: {} — safe to delete",
                        scratch.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                    ),
                ));
            }
        }
        Err(e) => checks.push(CheckResult::warn("generations", format!("{e:#}"))),
    }

    if !paths.shell_json().exists() {
        checks.push(CheckResult::warn(
            "shell.json",
            "missing — the shell falls back to defaults; regenerate with `opb bootstrap --redo`",
        ));
    } else {
        match std::fs::read_to_string(paths.shell_json())
            .map_err(anyhow::Error::from)
            .and_then(|raw| {
                serde_json::from_str::<serde_json::Value>(&raw)
                    .map_err(anyhow::Error::new)
            }) {
            Ok(_) => checks.push(CheckResult::pass("shell.json")),
            Err(e) => checks.push(CheckResult::fail("shell.json", format!("unparseable: {e}"))),
        }
    }

    if crate::shell::is_running(paths) {
        checks.push(CheckResult::info("shell process", "running"));
    } else {
        checks.push(CheckResult::info("shell process", "not running"));
    }

    // Session persistence is its own consent switch (D15) — report, never judge.
    // Enabled = managed wiring present AND activated in the user's config.
    let wired = paths.opb_lua().exists();
    let required = std::fs::read_to_string(paths.hyprland_lua())
        .map(|src| crate::hypr::already_required(&src))
        .unwrap_or(false);
    match (wired, required) {
        (true, true) => checks.push(CheckResult::pass_info(
            "session wiring",
            "enabled (autostarts with Hyprland)",
        )),
        (true, false) => checks.push(CheckResult::warn(
            "session wiring",
            "installed but not activated — run `opb enable` to add require(\"opb\")",
        )),
        _ => checks.push(CheckResult::info(
            "session wiring",
            "disabled — `opb enable` to autostart",
        )),
    }

    // Network last so local answers render even when offline.
    checks.push(channel_check(
        &lock.reference,
        git::latest_tag(git::REMOTE).ok(),
    ));

    Report(checks)
}

/// Compare pinned ref with the remote's newest release tag.
fn channel_check(pinned: &str, latest: Option<String>) -> CheckResult {
    match latest {
        None => CheckResult::warn(
            "channel",
            "remote unreachable — cannot compare against the newest tag",
        ),
        Some(tag) if tag == pinned => {
            CheckResult::pass_info("channel", &format!("up to date with newest tag ({tag})"))
        }
        Some(tag) => CheckResult::info(
            "channel",
            format!("newer tag available: {tag} — run `opb update`"),
        ),
    }
}

/// File names inside the upstream dir (pins + transient scratch).
fn upstream_entries(paths: &Paths) -> anyhow::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(paths.upstream_dir())
        .with_context(|| format!("read {}", paths.upstream_dir().display()))?
    {
        let entry = entry.with_context(|| "read upstream dir entry")?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic;
    use crate::check::Status;
    use crate::pin::PreviousPin;
    use crate::shelljson;
    use std::fs;

    fn find<'a>(report: &'a Report, name: &str) -> Option<&'a CheckResult> {
        report.0.iter().find(|c| c.name == name)
    }

    fn status_of(report: &Report, name: &str) -> String {
        let c = find(report, name).unwrap_or_else(|| panic!("no check named {name}"));
        format!("{:?}", c.status).split('(').next().unwrap().to_owned()
    }

    /// Fake pin dirs + active link + lock (optionally with previous).
    fn fixture(
        commits: &[&str],
        active: (&str, &str),
        prev: Option<(&str, &str)>,
    ) -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        for c in commits {
            let d = paths.pin_dir(c);
            fs::create_dir_all(d.join("shell/plugins")).unwrap();
            fs::write(d.join("version"), "4.0.0.alpha\n").unwrap();
        }
        let mut lock = PinLock::stable(active.0, active.1);
        if let Some((r, c)) = prev {
            lock.previous = Some(PreviousPin { reference: r.to_owned(), commit: c.to_owned() });
        }
        lock.save(&paths).unwrap();
        atomic::symlink_flip(&paths.pin_dir(active.1), &paths.current_dir()).unwrap();
        (dir, paths)
    }

    #[test]
    fn channel_check_covers_all_outcomes() {
        assert!(matches!(
            channel_check("v4.0.0", Some("v4.0.0".into())).status,
            Status::Pass(_)
        ));
        assert!(matches!(
            channel_check("v4.0.0", Some("v4.1.0".into())).status,
            Status::Info(_)
        ));
        assert!(matches!(channel_check("v4.0.0", None).status, Status::Warn(_)));
    }

    #[test]
    fn not_bootstrapped_is_info_and_exits_zero() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        let r = report(&paths);
        assert_eq!(status_of(&r, "pin"), "Info");
        assert_eq!(r.exit_code(), 0);
        // 6 dependency rows + the pin INFO: deps report even unbootstrapped.
        assert_eq!(r.0.len(), 7, "deps + pin info");
        for name in ["quickshell", "hyprctl", "git", "bash", "WAYLAND_DISPLAY"] {
            assert!(r.0.iter().any(|c| c.name == name), "missing dep row {name}");
        }
    }

    #[test]
    fn healthy_fixture_passes_every_local_check() {
        let (_d, paths) = fixture(&["aaa", "bbb"], ("v4.1.0", "bbb"), Some(("v4.0.0", "aaa")));
        fs::create_dir_all(paths.omarchy_config_dir()).unwrap();
        fs::write(
            paths.shell_json(),
            shelljson::render(&shelljson::generate(&["omarchy.clock".to_owned()])),
        )
        .unwrap();

        let r = report(&paths);
        assert_eq!(r.exit_code(), 0, "{:#?}", r);
        for name in ["pin", "current link", "pin dir", "generations", "scratch", "shell.json"] {
            assert_eq!(status_of(&r, name), "Pass", "{name}");
        }
        // previous generation is present → INFO with rollback hint.
        assert_eq!(status_of(&r, "previous generation"), "Info");
        let prev = find(&r, "previous generation").unwrap();
        if let crate::check::Status::Info(d) = &prev.status {
            assert!(d.contains("rollback"));
        } else {
            panic!("expected Info");
        }
    }

    #[test]
    fn lock_link_mismatch_fails() {
        let (_d, paths) = fixture(&["aaa", "bbb"], ("v4.1.0", "bbb"), None);
        // Flip the link by hand behind opb's back.
        atomic::symlink_flip(&paths.pin_dir("aaa"), &paths.current_dir()).unwrap();

        let r = report(&paths);
        assert_eq!(r.exit_code(), 1);
        assert_eq!(status_of(&r, "current link"), "Fail");
    }

    #[test]
    fn missing_pin_dir_fails() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        // Lock records a commit whose dir was never created.
        PinLock::stable("v4.0.0", "ghost").save(&paths).unwrap();

        let r = report(&paths);
        assert_eq!(r.exit_code(), 1);
        assert_eq!(status_of(&r, "pin dir"), "Fail");
    }

    #[test]
    fn stale_scratch_dirs_warn() {
        let (_d, paths) = fixture(&["aaa"], ("v4.0.0", "aaa"), None);
        fs::create_dir_all(
            paths
                .upstream_dir()
                .join(format!(".clone-tmp{}", std::process::id())),
        )
        .unwrap();

        let r = report(&paths);
        assert_eq!(status_of(&r, "scratch"), "Warn");
        assert_eq!(r.exit_code(), 0, "stale scratch is a warning, not a failure");
    }

    #[test]
    fn unparseable_shell_json_fails_missing_warns() {
        let (_d, paths) = fixture(&["aaa"], ("v4.0.0", "aaa"), None);

        // Missing → WARN only.
        let r = report(&paths);
        assert_eq!(status_of(&r, "shell.json"), "Warn");
        assert_eq!(r.exit_code(), 0);

        // Present but garbage → FAIL.
        fs::create_dir_all(paths.omarchy_config_dir()).unwrap();
        fs::write(paths.shell_json(), "not json").unwrap();
        let r = report(&paths);
        assert_eq!(status_of(&r, "shell.json"), "Fail");
        assert_eq!(r.exit_code(), 1);
    }

    #[test]
    fn more_than_two_generations_warn() {
        let (_d, paths) = fixture(&["aaa", "bbb", "ccc"], ("v4.2.0", "ccc"), None);

        let r = report(&paths);
        assert_eq!(status_of(&r, "generations"), "Warn");
        assert_eq!(r.exit_code(), 0);
    }

    #[test]
    fn lock_records_previous_whose_dir_is_gone() {
        let (_d, paths) = fixture(&["bbb"], ("v4.1.0", "bbb"), Some(("v4.0.0", "aaa")));

        let r = report(&paths);
        assert_eq!(status_of(&r, "previous generation"), "Warn");
    }
}

