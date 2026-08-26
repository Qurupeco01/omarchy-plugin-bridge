//! The `omarchy` icon font. The bar and menu render their brand glyphs
//! (U+E900–E907: omarchy logo, agent marks, install-ai icons) from the custom
//! `omarchy` font shipped as `default/fonts/omarchy/omarchy.ttf` inside the
//! pin. Nothing installs it on a raw system, so those icons render blank —
//! and a shell launched with plain `opb up` never runs `opb enable`'s wiring,
//! so the font is installed at **bootstrap**, the installation step. One
//! reversible file in fontconfig's user font dir, never a system font dir or
//! a fontconfig rule.

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
            "not installed — `opb bootstrap` copies it from the pin",
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed,
    AlreadyPresent,
}

/// Idempotent install that reports whether it changed anything — lets the
/// caller print a line only when the font actually landed.
pub fn ensure_installed(paths: &Paths) -> Result<bool> {
    Ok(install(paths)? == InstallOutcome::Installed)
}

/// Copy the active pin's icon font into the user font dir and refresh
/// fontconfig. Idempotent; atomic write.
pub fn install(paths: &Paths) -> Result<InstallOutcome> {
    let source = paths.current_dir().join(REL);
    let bytes = match std::fs::read(&source) {
        Ok(b) => b,
        // A pin that ships no icon font has nothing to install — not a
        // bootstrap failure.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InstallOutcome::AlreadyPresent)
        }
        Err(e) => return Err(e).with_context(|| format!("read {}", source.display())),
    };
    let dest = paths.fonts_dir().join("omarchy").join("omarchy.ttf");
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