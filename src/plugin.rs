//! `opb plugin …` — thin passthrough to upstream `bin/omarchy plugin …`
//! (anti-duplication §5.3). opb adds nothing except the environment: args are
//! forwarded verbatim (no hardcoded subcommand list — upstream additions flow
//! through), stdio is inherited so interactive flows and warnings (e.g. the
//! unsandboxed-code notice on `add`) surface verbatim, and upstream's exit
//! code propagates.

use anyhow::{bail, Context, Result};
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
    let status = Command::new(&omarchy)
        .arg("plugin")
        .args(args)
        .envs(env::for_pin(&pin_dir))
        .status()
        .with_context(|| format!("spawn {}", omarchy.display()))?;
    Ok(status.code().unwrap_or(1)) // signal death → treated as failure
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
                "add".to_owned(),
                "--flag".to_owned(),
                "value with spaces".to_owned(),
            ],
        )
        .unwrap();
        assert_eq!(code, 7);
    }
}
