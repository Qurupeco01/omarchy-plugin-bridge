//! Resolved filesystem locations (CONCEPT §4, D12).
//!
//! Path resolution is **per owner**, never one blanket rule:
//! - opb-owned state → XDG-aware (`dirs::data_local_dir` → `$XDG_DATA_HOME` or
//!   `~/.local/share`). It is our state; nothing else reads it.
//! - omarchy config → **contract-fixed** at `$HOME/.config/omarchy`. Upstream
//!   QML hardcodes that exact path (no XDG support); honoring XDG would write
//!   where the shell never reads.
//! - hypr wiring → XDG-aware (`dirs::config_dir` → `$XDG_CONFIG_HOME` or
//!   `~/.config`). Hyprland resolves its config the same way.
//!
//! Pure (env reads happen in `from_env` only) — fully unit-testable by
//! injecting roots.
#![allow(dead_code)] // consumed by bootstrap (C3+)

use std::path::{Path, PathBuf};

/// First-party plugin namespace is reserved upstream; the pin dir is the
/// only place opb keeps upstream code.
pub const PIN_DIRNAME_PREFIX: &str = "omarchy@";
/// Symlink inside `upstream/` that points at the active pin (D9).
pub const CURRENT_LINK: &str = "current";
pub const LOCK_FILENAME: &str = "upstream.lock";

#[derive(Debug, Clone)]
pub struct Paths {
    home: PathBuf,
    data_root: PathBuf,
    config_root: PathBuf,
}

impl Paths {
    /// Resolve from the environment via the `dirs` crate (XDG-aware), falling
    /// back to the default `~/.local/share` and `~/.config` layouts.
    pub fn from_env() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let data_root = dirs::data_local_dir().unwrap_or_else(|| home.join(".local/share"));
        let config_root = dirs::config_dir().unwrap_or_else(|| home.join(".config"));
        Self::from_parts(home, data_root, config_root)
    }

    /// Default-layout constructor (XDG vars unset): `~/.local/share` +
    /// `~/.config`. Used by tests; matches what `from_env` yields by default.
    pub fn new(home: PathBuf) -> Self {
        let data_root = home.join(".local/share");
        let config_root = home.join(".config");
        Self::from_parts(home, data_root, config_root)
    }

    /// Explicit roots — embedders/tests pin every base dir.
    pub fn from_parts(home: PathBuf, data_root: PathBuf, config_root: PathBuf) -> Self {
        Self {
            home,
            data_root,
            config_root,
        }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// opb-owned state root (XDG-aware): `…/opb`.
    pub fn opb_state_dir(&self) -> PathBuf {
        self.data_root.join("opb")
    }

    /// `…/opb/upstream` — one dir per pin + `current` link.
    pub fn upstream_dir(&self) -> PathBuf {
        self.opb_state_dir().join("upstream")
    }

    /// Directory name for a pin at `commit`: `omarchy@<commit>`.
    pub fn pin_dirname(commit: &str) -> String {
        format!("{PIN_DIRNAME_PREFIX}{commit}")
    }

    /// Absolute dir for a pinned checkout at `commit` (D9: immutable).
    pub fn pin_dir(&self, commit: &str) -> PathBuf {
        self.upstream_dir().join(Self::pin_dirname(commit))
    }

    /// The `current` symlink pointing at the active pin.
    pub fn current_dir(&self) -> PathBuf {
        self.upstream_dir().join(CURRENT_LINK)
    }

    /// `…/opb/upstream.lock` — `{channel, ref, commit}`.
    pub fn lock_file(&self) -> PathBuf {
        self.opb_state_dir().join(LOCK_FILENAME)
    }

    /// `$HOME/.config/omarchy` — contract-fixed (upstream reads exactly this).
    pub fn omarchy_config_dir(&self) -> PathBuf {
        self.home.join(".config/omarchy")
    }

    /// `$HOME/.config/omarchy/shell.json` — opb-generated, authoritative (D6).
    pub fn shell_json(&self) -> PathBuf {
        self.omarchy_config_dir().join("shell.json")
    }

    /// Hyprland's config dir (XDG-aware): `…/hypr`.
    pub fn hypr_config_dir(&self) -> PathBuf {
        self.config_root.join("hypr")
    }

    /// `…/hypr/opb.conf` — the managed Hyprland block (D8).
    pub fn opb_conf(&self) -> PathBuf {
        self.hypr_config_dir().join("opb.conf")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn paths() -> Paths {
        Paths::new(PathBuf::from("/home/test"))
    }

    #[test]
    fn pin_dirname_prefixes_commit() {
        assert_eq!(
            Paths::pin_dirname("0123456789abcdef"),
            "omarchy@0123456789abcdef"
        );
    }

    #[test]
    fn default_layout_matches_concept() {
        let p = paths();
        assert_eq!(
            p.opb_state_dir(),
            PathBuf::from("/home/test/.local/share/opb")
        );
        assert_eq!(
            p.upstream_dir(),
            PathBuf::from("/home/test/.local/share/opb/upstream")
        );
        assert_eq!(
            p.pin_dir("abc"),
            PathBuf::from("/home/test/.local/share/opb/upstream/omarchy@abc")
        );
        assert_eq!(
            p.current_dir(),
            PathBuf::from("/home/test/.local/share/opb/upstream/current")
        );
        assert_eq!(
            p.lock_file(),
            PathBuf::from("/home/test/.local/share/opb/upstream.lock")
        );
        assert_eq!(
            p.shell_json(),
            PathBuf::from("/home/test/.config/omarchy/shell.json")
        );
        assert_eq!(
            p.hypr_config_dir(),
            PathBuf::from("/home/test/.config/hypr")
        );
        assert_eq!(
            p.opb_conf(),
            PathBuf::from("/home/test/.config/hypr/opb.conf")
        );
    }

    #[test]
    fn xdg_roots_move_opb_and_hypr_but_not_omarchy_config() {
        // D12: opb state + hypr wiring follow XDG; omarchy config is
        // contract-fixed at $HOME/.config/omarchy regardless.
        let p = Paths::from_parts(
            PathBuf::from("/home/test"),
            PathBuf::from("/mnt/data"),
            PathBuf::from("/mnt/config"),
        );
        assert_eq!(p.opb_state_dir(), PathBuf::from("/mnt/data/opb"));
        assert_eq!(p.hypr_config_dir(), PathBuf::from("/mnt/config/hypr"));
        assert_eq!(
            p.shell_json(),
            PathBuf::from("/home/test/.config/omarchy/shell.json")
        );
    }

    #[test]
    fn from_env_uses_home_and_xdg() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let p = Paths::from_env();
        assert_eq!(p.home(), Path::new(&home));
        // With XDG unset these are the fallbacks; asserting them only checks
        // the crate wiring, not the values themselves.
        assert!(p.opb_state_dir().ends_with("opb"));
        assert!(p.hypr_config_dir().ends_with("hypr"));
    }
}