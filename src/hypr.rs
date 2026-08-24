//! Hyprland wiring (D8, D15): one managed `~/.config/hypr/opb.lua`.
//!
//! Targets a **Lua-config Hyprland** through the native API only (`hl.on`,
//! `hl.exec_cmd`) — no dependency on omarchy helper functions. Activation is
//! the user adding a single `require("opb")` line; deactivation is removing it
//! plus `opb disable`. Every spawned command is env-wrapped (CONCEPT §4), so
//! nothing here touches session-wide environment.

use anyhow::{bail, Context, Result};
use std::path::Path;

use crate::atomic;
use crate::paths::Paths;

/// First line of every opb-generated Hyprland file — how we recognize our own
/// dead artifacts when cleaning them up.
pub const MANAGED_MARKER: &str = "# Managed by opb";

const TEMPLATE: &str = r#"-- Managed by opb — regenerate with `opb enable`. Do not edit by hand.
-- Session wiring for the pinned omarchy-shell: autostart + keybinds.
--
-- Activate by adding this line to your Hyprland Lua config:
--   require("opb")
-- Deactivate with `opb disable` and remove that line.
--
-- Keybinds live in OPB_KEYS below (user-owned): edit it freely, or manage
-- entries with `opb keys set` / `opb keys import-suggested`.

local OPB_PIN = [[<PIN>]]
local OPB_KEYS = [[<KEYS>]]

-- Self-contained exec: every opb-spawned command carries the pin env, so no
-- session-wide env is needed (CONCEPT §4 Environment handling).
local function opb_quote(s)
  return "'" .. s:gsub("'", "'\\''") .. "'"
end

local function opb_exec(cmd)
  local inner = "export OMARCHY_PATH=" .. opb_quote(OPB_PIN)
    .. "; export PATH=" .. opb_quote(OPB_PIN .. "/bin:$PATH")
    .. "; exec " .. cmd
  return "sh -c " .. opb_quote(inner)
end

-- Fires once per Hyprland instance — never on reload (exec-once semantics).
hl.on("hyprland.start", function()
  hl.exec_cmd(opb_exec("quickshell -p " .. opb_quote("<CURRENT>/shell")))
end)

-- Load user keybinds when present. Optional until `opb keys …` creates the
-- file; it stays authoritative — opb only ever appends entries.
local f = io.open(OPB_KEYS, "r")
if f then
  f:close()
  dofile(OPB_KEYS)
end
"#;

/// Render the managed opb.lua against the active pin.
pub fn render(pin_dir: &Path, current_dir: &Path, keys_path: &Path) -> String {
    TEMPLATE
        .replace("<PIN>", &pin_dir.display().to_string())
        .replace("<KEYS>", &keys_path.display().to_string())
        .replace("<CURRENT>", &current_dir.display().to_string())
}

/// The one line the user adds to their Hyprland Lua config.
pub fn require_hint() -> &'static str {
    "require(\"opb\")"
}

/// Does the user's Hyprland Lua config already activate our wiring? Matches
/// require/dofile forms mentioning opb — conservative: a false positive only
/// withholds a reminder the user can act on manually.
pub fn already_required(hyprland_lua_src: &str) -> bool {
    hyprland_lua_src.lines().any(|line| {
        let t = line.trim();
        if !t.contains("opb") {
            return false;
        }
        t.contains("require(\"opb\")")
            || t.contains("require('opb')")
            || t.contains("require \"opb\"")
            || t.contains("require 'opb'")
            || (t.starts_with("dofile") && t.contains("opb.lua"))
    })
}

/// Is this file ours (marker header), i.e. safe to delete during cleanup?
/// A user-modified or foreign opb.conf is left untouched.
pub fn is_managed(src: &str) -> bool {
    src.trim_start().starts_with(MANAGED_MARKER)
}

// --- Managed require block -------------------------------------------------
//
// `opb enable` offers to write the activation line itself, as a marker-scoped
// block so `opb disable` can remove exactly what it added — nothing else,
// even if the user moved or reformatted the surrounding file.

pub const REQUIRE_BEGIN: &str = "-- BEGIN opb (managed by `opb enable`; remove with `opb disable`)";
pub const REQUIRE_END: &str = "-- END opb";

/// The exact managed block we write into the user's hyprland.lua.
pub fn require_block() -> String {
    format!("{REQUIRE_BEGIN}\n{}\n{REQUIRE_END}\n", require_hint())
}

/// Pure: strip every managed block from a config, returning (cleaned, removed).
/// Only lines inside BEGIN/END markers are touched.
fn strip_require_blocks(src: &str) -> (String, bool) {
    let mut kept: Vec<&str> = Vec::new();
    let mut inside = false;
    let mut removed = false;
    for line in src.lines() {
        let t = line.trim_end();
        if t == REQUIRE_BEGIN {
            inside = true;
            removed = true;
            continue;
        }
        if inside {
            if t == REQUIRE_END {
                inside = false;
            }
            continue;
        }
        kept.push(line);
    }
    let mut cleaned = kept.join("\n");
    while cleaned.ends_with("\n\n") {
        cleaned.pop();
    }
    if !cleaned.is_empty() && !cleaned.ends_with('\n') {
        cleaned.push('\n');
    }
    (cleaned, removed)
}

/// Append the managed block. Idempotent: no-op when any form of activation is
/// already present. Returns whether the file was modified.
pub fn write_require_block(paths: &Paths) -> Result<bool> {
    let path = paths.hyprland_lua();
    let src = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    if already_required(&src) {
        return Ok(false);
    }
    let mut next = src.clone();
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(&require_block());
    atomic::write(&path, next.as_bytes())?;
    Ok(true)
}

/// Remove our managed block(s). Hand-written activation lines are left alone
/// — only marker-scoped content qualifies as ours. Returns whether the file
/// was modified.
pub fn remove_require_block(paths: &Paths) -> Result<bool> {
    let path = paths.hyprland_lua();
    if !path.exists() {
        return Ok(false);
    }
    let src = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    let (cleaned, removed) = strip_require_blocks(&src);
    if !removed {
        return Ok(false);
    }
    atomic::write(&path, cleaned.as_bytes())?;
    Ok(true)
}

/// Outcome of `opb enable` worth reporting to the user.
pub struct EnableReport {
    /// A stale managed `opb.conf` from the hyprlang era was removed.
    pub legacy_conf_removed: bool,
    /// The user's hyprland.lua does not reference opb yet.
    pub needs_require_line: bool,
}

/// Install/regenerate opb.lua against the active pin (idempotent — systemctl
/// enable semantics). Refuses on a non-Lua Hyprland config: sourcing a Lua
/// module from hyprlang is impossible.
pub fn enable(paths: &Paths, pin_dir: &Path) -> Result<EnableReport> {
    let hyprland_lua = paths.hyprland_lua();
    if !hyprland_lua.exists() {
        bail!(
            "{} not found — opb session wiring requires a Lua-config Hyprland \
             (hyprlang setups are unsupported)",
            hyprland_lua.display()
        );
    }
    atomic::write(
        &paths.opb_lua(),
        render(pin_dir, &paths.current_dir(), &paths.keys_lua()).as_bytes(),
    )?;

    let legacy_conf_removed = remove_legacy_conf(paths)?;
    let needs_require_line = !std::fs::read_to_string(&hyprland_lua)
        .map(|src| already_required(&src))
        .unwrap_or(false);

    Ok(EnableReport {
        legacy_conf_removed,
        needs_require_line,
    })
}

/// Remove opb-managed wiring. Only deletes files carrying our marker; the
/// user's hyprland.lua (their require line) and keys.lua are never touched.
/// Returns whether an opb.lua was removed.
pub fn disable(paths: &Paths) -> Result<bool> {
    remove_legacy_conf(paths)?;
    let opb_lua = paths.opb_lua();
    if !opb_lua.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&opb_lua)
        .with_context(|| format!("remove {}", opb_lua.display()))?;
    Ok(true)
}

/// Delete the dead hyprlang-era block if (and only if) we still own it.
fn remove_legacy_conf(paths: &Paths) -> Result<bool> {
    let legacy = paths.legacy_opb_conf();
    if !legacy.exists() {
        return Ok(false);
    }
    let owned = std::fs::read_to_string(&legacy)
        .map(|src| is_managed(&src))
        .unwrap_or(false);
    if !owned {
        return Ok(false);
    }
    std::fs::remove_file(&legacy)
        .with_context(|| format!("remove stale {}", legacy.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn paths(home: &Path) -> Paths {
        Paths::new(home.to_path_buf())
    }

    #[test]
    fn render_targets_native_api_only_and_loads_keys() {
        let s = render(
            Path::new("/h/.local/share/opb/upstream/omarchy@abc"),
            Path::new("/h/.local/share/opb/upstream/current"),
            Path::new("/h/.config/opb/keys.lua"),
        );
        assert!(s.contains("local OPB_PIN = [[/h/.local/share/opb/upstream/omarchy@abc]]"));
        assert!(s.contains("local OPB_KEYS = [[/h/.config/opb/keys.lua]]"));
        assert!(s.contains("hl.on(\"hyprland.start\""));
        assert!(s.contains("quickshell -p "));
        assert!(s.contains("dofile(OPB_KEYS)"));
        // Autostart + keybind loading only — zero binds of its own (D11),
        // and no dependency on omarchy helper functions.
        assert!(!s.contains("hl.bind"));
        assert!(!s.contains("o.bind"));
        assert!(s.contains(require_hint()));
    }

    #[test]
    fn already_required_detects_activation_forms() {
        assert!(already_required("require(\"opb\")"));
        assert!(already_required("  require('opb') -- wiring"));
        assert!(already_required("local opb = require \"opb\""));
        assert!(already_required("dofile(\"/home/u/.config/hypr/opb.lua\")"));
        assert!(!already_required("require(\"binds\")"));
        assert!(!already_required("-- opb TODO later"));
        assert!(!already_required(""));
    }

    #[test]
    fn is_managed_checks_marker() {
        assert!(is_managed("# Managed by opb — regenerate…\nenv = X\n"));
        assert!(is_managed("\n\n# Managed by opb\nrest"));
        assert!(!is_managed("# my own config\n"));
        assert!(!is_managed(""));
    }

    #[test]
    fn enable_wires_and_reports_missing_require_line() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        fs::create_dir_all(p.hypr_config_dir()).unwrap();
        fs::write(p.hyprland_lua(), "require(\"binds\")\n").unwrap();

        let report = enable(&p, Path::new("/pins/omarchy@abc")).unwrap();

        assert!(paths_opb_lua_exists(&p));
        assert!(report.needs_require_line);
        assert!(!report.legacy_conf_removed);
    }

    #[test]
    fn enable_is_idempotent_and_detects_existing_require_line() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        fs::create_dir_all(p.hypr_config_dir()).unwrap();
        fs::write(p.hyprland_lua(), "require(\"opb\")\n").unwrap();

        enable(&p, Path::new("/pins/a")).unwrap();
        let report = enable(&p, Path::new("/pins/b")).unwrap();

        assert!(!report.needs_require_line);
        let body = fs::read_to_string(p.opb_lua()).unwrap();
        assert!(body.contains("/pins/b"), "regenerated against active pin");
    }

    #[test]
    fn enable_refuses_without_hyprland_lua() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        assert!(enable(&p, Path::new("/pins/a")).is_err());
        assert!(!paths_opb_lua_exists(&p));
    }

    #[test]
    fn enable_removes_only_owned_legacy_conf() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        fs::create_dir_all(p.hypr_config_dir()).unwrap();
        fs::write(p.hyprland_lua(), "").unwrap();

        // Owned artifact → removed.
        fs::write(p.legacy_opb_conf(), "# Managed by opb — regenerate.\nenv = X\n").unwrap();
        let report = enable(&p, Path::new("/pins/a")).unwrap();
        assert!(report.legacy_conf_removed);
        assert!(!p.legacy_opb_conf().exists());

        // User-touched artifact → kept.
        fs::write(p.legacy_opb_conf(), "# my tweaks\n").unwrap();
        let report = enable(&p, Path::new("/pins/a")).unwrap();
        assert!(!report.legacy_conf_removed);
        assert!(p.legacy_opb_conf().exists());
    }

    #[test]
    fn disable_removes_wiring_but_never_keys() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        fs::create_dir_all(p.hypr_config_dir()).unwrap();
        fs::write(p.hyprland_lua(), "").unwrap();
        enable(&p, Path::new("/pins/a")).unwrap();
        fs::create_dir_all(p.opb_config_dir()).unwrap();
        fs::write(p.keys_lua(), "-- mine\n").unwrap();

        assert!(disable(&p).unwrap());
        assert!(!p.opb_lua().exists());
        assert!(p.keys_lua().exists(), "user-owned binds survive");

        // Second run: nothing left to remove, still a success.
        assert!(!disable(&p).unwrap());
    }

    #[test]
    fn require_block_is_self_describing_and_detectable() {
        let block = require_block();
        assert!(block.contains(REQUIRE_BEGIN));
        assert!(block.contains(REQUIRE_END));
        assert!(already_required(&block));
    }

    #[test]
    fn strip_require_blocks_removes_only_marked_lines() {
        let src = "local a = 1\n\n\
                   -- BEGIN opb (managed by `opb enable`; remove with `opb disable`)\n\
                   require(\"opb\")\n\
                   -- END opb\n\
                   local b = 2\n";
        let (cleaned, removed) = strip_require_blocks(src);
        assert!(removed);
        assert_eq!(cleaned, "local a = 1\n\nlocal b = 2\n");
        assert!(!already_required(&cleaned));
    }

    #[test]
    fn strip_is_noop_without_block_and_tolerates_unclosed_block() {
        let plain = "require(\"binds\")\n";
        let (out, removed) = strip_require_blocks(plain);
        assert!(!removed);
        assert_eq!(out, plain);

        // A BEGIN without END swallows to EOF — still only our lines at risk.
        let broken = "x = 1\n-- BEGIN opb (managed by `opb enable`; remove with `opb disable`)\nrequire(\"opb\")\n";
        let (out, removed) = strip_require_blocks(broken);
        assert!(removed);
        assert_eq!(out, "x = 1\n");
    }

    #[test]
    fn write_then_remove_round_trips_hyprland_lua() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        fs::create_dir_all(p.hypr_config_dir()).unwrap();
        fs::write(p.hyprland_lua(), "require(\"binds\")\n").unwrap();

        assert!(write_require_block(&p).unwrap());
        let with_block = fs::read_to_string(p.hyprland_lua()).unwrap();
        assert!(with_block.contains("require(\"opb\")"));
        assert!(with_block.starts_with("require(\"binds\")\n"));

        // Idempotent: second write is a no-op.
        assert!(!write_require_block(&p).unwrap());

        // Hand-written activation also counts as present.
        let hand = "require('opb')\n";
        fs::write(p.hyprland_lua(), hand).unwrap();
        assert!(!write_require_block(&p).unwrap());
        // …and removal strips nothing — only marker-scoped content is ours.
        assert!(!remove_require_block(&p).unwrap());
        assert_eq!(fs::read_to_string(p.hyprland_lua()).unwrap(), hand);

        // Removal strips exactly our block, preserving the rest.
        fs::write(p.hyprland_lua(), &with_block).unwrap();
        assert!(remove_require_block(&p).unwrap());
        assert!(!remove_require_block(&p).unwrap());
        assert_eq!(
            fs::read_to_string(p.hyprland_lua()).unwrap(),
            "require(\"binds\")\n"
        );
    }

    fn paths_opb_lua_exists(p: &Paths) -> bool {
        p.opb_lua().exists()
    }
}
