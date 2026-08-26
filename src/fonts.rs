//! The `omarchy` icon font. The bar and menu render their brand glyphs
//! (U+E900–E907: omarchy logo, agent marks, install-ai icons) from the custom
//! `omarchy` font shipped as `default/fonts/omarchy/omarchy.ttf` inside the
//! pin. Nothing installs it on a raw system, so those icons render blank.
//! `opb enable` copies the file into fontconfig's user font dir and refreshes
//! the cache — one reversible file, never a system font dir or a fontconfig
//! rule.

use anyhow::{Context, Result};

use crate::check::CheckResult;
use crate::paths::Paths;

/// Relative path of the icon font inside a pin checkout.
const REL: &str = "default/fonts/omarchy/omarchy.ttf";

/// Is the omarchy font installed system-wide? Probes fontconfig for any font
/// covering U+E900 (the omarchy logo glyph). `None` = fc-list unavailable.
pub fn probe_installed() -> bool {
    let out = std::process::Command::new("fc-list")
        .args(["--format", "%{family}\n", ":charset=e900"])
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).contains("omarchy"),
        Err(_) => false,
    }
}

pub fn check(present: bool) -> CheckResult {
    if present {
        CheckResult::pass("omarchy icon font")
    } else {
        CheckResult::warn(
            "omarchy icon font",
            "not installed — `opb enable` copies it from the pin",
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed,
    AlreadyPresent,
}

/// Copy the active pin's icon font into the user font dir and refresh
/// fontconfig. Idempotent; atomic write.
pub fn install(paths: &Paths) -> Result<InstallOutcome> {
    let source = paths.current_dir().join(REL);
    let dest = paths.fonts_dir().join("omarchy").join("omarchy.ttf");
    let bytes = std::fs::read(&source)
        .with_context(|| format!("read {}", source.display()))?;
    if dest.is_file() && std::fs::read(&dest).ok().as_deref() == Some(bytes.as_slice()) {
        return Ok(InstallOutcome::AlreadyPresent);
    }
    let dir = dest.parent().expect("fonts dir has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    crate::atomic::write(&dest, &bytes).with_context(|| format!("write {}", dest.display()))?;
    refresh_cache()?;
    Ok(InstallOutcome::Installed)
}

fn refresh_cache() -> Result<()> {
    let status = std::process::Command::new("fc-cache")
        .arg("-f")
        .status()
        .with_context(|| "spawn fc-cache (fontconfig)")?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("fc-cache exited nonzero")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Fake install: current → pin carrying `default/fonts/omarchy/omarchy.ttf`.
    fn fixture(root: &Path) -> Paths {
        let pin = root.join("pin");
        let font = pin.join("default/fonts/omarchy");
        std::fs::create_dir_all(&font).unwrap();
        std::fs::write(font.join("omarchy.ttf"), b"fake ttf bytes").unwrap();

        let upstream = root.join("data/opb/upstream");
        std::fs::create_dir_all(&upstream).unwrap();
        std::os::unix::fs::symlink(&pin, upstream.join("current")).unwrap();

        Paths::from_parts(
            root.join("home"),
            root.join("data"),
            root.join("home/.config"),
        )
    }

    #[test]
    fn check_reflects_presence() {
        assert_eq!(check(true).status, crate::check::Status::Pass(None));
        assert!(matches!(check(false).status, crate::check::Status::Warn(_)));
    }

    #[test]
    fn install_writes_font_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let paths = fixture(dir.path());

        let dest = paths.fonts_dir().join("omarchy").join("omarchy.ttf");
        assert_eq!(install(&paths).unwrap(), InstallOutcome::Installed);
        assert_eq!(std::fs::read(&dest).unwrap(), b"fake ttf bytes");
        // Second run changes nothing and does not rewrite the cache again.
        assert_eq!(install(&paths).unwrap(), InstallOutcome::AlreadyPresent);
    }
}