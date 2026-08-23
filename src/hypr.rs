//! Hyprland wiring: the managed `opb.conf` block (D8) + source-line guard.
//!
//! `opb.conf` is the only thing opb writes under the user's Hyprland config.
//! It carries session-wide env (RESEARCH §3.1: keybind-spawned helpers need
//! `OMARCHY_PATH`/PATH, not just the shell process), the autostart line, and
//! commented example keybinds — zero active binds by construction (D11).

use anyhow::{bail, Context, Result};
use std::path::Path;

use crate::atomic;
use crate::paths::Paths;

/// Render the managed opb.conf. Absolute paths into the pin (D9: the
/// `current` symlink means this never changes across updates).
pub fn render(pin_dir: &Path, current_dir: &Path) -> String {
    format!(
        "# Managed by opb — regenerate with `opb bootstrap --redo`. Do not edit by hand.\n\
         # Session-wide env: the shell and keybind-spawned helpers (menu, lock, ...)\n\
         # both resolve the pin through these (RESEARCH §3.1).\n\
         env = OMARCHY_PATH,{pin}\n\
         env = PATH,{pin}/bin:$PATH\n\
         \n\
         # Autostart the shell (idempotent with `opb up`).\n\
         exec-once = quickshell -p {current}/shell\n\
         \n\
         # Example keybinds — commented by design: the shell never owns a keybind\n\
         # (D11). Uncomment to enable; bin/ resolves because of the PATH line above.\n\
         # bind = SUPER, SPACE, exec, omarchy-menu toggle\n\
         # bind = SUPER CTRL, V, exec, omarchy-shell shell toggle omarchy.clipboard\n",
        pin = pin_dir.display(),
        current = current_dir.display(),
    )
}

/// Does the user's hyprland.conf already `source` opb.conf? Matches any source
/// line mentioning the file name — covers absolute, `~`-relative, and quoted
/// paths. Pure and conservative: a false positive only withholds a file the
/// user can still source manually.
pub fn already_sourced(hyprland_conf: &str, opb_conf_name: &str) -> bool {
    hyprland_conf.lines().any(|line| {
        let t = line.trim();
        t.starts_with("source") && t.contains(opb_conf_name)
    })
}

/// Write opb.conf atomically. Refuses on fresh bootstrap when the user config
/// already sources it (double source = double autostart); `redo` rewrites
/// unconditionally — the user sourced it *because* of us.
pub fn wire(paths: &Paths, pin_dir: &Path, refuse_if_sourced: bool) -> Result<()> {
    let hyprland_conf = paths.hypr_config_dir().join("hyprland.conf");
    if refuse_if_sourced && hyprland_conf.exists() {
        let raw = std::fs::read_to_string(&hyprland_conf)
            .with_context(|| format!("read {}", hyprland_conf.display()))?;
        if already_sourced(&raw, "opb.conf") {
            bail!(
                "{} already sources opb.conf — refusing to double-source",
                hyprland_conf.display()
            );
        }
    }
    atomic::write(
        &paths.opb_conf(),
        render(pin_dir, &paths.current_dir()).as_bytes(),
    )
}

/// The single line to paste into hyprland.conf. `~`-relative when under home,
/// absolute otherwise (D12: the hypr config dir may be XDG-moved).
pub fn source_line(paths: &Paths) -> String {
    let conf = paths.opb_conf();
    let shown = match conf.strip_prefix(paths.home()) {
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => conf.display().to_string(),
    };
    format!("source = {shown}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn render_has_env_autostart_and_only_commented_binds() {
        let s = render(
            Path::new("/h/.local/share/opb/upstream/omarchy@abc"),
            Path::new("/h/.local/share/opb/upstream/current"),
        );
        assert!(s.contains("env = OMARCHY_PATH,/h/.local/share/opb/upstream/omarchy@abc"));
        assert!(s.contains("env = PATH,/h/.local/share/opb/upstream/omarchy@abc/bin:$PATH"));
        assert!(s.contains("exec-once = quickshell -p /h/.local/share/opb/upstream/current/shell"));
        assert!(s.contains("bind = SUPER, SPACE, exec, omarchy-menu toggle"));
        // Every bind line is commented: no active binds (D11).
        for line in s.lines() {
            let t = line.trim();
            if t.contains("bind =") && !t.starts_with('#') {
                panic!("active bind leaked: {line}");
            }
        }
    }

    #[test]
    fn already_sourced_detects_path_forms() {
        assert!(already_sourced("source = ~/.config/hypr/opb.conf", "opb.conf"));
        assert!(already_sourced("source = /home/u/.config/hypr/opb.conf", "opb.conf"));
        assert!(already_sourced("source = ~/.config/hypr/opb.conf  # opb", "opb.conf"));
        assert!(!already_sourced("source = ~/.config/hypr/opb", "opb.conf"));
        assert!(!already_sourced("bind = SUPER, Q, exec, opb.conf-thing", "opb.conf"));
        assert!(!already_sourced("", "opb.conf"));
    }

    #[test]
    fn source_line_is_tilde_relative_under_home() {
        let paths = Paths::new(PathBuf::from("/home/u"));
        assert_eq!(source_line(&paths), "source = ~/.config/hypr/opb.conf");
    }

    #[test]
    fn source_line_is_absolute_outside_home() {
        let paths = Paths::from_parts(
            PathBuf::from("/home/u"),
            PathBuf::from("/home/u/.local/share"),
            PathBuf::from("/mnt/config"),
        );
        assert_eq!(source_line(&paths), "source = /mnt/config/hypr/opb.conf");
    }

    #[test]
    fn wire_refuses_when_already_sourced() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        fs::create_dir_all(paths.hypr_config_dir()).unwrap();
        fs::write(
            paths.hypr_config_dir().join("hyprland.conf"),
            "source = ~/.config/hypr/opb.conf\n",
        )
        .unwrap();

        let pin = paths.pin_dir("abc");
        assert!(wire(&paths, &pin, true).is_err());
        // redo path rewrites despite the source line.
        assert!(wire(&paths, &pin, false).is_ok());
        assert!(paths.opb_conf().exists());
    }
}