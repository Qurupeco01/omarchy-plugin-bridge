//! `opb plugin …` — thin passthrough to upstream `bin/omarchy plugin …`
//! (anti-duplication §5.3). This is the ONLY mutation path: upstream owns
//! enable/disable/placement semantics end-to-end (D13), driving them over
//! IPC so the running shell is the single writer of shell.json.
//!
//! Routing (CONCEPT §4 Plugin model):
//! - bare `plugin` / `-h` / `--help` → opb-branded static help (upstream's
//!   verbatim help leaks branding and hides our additions; drift is guarded
//!   by the CI sentinel on `bin/omarchy-plugin-*`)
//! - bare `list` → the read-only x-ray (`plugin_list`) — upstream's list is
//!   IPC-only; ours works headless
//! - args containing `--upstream` → flag consumed, rest forwarded verbatim
//!   (raw upstream view; skips all opb additions by design)
//! - everything else → forwarded verbatim (conflicts between desktop apps
//!   are out of scope, ADR-0008); `add` gets a gum pre-flight so a missing
//!   `gum` refuses up front instead of dying mid-clone
//!
//! stdio is inherited so interactive flows and warnings surface verbatim;
//! upstream's exit code propagates (signal death → 1).

use anyhow::{bail, Context, Result};
use std::process::Command;

use crate::env;
use crate::paths::Paths;

/// Where a `plugin …` arg vector goes.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    Help,
    ListXray,
    Forward(Vec<String>),
}

/// Pure arg routing. `--upstream` anywhere wins: raw passthrough of the rest.
pub fn route(args: &[String]) -> Route {
    if args.is_empty()
        || args == ["-h"]
        || args == ["--help"]
        || args == ["help"] // clap muscle memory; upstream has no such verb
    {
        return Route::Help;
    }
    let stripped: Vec<String> = args
        .iter()
        .filter(|a| a.as_str() != "--upstream")
        .cloned()
        .collect();
    if stripped.len() != args.len() {
        return Route::Forward(stripped);
    }
    if stripped.as_slice() == ["list"] {
        return Route::ListXray;
    }
    Route::Forward(stripped)
}

const HELP: &str = "\
opb plugin — manage omarchy-shell plugins (thin wrapper over upstream)

Acquire:
  opb plugin add <git-url> [--enable] [--yes]  add a plugin from git
  opb plugin clone <source-id> [--edit]        clone a built-in plugin into your config
  opb plugin remove [id] [--yes]               remove an installed plugin
  opb plugin update [id] [--yes]               update git-managed plugins

Activate (requires a running shell — `opb up`; state persists via IPC):
  opb plugin enable <id> [placement]           placement: --section left|center|right,
                                               --index N, --before/--after <widget-id>
  opb plugin disable <id>

Inspect:
  opb plugin list                              installed plugins × enabled state × live conflicts
  opb plugin list --json                       machine-readable output (forwarded to upstream)
  opb plugin list --upstream                   raw upstream view over IPC (needs running shell)
  opb plugin validate <folder>                 validate a manifest folder

opb additions: enable prompts before activating a component that collides with a
running service (§10); pass --yes to skip. Other flags forward untouched.
";

/// Run `bin/omarchy plugin <args…>` per the routing above; returns the exit code.
pub fn run(paths: &Paths, args: &[String]) -> Result<i32> {
    match route(args) {
        Route::Help => {
            print!("{HELP}");
            Ok(0)
        }
        Route::ListXray => {
            let rows = crate::plugin_list::list_rows(paths)?;
            print!("{}", crate::plugin_list::render_rows(&rows));
            Ok(0)
        }
        Route::Forward(forward) => {
            // Spell the tree through `current`: quickshell IPC matches
            // instances by exact config path, and upstream's own helpers
            // derive theirs from OMARCHY_PATH (see shell::shell_dir).
            if !paths.current_dir().is_symlink() {
                bail!("not bootstrapped — run `opb bootstrap` first");
            }
            let pin_dir = paths.current_dir();
            let omarchy = pin_dir.join("bin/omarchy");
            if !omarchy.is_file() {
                bail!("upstream helper missing: {}", omarchy.display());
            }

            if forward.first().map(String::as_str) == Some("add") {
                gum_guard(&forward)?;
            }
            exec(&omarchy, &pin_dir, &forward)
        }
    }
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

/// Upstream's `plugin add` shells out to `gum` for every interactive step
/// (URL input, trust confirm, bar placement). On a raw system without gum the
/// forwarded script dies mid-flight under `set -e`; refuse up front instead.
/// Non-interactive runs never reach gum (upstream fails cleanly on its own),
/// `--yes` skips confirm/placement, and help output needs nothing.
fn gum_guard(forward: &[String]) -> Result<()> {
    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin())
        && std::io::IsTerminal::is_terminal(&std::io::stdout());
    if !interactive {
        return Ok(());
    }
    if let Some(reason) = gum_block_reason(forward, which::which("gum").is_ok()) {
        bail!("add: {reason}");
    }
    Ok(())
}

/// Pure decision for [`gum_guard`]: Some(reason) when the run would die at a
/// gum prompt. A missing URL arg needs `gum input` even under `--yes`.
fn gum_block_reason(forward: &[String], gum_installed: bool) -> Option<String> {
    if gum_installed || forward.first().map(String::as_str) != Some("add") {
        return None;
    }
    let rest = &forward[1..];
    if rest.iter().any(|a| a == "-h" || a == "--help") {
        return None;
    }
    let has_url = rest.iter().any(|a| !a.starts_with('-'));
    let has_yes = rest.iter().any(|a| a == "--yes" || a == "-y");
    if has_url && has_yes {
        return None;
    }
    let hint = match (has_url, has_yes) {
        (true, true) => unreachable!("covered by the early return above"),
        (true, false) => "pass --yes to skip its prompts",
        (false, true) => "a git URL argument is required (--yes does not replace it)",
        (false, false) => "pass a git URL plus --yes to skip its prompts",
    };
    Some(format!(
        "upstream's add flow needs `gum` for its interactive prompts \
         (not installed) — install it (`sudo pacman -S gum`) or {hint}"
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
    fn routing_covers_all_shapes() {
        let mk = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };
        assert_eq!(route(&[]), Route::Help);
        assert_eq!(route(&mk(&["-h"])), Route::Help);
        assert_eq!(route(&mk(&["--help"])), Route::Help);
        assert_eq!(route(&mk(&["help"])), Route::Help);
        assert_eq!(route(&mk(&["list"])), Route::ListXray);
        // Escape hatch: --upstream consumes itself, forwards the rest — even bare list.
        assert_eq!(route(&mk(&["list", "--upstream"])), Route::Forward(mk(&["list"])));
        assert_eq!(
            route(&mk(&["--upstream", "--json"])),
            Route::Forward(mk(&["--json"]))
        );
        // Machine-readable list forwards; other verbs forward untouched.
        assert_eq!(
            route(&mk(&["list", "--json"])),
            Route::Forward(mk(&["list", "--json"]))
        );
        assert_eq!(
            route(&mk(&["add", "owner/repo", "--enable"])),
            Route::Forward(mk(&["add", "owner/repo", "--enable"]))
        );
        assert_eq!(
            route(&mk(&["validate", "--flag", "value with spaces"])),
            Route::Forward(mk(&["validate", "--flag", "value with spaces"]))
        );
    }

    #[test]
    fn help_works_without_a_pin() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        assert_eq!(run(&paths, &[]).unwrap(), 0);
    }

    #[test]
    fn not_bootstrapped_errors() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        let err = run(&paths, &["add".to_owned(), "url".to_owned()]).unwrap_err();
        assert!(err.to_string().contains("not bootstrapped"), "got: {err}");
    }

    #[test]
    fn missing_helper_errors() {
        let dir = tempfile::tempdir().unwrap();
        // current → empty tempdir: no bin/omarchy inside.
        let paths = fixture(dir.path(), dir.path());
        let err = run(&paths, &["validate".to_owned()]).unwrap_err();
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
    fn gum_guard_blocks_exactly_the_dying_shapes() {
        let mk = |v: &[&str]| -> Vec<String> { v.iter().map(|s| String::from(*s)).collect() };

        // The reported failure: URL given, interactive, no --yes.
        let msg = gum_block_reason(&mk(&["add", "https://x/y.git"]), false).unwrap();
        assert!(msg.contains("--yes"));
        // Bare add dies at `gum input` even with --yes.
        let msg = gum_block_reason(&mk(&["add", "--yes"]), false).unwrap();
        assert!(msg.contains("URL argument"));
        // Both missing: one message naming both ways out.
        let msg = gum_block_reason(&mk(&["add"]), false).unwrap();
        assert!(msg.contains("--yes") && msg.contains("URL"));
        // Surviving shapes.
        assert!(gum_block_reason(&mk(&["add", "url", "--yes"]), false).is_none());
        assert!(gum_block_reason(&mk(&["add", "-y", "u"]), false).is_none());
        assert!(gum_block_reason(&mk(&["add", "url", "--yes"]), true).is_none());
        assert!(gum_block_reason(&mk(&["add", "-h"]), false).is_none());
        // Non-add vectors are not this guard's business.
        assert!(gum_block_reason(&mk(&["remove", "id"]), false).is_none());
        // Flags are never mistaken for the URL positional.
        let msg = gum_block_reason(&mk(&["add", "--enable", "--yes"]), false).unwrap();
        assert!(msg.contains("URL argument"));
    }
}
