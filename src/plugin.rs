//! `opb plugin …` — thin passthrough to upstream `bin/omarchy plugin …`
//! (anti-duplication §5.3). This is the ONLY mutation path: upstream owns
//! enable/disable/placement semantics end-to-end (D13), driving them over
//! IPC so the running shell is the single writer of shell.json.
//!
//! opb adds exactly two things:
//! - the environment (OMARCHY_PATH, PATH prepend)
//! - a §10 conflict pre-flight on `plugin enable <id>`: detected collisions
//!   are shown and confirmed before anything runs (`--yes` skips the prompt;
//!   the flag is consumed only for enable/disable, where upstream has no use
//!   for it — other verbs forward it untouched for upstream's own flows)
//!
//! stdio is inherited so interactive flows and warnings surface verbatim;
//! upstream's exit code propagates (signal death → 1).

use anyhow::{bail, Context, Result};
use std::io::BufRead;
use std::process::Command;

use crate::env;
use crate::paths::Paths;
use crate::pin;

/// Run `bin/omarchy plugin <args…>` inside the active pin; returns its exit code.
pub fn run(paths: &Paths, args: &[String]) -> Result<i32> {
    let pin_dir = pin::active_dir(paths)?;
    let omarchy = pin_dir.join("bin/omarchy");
    if !omarchy.is_file() {
        bail!("upstream helper missing: {}", omarchy.display());
    }

    // Conflict pre-flight: only for toggles, and only enable can create a
    // collision (§10 matrix rows are about running alternatives).
    let forward: Vec<String> = match extract_toggle(args) {
        Some((verb, id, rest)) => {
            let (consent, mut rest) = take_consent(&rest);
            if verb == "enable" && !consent && !confirm_enable(paths, &id)? {
                println!("opb plugin: aborted");
                return Ok(1);
            }
            let mut rebuilt = vec![verb, id];
            rebuilt.append(&mut rest);
            rebuilt
        }
        None => args.to_vec(),
    };
    exec(&omarchy, &pin_dir, &forward)
}

fn exec(
    omarchy: &std::path::Path,
    pin_dir: &std::path::Path,
    forward: &[String],
) -> Result<i32> {
    let status = Command::new(omarchy)
        .arg("plugin")
        .args(forward)
        .envs(env::for_pin(pin_dir))
        .status()
        .with_context(|| format!("spawn {}", omarchy.display()))?;
    Ok(status.code().unwrap_or(1))
}

/// Peel `"enable"|"disable" <id>` off the front of the forwarded args.
/// Flags before the id (-h, --help) mean there is no toggle to pre-flight.
pub fn extract_toggle(args: &[String]) -> Option<(String, String, Vec<String>)> {
    let mut iter = args.iter();
    let verb = iter.next()?;
    if verb != "enable" && verb != "disable" {
        return None;
    }
    let id = iter.next()?;
    if id.starts_with('-') || id.is_empty() {
        return None; // help flags / missing id: forward verbatim, upstream explains
    }
    Some((verb.clone(), id.clone(), args[2..].to_vec()))
}

/// Consume an opb-level `-y/--yes` (first occurrence) from the remaining args.
/// Returns (consent, args-without-it).
pub fn take_consent(args: &[String]) -> (bool, Vec<String>) {
    let mut consent = false;
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        if !consent && (a == "--yes" || a == "-y") {
            consent = true;
            continue;
        }
        out.push(a.clone());
    }
    (consent, out)
}

/// §10 pre-flight for `enable <id>`: true = proceed, false = abort.
/// Unknown ids / missing shell.json / no bus → forward silently (upstream
/// owns error reporting); only *detected collisions for this very component*
/// trigger a prompt. Bar-style INFO rows never block (coexistence is fine).
pub fn confirm_enable(paths: &Paths, id: &str) -> Result<bool> {
    let Some(doc) = maybe_doc(paths)? else {
        return Ok(true);
    };
    let mut enabled = crate::doctor::shellcfg::enabled_components(&doc);
    if !enabled.iter().any(|c| c == id) {
        enabled.push(id.to_owned());
    }
    let Some(processes) = crate::doctor::live_bus_processes() else {
        return Ok(true); // no busctl: nothing detected, nothing to ask about
    };
    let refs: Vec<&str> = enabled.iter().map(String::as_str).collect();
    let hits = collisions_for(&crate::doctor::conflicts::scan(&processes, &refs), id);
    if hits.is_empty() {
        return Ok(true);
    }
    println!("opb plugin: enabling {id} collides with:");
    for h in &hits {
        println!("  - {h}");
    }
    print!("Proceed anyway? [y/N] ");
    use std::io::Write;
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).ok();
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// Pure: warning-level conflicts for this exact component — other rows
/// (pre-existing states, INFO bar coexistence) never block.
fn collisions_for(results: &[crate::check::CheckResult], id: &str) -> Vec<String> {
    results
        .iter()
        .filter(|c| c.name == id)
        .filter_map(|c| match &c.status {
            crate::check::Status::Warn(d) => Some(d.clone()),
            _ => None,
        })
        .collect()
}

fn maybe_doc(paths: &Paths) -> Result<Option<serde_json::Value>> {
    let path = paths.shell_json();
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    Ok(Some(
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Paths rooted at `root` with `current` symlinked to `target`.
    fn fixture(root: &Path, target: &Path) -> Paths {
        let upstream = root.join("data/opb/upstream");
        std::fs::create_dir_all(&upstream).unwrap();
        std::os::unix::fs::symlink(target, upstream.join("current")).unwrap();
        Paths::from_parts(
            root.to_path_buf(),
            root.join("data"),
            root.join("config"),
        )
    }

    #[test]
    fn not_bootstrapped_errors() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        let err = run(&paths, &["list".to_owned()]).unwrap_err();
        assert!(err.to_string().contains("not bootstrapped"), "got: {err}");
    }

    #[test]
    fn missing_helper_errors() {
        let dir = tempfile::tempdir().unwrap();
        // current → empty tempdir: no bin/omarchy inside.
        let paths = fixture(dir.path(), dir.path());
        let err = run(&paths, &[]).unwrap_err();
        assert!(
            err.to_string().contains("upstream helper missing"),
            "got: {err}"
        );
    }

    #[test]
    fn forwards_args_verbatim_and_propagates_exit_code() {
        // Fake pin: bin/omarchy exits 7 regardless of args; exit code is the
        // passthrough contract (verbatim argv verified live in C1's gate).
        let pin = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let bin = pin.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("omarchy"), "#!/bin/sh\nexit 7\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join("omarchy"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        let paths = fixture(dir.path(), pin.path());
        let code = run(
            &paths,
            &[
                "validate".to_owned(),
                "--flag".to_owned(),
                "value with spaces".to_owned(),
            ],
        )
        .unwrap();
        assert_eq!(code, 7);
    }

    #[test]
    fn toggle_extraction() {
        let mk = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };
        let (verb, id, rest) =
            extract_toggle(&mk(&["enable", "omarchy.clock", "--section", "center"])).unwrap();
        assert_eq!((verb.as_str(), id.as_str()), ("enable", "omarchy.clock"));
        assert_eq!(rest, mk(&["--section", "center"]));
        assert!(extract_toggle(&mk(&["disable", "x"])).is_some());
        assert!(extract_toggle(&mk(&["list"])).is_none());
        assert!(extract_toggle(&mk(&["enable", "--help"])).is_none());
        assert!(extract_toggle(&mk(&["enable"])).is_none());
        assert!(extract_toggle(&mk(&["add", "url"])).is_none());
    }

    #[test]
    fn consent_taken_once_and_only_leading_flag() {
        let mk = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };
        let (yes, rest) = take_consent(&mk(&["--yes", "--section", "left"]));
        assert!(yes);
        assert_eq!(rest, mk(&["--section", "left"]));
        let (no, rest) = take_consent(&mk(&["--section", "left"]));
        assert!(!no);
        assert_eq!(rest.len(), 2);
        let (yes, rest) = take_consent(&mk(&["-y"]));
        assert!(yes);
        assert!(rest.is_empty());
    }

    #[test]
    fn collisions_filter_targets_only_the_enabled_component() {
        use crate::check::CheckResult;
        let results = vec![
            CheckResult::warn("omarchy.notifications", "mako owns the notifications bus name"),
            CheckResult::info("omarchy.bar", "another bar is running: waybar"),
            CheckResult::warn("omarchy.polkit", "hyprpolkitagent is a polkit auth agent"),
        ];
        let hits = collisions_for(&results, "omarchy.notifications");
        assert_eq!(hits, ["mako owns the notifications bus name"]);
        // INFO rows never block their own component either.
        assert!(collisions_for(&results, "omarchy.bar").is_empty());
    }
}
