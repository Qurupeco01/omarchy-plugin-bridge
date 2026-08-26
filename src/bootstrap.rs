//! `opb bootstrap` — clone + pin upstream, generate shell.json.
//!
//! Never moves an existing pin: that is `opb pin update`. Re-running reports the
//! current pin; `--redo` regenerates shell.json against the existing pin
//! without re-cloning. Bootstrap writes **no** Hyprland wiring (D15) —
//! session integration is opt-in via `opb enable`.

use anyhow::{bail, Context, Result};
use std::path::Path;

use crate::atomic;
use crate::git;
use crate::paths::Paths;
use crate::pin::{self, short, PinLock};
use crate::shelljson;

pub struct BootstrapOptions {
    /// Tag to pin; `None` = newest release tag on the remote.
    pub reference: Option<String>,
    /// Regenerate generated artifacts against the existing pin.
    pub redo: bool,
}

/// Requirement gate: opb installs nothing on a machine that cannot run the
/// result. Renders the failing checks and bails — we point at what's missing,
/// we don't analyze or fix the user's system.
pub fn require_dependencies() -> Result<()> {
    let report = crate::deps::dependency_checks();
    if report.exit_code() == super::exit::OK {
        return Ok(());
    }
    print!("{}", report.render());
    bail!("missing requirements — install them, then re-run `opb bootstrap`");
}

pub fn run(paths: &Paths, opts: &BootstrapOptions) -> Result<()> {
    if let Some((commit, pin_dir)) = pin::active_pin(paths)? {
        // Bootstrap is the installation step: re-running it also repairs
        // runtime installs (the icon font) on already-bootstrapped machines.
        if crate::fonts::ensure_installed(paths)? {
            println!("opb bootstrap: installed omarchy icon font");
        }
        return if opts.redo {
            regenerate(paths, &commit, &pin_dir)
        } else {
            println!(
                "opb bootstrap: already bootstrapped ({} @ {})",
                short(&commit),
                commit
            );
            println!("  move the pin with `opb pin update` (Phase 4)");
            println!("  re-generate generated artifacts with `opb bootstrap --redo`");
            Ok(())
        };
    }
    if opts.redo {
        bail!("not bootstrapped yet — nothing to redo; run `opb bootstrap` first");
    }

    let reference = match &opts.reference {
        Some(r) => r.clone(),
        None => git::latest_tag(git::REMOTE)?,
    };

    // Clone into a unique temp dir, resolve the commit, then rename into the
    // final pin dir (D9: `omarchy@<commit>`, immutable).
    let tmp = paths
        .upstream_dir()
        .join(format!("{}{}", crate::paths::SCRATCH_CLONE, std::process::id()));
    let result = (|| -> Result<()> {
        git::clone_shallow(git::REMOTE, &reference, &tmp)?;
        let commit = git::rev_parse_head(&tmp)?;
        let pin_dir = paths.pin_dir(&commit);
        if pin_dir.exists() {
            bail!("commit {commit} is already pinned; re-run without --ref");
        }
        // Validate the checkout while it still lives in `tmp`; a bad ref must
        // never become an active pin (everything-on-by-default footgun).
        ensure_pin_usable(&tmp, &commit).with_context(|| "clone is not an omarchy checkout")?;
        std::fs::rename(&tmp, &pin_dir)
            .with_context(|| format!("move clone to {}", pin_dir.display()))?;
        atomic::symlink_flip(&pin_dir, &paths.current_dir())?;
        let lock = PinLock::stable(&reference, &commit);
        lock.save(paths)?;
        let ids = shelljson::first_party_non_bar_ids(&pin_dir.join("shell/plugins"))?;
        atomic::write(
            &paths.shell_json(),
            shelljson::render(&shelljson::generate(&ids)).as_bytes(),
        )?;
        if crate::fonts::ensure_installed(paths)? {
            println!("opb bootstrap: installed omarchy icon font");
        }
        hyprless_success_message(paths, &reference, &commit);
        Ok(())
    })();
    if result.is_err() {
        // Never leave a half-cloned temp dir behind.
        if tmp.exists() {
            let _ = std::fs::remove_dir_all(&tmp);
        }
    }
    result
}

/// `--redo`: regenerate shell.json against the existing pin. opb.lua, when
/// installed, is refreshed by `opb enable` (idempotent) — not here.
fn regenerate(paths: &Paths, commit: &str, pin_dir: &Path) -> Result<()> {
    ensure_pin_usable(pin_dir, commit)?;
    let ids = shelljson::first_party_non_bar_ids(&pin_dir.join("shell/plugins"))?;
    atomic::write(
        &paths.shell_json(),
        shelljson::render(&shelljson::generate(&ids)).as_bytes(),
    )?;
    println!(
        "opb bootstrap: regenerated shell.json against {} ({})",
        short(commit),
        pin_dir.display()
    );
    Ok(())
}

/// Bootstrap output: pin facts + the opt-in session-integration hint.
fn hyprless_success_message(paths: &Paths, reference: &str, commit: &str) {
    println!("opb bootstrap: pinned {reference} at {commit}");
    println!("  pin:  {}", paths.pin_dir(commit).display());
    println!("  link: {} -> current", paths.current_dir().display());
    println!("  config: {}", paths.shell_json().display());
    println!("  start the shell now:      opb up");
    println!("  autostart it per session: opb enable");
}

/// A pin is only usable if it carries the shell plugin tree — anything else
/// means the ref resolved to a non-omarchy commit (or the dir was mangled) —
/// and clears the quattro support floor (CONCEPT §8). Shared with `opb pin update`'s
/// pre-flip clone validation.
pub(crate) fn ensure_pin_usable(pin_dir: &Path, commit: &str) -> Result<()> {
    let plugins = pin_dir.join("shell/plugins");
    if !plugins.is_dir() {
        bail!(
            "pin {commit} has no shell/plugins dir — not an omarchy checkout; \
             delete the pin and bootstrap again"
        );
    }
    pin::ensure_supported_tree(pin_dir)
        .with_context(|| format!("pin {commit} rejected"))
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn home() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    /// Bootstrap a fake pin inside `home`'s layout: pin dir, `current` link,
    /// lock, and a minimal plugins tree (version file clears the floor).
    fn fake_pin(home: &std::path::Path) -> (String, PathBuf) {
        let commit = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0";
        let paths = Paths::new(home.to_path_buf());
        let pin_dir = paths.pin_dir(commit);
        fs::create_dir_all(&pin_dir).unwrap();
        std::fs::write(pin_dir.join("version"), "4.0.0.alpha\n").unwrap();
        let plugins = pin_dir.join("shell/plugins");
        fs::create_dir_all(plugins.join("bar")).unwrap();
        fs::write(
            plugins.join("bar/manifest.json"),
            r#"{"id":"omarchy.bar","kinds":["bar"]}"#,
        )
        .unwrap();
        fs::create_dir_all(plugins.join("lock")).unwrap();
        fs::write(
            plugins.join("lock/manifest.json"),
            r#"{"id":"omarchy.lock","kinds":["service"]}"#,
        )
        .unwrap();
        (commit.to_owned(), pin_dir)
    }

    #[test]
    fn redo_regenerates_shell_json_from_pin() {
        let dir = home();
        let paths = Paths::new(dir.path().to_path_buf());
        let (commit, pin_dir) = fake_pin(dir.path());
        PinLock::stable("v4.0.0", &commit).save(&paths).unwrap();
        atomic::symlink_flip(&pin_dir, &paths.current_dir()).unwrap();

        run(&paths, &BootstrapOptions { reference: None, redo: true }).unwrap();

        let cfg: serde_json::Value = serde_json::from_str(&fs::read_to_string(paths.shell_json()).unwrap()).unwrap();
        let disabled: Vec<_> = cfg["disabledPlugins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert_eq!(disabled, ["omarchy.lock"]);
        assert_eq!(cfg["bar"]["id"], "omarchy.bar");
    }

    #[test]
    fn already_bootstrapped_without_redo_is_noop() {
        let dir = home();
        let paths = Paths::new(dir.path().to_path_buf());
        let (commit, pin_dir) = fake_pin(dir.path());
        PinLock::stable("v4.0.0", &commit).save(&paths).unwrap();
        atomic::symlink_flip(&pin_dir, &paths.current_dir()).unwrap();

        run(&paths, &BootstrapOptions { reference: None, redo: false }).unwrap();

        // Nothing generated: the "already bootstrapped" path writes nothing.
        assert!(!paths.shell_json().exists());
    }

    #[test]
    fn redo_without_existing_pin_is_an_error() {
        let dir = home();
        let paths = Paths::new(dir.path().to_path_buf());
        assert!(run(&paths, &BootstrapOptions { reference: None, redo: true }).is_err());
    }

    #[test]
    fn redo_on_missing_pin_dir_errors() {
        let dir = home();
        let paths = Paths::new(dir.path().to_path_buf());
        // Lock present but pin dir absent — a broken state. fake_pin() would
        // create the dir, so write the lock by hand.
        PinLock::stable("v4.0.0", "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0")
            .save(&paths)
            .unwrap();

        assert!(run(&paths, &BootstrapOptions { reference: None, redo: true }).is_err());
    }
}