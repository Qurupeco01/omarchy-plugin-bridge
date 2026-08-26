//! Keybinds — the action catalog (`opb keys list`), the interactive editor
//! (`opb keys edit`), the non-interactive writer (`opb keys set`), and live
//! registration (`opb up` / `opb down`).
//!
//! The catalog is **derived at runtime from the pin** (ROADMAP C2): parse
//! upstream's `default/hypr/bindings/*.lua` for shell-facing `o.bind` calls,
//! then add derived toggle entries for toggleable components that upstream
//! never bound. Nothing here is a static table — pin bumps re-derive
//! automatically, and the bindings lua paths are contract-watch sentinels.
//!
//! Bind state comes from keys.lua, not the component x-ray: each action is
//! unbound (—), bound to the upstream suggestion (✓), or customized (✎).
//! Component enable-state only gates what the list/editor offer.
//!
//! keys.lua is the single source of truth. Binds are registered into the live
//! Hyprland session and re-registered after every `hyprctl reload` by an
//! internal keeper process (spawned by `opb up` / the boot autostart via an
//! env-var re-exec — it is not a command) — so they survive reloads without
//! any config write from `up`. `opb enable` wires boot autostart; binds never
//! depend on enable.

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;

use crate::paths::Paths;

// --- upstream binding parser -------------------------------------------------

/// One parsed `o.bind("COMBO", "DESC", "CMD"[, opts])` line.
#[derive(Debug, Clone, PartialEq)]
pub struct UpstreamBind {
    pub combo: String,
    pub desc: String,
    pub cmd: String,
}

/// Parse single-line `o.bind(...)` calls with three string arguments.
///
/// Deliberately skipped (they carry no static action):
/// - multiline calls (upstream's loop-generated `Bar panel N` binds),
/// - function dispatchers (`function() … end`) — pure compositor concerns,
/// - compound commands (shell metacharacters) — not a single IPC invocation.
pub fn parse_binds(lua_src: &str) -> Vec<UpstreamBind> {
    let mut out = Vec::new();
    for line in lua_src.lines() {
        let t = line.trim();
        let Some(inner) = t.strip_prefix("o.bind(") else {
            continue;
        };
        // Multiline call (e.g. string-concatenated combos) — not statically
        // parsable, and its actions are covered by derived toggles.
        if !inner.ends_with(')') {
            continue;
        }
        let inner = &inner[..inner.len() - 1];
        let Some((combo, rest)) = split_quoted(inner) else {
            continue;
        };
        let Some((desc, rest)) = split_quoted(rest) else {
            continue;
        };
        let Some((cmd, tail)) = split_quoted(rest) else {
            continue;
        };
        // Optional fourth arg must be an options table; anything else
        // (dispatchers as strings are already consumed, functions rejected).
        let tail_ok = tail.trim().is_empty()
            || tail.trim().starts_with('{') && tail.trim().ends_with('}');
        if !tail_ok || cmd.contains("||") || cmd.contains("&&") || cmd.contains(';') {
            continue;
        }
        out.push(UpstreamBind {
            combo,
            desc,
            cmd,
        });
    }
    out
}

/// Split one leading `"…"` argument off `s`, returning (content, remainder
/// after the separating comma). `None` when s doesn't start with a complete
/// quoted string. No escape handling — upstream's table contains none.
fn split_quoted(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    let rest = s.strip_prefix('"')?;
    let end = rest.find('"')?;
    let content = rest[..end].to_owned();
    let after = rest[end + 1..].trim_start();
    let after = after.strip_prefix(',').unwrap_or(after);
    Some((content, after))
}

// --- action derivation --------------------------------------------------------

/// A bindable shell action. `derived` marks entries opb inferred (no upstream
/// bind exists); they have no suggested combo.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub id: String,
    pub description: String,
    /// Component id used for enable-state gating (§3.4 rules).
    pub plugin: String,
    /// Full exec command (pre env-wrap; keys.rs C3 wraps at write time).
    pub invocation: String,
    pub suggested_combo: Option<String>,
    pub derived: bool,
}

/// Pure: bind list → deduplicated shell-facing actions. First occurrence of
/// an id wins (upstream sometimes binds one action under two combos).
pub fn actions_from_binds(binds: &[UpstreamBind]) -> Vec<Action> {
    let mut out: Vec<Action> = Vec::new();
    for b in binds {
        let Some(action) = action_from_cmd(&b.combo, &b.desc, &b.cmd) else {
            continue;
        };
        if !out.iter().any(|a| a.id == action.id) {
            out.push(action);
        }
    }
    out
}

/// Pure: classify one command. Only plugin interactions belong in the catalog
/// (CONCEPT §4 Keybind model): WM/app binds stay Hyprland's.
fn action_from_cmd(combo: &str, desc: &str, cmd: &str) -> Option<Action> {
    let mut tokens = cmd.split_whitespace().peekable();
    match tokens.next()? {
        "omarchy-shell" => {
            // Skip flags like -q between the binary and its arguments.
            while tokens.peek().is_some_and(|t| t.starts_with('-')) {
                tokens.next();
            }
            match tokens.next()? {
                "shell" => match (tokens.next()?, tokens.next()?) {
                    ("toggle", component) => Some(Action {
                        id: format!("{component}:toggle"),
                        description: desc.to_owned(),
                        plugin: component.to_owned(),
                        invocation: cmd.to_owned(),
                        suggested_combo: Some(combo.to_owned()),
                        derived: false,
                    }),
                    _ => None,
                },
                target @ ("notifications" | "media") => {
                    let method = tokens.next()?;
                    Some(Action {
                        id: format!("{target}:{method}"),
                        description: desc.to_owned(),
                        plugin: format!("omarchy.{target}"),
                        invocation: cmd.to_owned(),
                        suggested_combo: Some(combo.to_owned()),
                        derived: false,
                    })
                }
                _ => None,
            }
        }
        "omarchy-menu" => {
            let sub = tokens.next()?;
            if sub != "toggle" {
                return None;
            }
            let name = tokens.next();
            Some(Action {
                id: match name {
                    Some(n) => format!("menu:{n}"),
                    None => "menu".to_owned(),
                },
                description: desc.to_owned(),
                plugin: "omarchy.menu".to_owned(),
                invocation: cmd.to_owned(),
                suggested_combo: Some(combo.to_owned()),
                derived: false,
            })
        }
        _ => None,
    }
}

/// Short human label from a component id: `omarchy.tailscale` → `tailscale`.
fn short_name(id: &str) -> &str {
    id.rsplit('.').next().unwrap_or(id)
}

// --- catalog ------------------------------------------------------------------

/// One rendered-catalog row: an action plus live gating info.
pub struct Entry {
    pub action: Action,
    pub state: &'static str,
}

/// Build the full catalog: upstream-declared actions × derived toggle
/// possibilities, each gated against the generated shell.json (read-only,
/// headless). Errors only when shell.json is missing/unparseable.
pub fn catalog(paths: &Paths) -> Result<Vec<Entry>> {
    let doc = crate::plugin_list::load_doc(paths)?;
    let pin_dir = crate::pin::active_dir(paths)?;
    let manifests: BTreeMap<String, Vec<String>> =
        crate::shelljson::scan_manifests(&pin_dir.join("shell/plugins"))?
            .into_iter()
            .map(|info| (info.id, info.kinds))
            .collect();

    let mut actions = {
        let mut src = String::new();
        let bindings_dir = pin_dir.join("default/hypr/bindings");
        let mut files: Vec<_> = std::fs::read_dir(&bindings_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "lua"))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        files.sort();
        for f in files {
            if let Ok(lua) = std::fs::read_to_string(&f) {
                src.push_str(&lua);
                src.push('\n');
            }
        }
        actions_from_binds(&parse_binds(&src))
    };

    // Derived toggle possibilities: every toggleable-surface component that
    // has no upstream bind yet. `shell toggle <id>` is generic upstream IPC
    // (shell.qml summon/hide/toggle over resolved enabled ids).
    for (id, kinds) in &manifests {
        let toggleable = kinds
            .iter()
            .any(|k| matches!(k.as_str(), "panel" | "menu" | "overlay"));
        if !toggleable || actions.iter().any(|a| &a.plugin == id) {
            continue;
        }
        actions.push(Action {
            id: format!("{id}:toggle"),
            description: format!("Toggle {} (unbound)", short_name(id)),
            plugin: id.clone(),
            invocation: format!("omarchy-shell shell toggle {id}"),
            suggested_combo: None,
            derived: true,
        });
    }

    actions.sort_by(|a, b| a.plugin.cmp(&b.plugin).then(a.id.cmp(&b.id)));

    Ok(actions
        .into_iter()
        .map(|action| {
            let kinds = manifests.get(&action.plugin).cloned().unwrap_or_default();
            let state = crate::plugin_list::state_of(&doc, &kinds, &action.plugin);
            Entry { action, state }
        })
        .collect())
}

// --- bind-state resolver ---------------------------------------------------------

/// Bind status of one action, resolved from keys.lua against upstream's
/// suggested combo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindState {
    /// No opb-managed bind in keys.lua.
    Unbound,
    /// Bound to exactly the upstream-suggested combo (accepted as-is).
    Suggested,
    /// Bound to a combo that differs from the suggestion.
    Customized,
}

/// Pure: current combo each action is bound to, from opb-marked keys.lua
/// entries. Foreign/hand-written binds are invisible here — opb only reasons
/// about what it wrote (`-- opb: <id> |` marker + following `hl.bind` line).
pub fn bound_combos(keys_lua_src: &str) -> BTreeMap<String, Combo> {
    let mut map = BTreeMap::new();
    let mut cur: Option<String> = None;
    for line in keys_lua_src.lines() {
        let t = line.trim();
        if let Some(id) = marker_id(t) {
            cur = Some(id.to_owned());
            continue;
        }
        if let Some(inner) = t.strip_prefix("hl.bind(") {
            let first = inner.split(',').next().unwrap_or("").trim();
            let Some(unquoted) = first.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
                cur = None;
                continue;
            };
            if let Some(id) = &cur
                && let Ok(c) = parse_combo(unquoted)
            {
                map.insert(id.clone(), c);
            }
            cur = None;
        }
    }
    map
}

/// Pure: the action id from an opb marker comment (`-- opb: <id> | <desc>`).
fn marker_id(t: &str) -> Option<&str> {
    let rest = t.strip_prefix("-- opb: ")?;
    let end = rest.find(" |")?;
    Some(&rest[..end])
}

/// Pure: classify one action's bind status.
pub fn bind_state(action: &Action, bound: Option<&Combo>) -> BindState {
    match bound {
        None => BindState::Unbound,
        Some(combo) => {
            let suggested = action
                .suggested_combo
                .as_deref()
                .and_then(|s| parse_combo(s).ok());
            match suggested {
                Some(s) if &s == combo => BindState::Suggested,
                // Derived actions have no suggestion — any bind is custom.
                _ => BindState::Customized,
            }
        }
    }
}

/// Pure: the combo to display for an action — its live bind if any, else the
/// upstream suggestion, else a placeholder.
fn display_combo(action: &Action, bound: Option<&Combo>) -> String {
    if let Some(c) = bound {
        return c.to_lua_string();
    }
    action.suggested_combo.clone().unwrap_or_else(|| "-".to_owned())
}

/// Icon for a bind state.
fn bind_icon(state: BindState) -> &'static str {
    match state {
        BindState::Unbound => "—",
        BindState::Suggested => "✓",
        BindState::Customized => "✎",
    }
}

// --- rendering ------------------------------------------------------------------

/// Render rows honoring filters: default hides disabled components; `all`
/// includes them; `plugin` narrows to exact component id(s). The BIND column
/// is per-action state from keys.lua (— / ✓ / ✎), not component enable state.
pub fn render(entries: &[Entry], keys_lua_src: &str, all: bool, plugin: Option<&str>) -> String {
    let combos = bound_combos(keys_lua_src);
    let mut out = String::new();
    let width_id = entries
        .iter()
        .map(|e| e.action.id.len())
        .max()
        .unwrap_or(5)
        .max(5);
    let header = format!(
        "{:<4} {:<id_w$}  {:<14}  {}",
        "BIND",
        "ACTION",
        "COMBO",
        "DESCRIPTION",
        id_w = width_id,
    );
    out.push_str(&header);
    out.push('\n');
    for e in entries {
        if !all && e.state == "off" {
            continue;
        }
        if plugin.is_some_and(|p| e.action.plugin != p && e.action.id != p) {
            continue;
        }
        let bound = combos.get(&e.action.id);
        let state = bind_state(&e.action, bound);
        let combo = display_combo(&e.action, bound);
        let mut desc = e.action.description.clone();
        if e.action.derived {
            desc.push_str(" [derived]");
        }
        if e.state == "off" {
            desc.push_str(" [component off]");
        }
        out.push_str(&format!(
            "{:<4} {:<id_w$}  {:<14}  {}\n",
            bind_icon(state),
            e.action.id,
            truncate_pad(&combo, 14),
            desc,
            id_w = width_id,
        ));
    }
    out
}

fn truncate_pad(s: &str, w: usize) -> String {
    if s.chars().count() > w {
        s.chars().take(w.saturating_sub(1)).collect::<String>() + "…"
    } else {
        format!("{:<w$}", s, w = w)
    }
}

// --- collision engine (C3) ----------------------------------------------------
//
// `hyprctl -j binds` reports modmask bits (verified live @ Hyprland 0.56):
// shift=1, ctrl=4, alt=8, super=64; Lua-bound combos surface as opaque
// `__lua` dispatchers — we see occupancy, never intent. That is enough:
// an occupied combo shadows by definition order, whoever owns it.

pub const MOD_SHIFT: u32 = 1;
pub const MOD_CTRL: u32 = 4;
pub const MOD_ALT: u32 = 8;
pub const MOD_SUPER: u32 = 64;

/// One bind as Hyprland reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveBind {
    pub modmask: u32,
    pub key: String,
    pub keycode: u32,
}

impl LiveBind {
    fn describe(&self) -> String {
        let mut mods = Vec::new();
        if self.modmask & MOD_SUPER != 0 {
            mods.push("SUPER");
        }
        if self.modmask & MOD_CTRL != 0 {
            mods.push("CTRL");
        }
        if self.modmask & MOD_ALT != 0 {
            mods.push("ALT");
        }
        if self.modmask & MOD_SHIFT != 0 {
            mods.push("SHIFT");
        }
        let key = if self.keycode > 0 {
            format!("code:{}", self.keycode)
        } else {
            self.key.clone()
        };
        mods.push(&key);
        mods.join(" + ")
    }
}

/// A normalized combo: modmask + key (either a keysym string or `code:N`).
#[derive(Debug, Clone, PartialEq)]
pub struct Combo {
    pub modmask: u32,
    /// Uppercased keysym, or `code:N` preserved verbatim.
    pub key: String,
}

/// Parse an upstream-style combo ("SUPER + CTRL + E", "XF86AudioPlay",
/// "SUPER + SHIFT + code:201"). Structural validation only — unknown
/// non-mod tokens become the key; exactly one key token is required.
pub fn parse_combo(input: &str) -> Result<Combo> {
    let mut mask = 0u32;
    let mut key: Option<String> = None;
    for token in input.split('+').map(str::trim).filter(|t| !t.is_empty()) {
        match token.to_ascii_uppercase().as_str() {
            "SUPER" | "MOD" => mask |= MOD_SUPER,
            "CTRL" | "CONTROL" => mask |= MOD_CTRL,
            "ALT" => mask |= MOD_ALT,
            "SHIFT" => mask |= MOD_SHIFT,
            _ if key.is_none() => key = Some(token.to_owned()),
            _ => anyhow::bail!("combo {input:?}: more than one key token"),
        }
    }
    let Some(key) = key else {
        anyhow::bail!("combo {input:?}: no key");
    };
    Ok(Combo {
        modmask: mask,
        key: normalize_key(&key),
    })
}

fn normalize_key(key: &str) -> String {
    // `code:N` keeps its case-sensitive form; everything else compares
    // case-insensitively (hyprctl reports arrows lowercase, letters uppercase).
    if let Some(n) = key.strip_prefix("code:") {
        format!("code:{n}")
    } else {
        key.to_ascii_uppercase()
    }
}

impl Combo {
    pub fn to_lua_string(&self) -> String {
        let mut parts = Vec::new();
        if self.modmask & MOD_SUPER != 0 {
            parts.push("SUPER");
        }
        if self.modmask & MOD_CTRL != 0 {
            parts.push("CTRL");
        }
        if self.modmask & MOD_ALT != 0 {
            parts.push("ALT");
        }
        if self.modmask & MOD_SHIFT != 0 {
            parts.push("SHIFT");
        }
        // Restore the user's original casing is unnecessary for hl.bind —
        // upstream feeds the same uppercase style.
        parts.push(self.key.as_str());
        parts.join(" + ")
    }

    fn matches_live(&self, b: &LiveBind) -> bool {
        if b.keycode > 0 {
            return self.modmask == b.modmask && self.key == format!("code:{}", b.keycode);
        }
        self.modmask == b.modmask && self.key == b.key.to_ascii_uppercase()
    }
}

/// Pure: first live bind colliding with `combo`, ignoring an exempt combo
/// (the action's own combo being replaced). Returns a human-readable
/// description of the first hit.
pub fn collision_excluding(
    combo: &Combo,
    live: &[LiveBind],
    exempt: Option<&Combo>,
) -> Option<String> {
    live.iter()
        .find(|b| combo.matches_live(b) && !exempt.is_some_and(|e| e.matches_live(b)))
        .map(LiveBind::describe)
}

/// Read the session's binds. `Ok(None)` when hyprctl is unavailable or errors
/// — callers degrade gracefully (nothing detected, nothing blocked), matching
/// the plugin enable pre-flight.
pub fn live_binds() -> Option<Vec<LiveBind>> {
    let out = std::process::Command::new("hyprctl")
        .arg("-j")
        .arg("binds")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    Some(
        v.as_array()?
            .iter()
            .map(|b| LiveBind {
                modmask: b["modmask"].as_u64().unwrap_or(0) as u32,
                key: b["key"].as_str().unwrap_or_default().to_owned(),
                keycode: b["keycode"].as_u64().unwrap_or(0) as u32,
            })
            .collect(),
    )
}

// --- writer (C3): keys.lua -----------------------------------------------------

/// Render one bind entry. Native-API only (`hl.bind` + `hl.dsp.exec_cmd`),
/// env-wrapped exec so the entry is self-contained (CONCEPT §4).
///
/// The command travels in a Lua **long-bracket string** (`[[…]]`): the shell
/// fragment legitimately contains both quote flavors (double quotes around
/// env values inside single-quoted `sh -c`), which would need layered
/// escaping in a quoted Lua string. Falls back to an escaped quoted string
/// if the fragment ever contained `]]`.
pub fn render_entry(pin_dir: &std::path::Path, action: &Action, combo: &Combo) -> String {
    let inner = format!(
        "{} exec {}",
        crate::env::shell_exports(pin_dir),
        action.invocation,
    );
    let cmd = format!("sh -c '{}'", inner);
    let cmd = if cmd.contains("]]") {
        format!("\"{}\"", cmd.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("[[{cmd}]]")
    };
    format!(
        "-- opb: {id} | {desc}\nhl.bind(\"{combo}\", hl.dsp.exec_cmd({cmd}), {{ description = \"{desc}\" }})\n",
        id = action.id,
        desc = lua_escape(&action.description),
        // The combo MUST be a Lua string — unquoted, `SUPER + F1` would be
        // parsed as arithmetic on nil globals (found live via hyprctl eval).
        combo = combo.to_lua_string(),
    )
}

fn lua_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

const KEYS_HEADER: &str = "\
-- opb keybinds (user-owned). Managed with `opb keys edit` / `opb keys set`;
-- hand-edit freely. opb only ever edits its own `-- opb:`-marked entries.
-- Loaded on `opb up` and at boot via ~/.config/hypr/opb.lua (require(\"opb\")).

";

/// Pure: keys.lua source with `entry` appended, creating the header when the
/// file is empty and normalizing a missing trailing newline.
fn append_source(src: &str, entry: &str) -> String {
    if src.is_empty() {
        format!("{KEYS_HEADER}{entry}")
    } else if src.ends_with('\n') {
        format!("{src}{entry}")
    } else {
        format!("{src}\n{entry}")
    }
}

/// Pure: keys.lua source with `action_id`'s opb-marked block replaced by
/// `replacement` (a full rendered entry), or removed when empty. The block is
/// the `-- opb: <id> | …` marker line plus the `hl.bind(...)` line after it —
/// exactly what `render_entry` emits.
fn rewrite_action_block(src: &str, action_id: &str, replacement: &str) -> String {
    let marker_prefix = format!("-- opb: {action_id} |");
    let mut out: Vec<&str> = Vec::new();
    let mut lines = src.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with(&marker_prefix) {
            if lines
                .peek()
                .is_some_and(|n| n.trim_start().starts_with("hl.bind("))
            {
                lines.next();
            }
            if !replacement.is_empty() {
                out.extend(replacement.lines());
            }
            continue;
        }
        out.push(line);
    }
    let mut s = out.join("\n");
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Parse-check (when luac exists) and atomically write keys.lua.
fn write_source(paths: &Paths, next: &str) -> Result<()> {
    let path = paths.keys_lua();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    if let Some(reported) = lua_parse_error(next) {
        bail!(
            "refusing to write {}: generated entry does not parse ({reported}) — \
             this is an opb bug, please report it",
            path.display()
        );
    }
    crate::atomic::write(path.as_path(), next.as_bytes())?;
    Ok(())
}

/// `None` when the Lua source parses (or when no Lua toolchain exists to
/// check with), otherwise the parser's error line.
fn lua_parse_error(src: &str) -> Option<String> {
    use std::io::Write;
    let mut child = std::process::Command::new("luac")
        .arg("-p")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(src.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    if out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stderr).trim().to_owned())
}

/// Pure: does an already-written entry bind this same combo? Returns the
/// conflicting combo string as written in the file. Our entries emit the
/// combo as the first (quoted) `hl.bind(...)` argument.
pub fn existing_combo_conflict(keys_lua_src: &str, combo: &Combo) -> Option<String> {
    for line in keys_lua_src.lines() {
        let Some(inner) = line.trim().strip_prefix("hl.bind(") else {
            continue;
        };
        let first = inner.split(',').next().unwrap_or("").trim();
        let Some(unquoted) = first.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
            continue;
        };
        if parse_combo(unquoted).is_ok_and(|c| &c == combo) {
            return Some(unquoted.to_owned());
        }
    }
    None
}

/// Pure: is this action already bound in the file?
pub fn action_already_bound(keys_lua_src: &str, action_id: &str) -> bool {
    keys_lua_src
        .lines()
        .any(|l| l.trim() == format!("-- opb: {} |", action_id).trim_end_matches(" |"))
        || keys_lua_src.contains(&format!("-- opb: {action_id} |"))
}

/// Shared write path for `keys set` and the interactive editor: parse,
/// collision-check, duplicate-check, then atomic replace-or-append. An already
/// bound action is rebound in place (its old line is replaced, never
/// duplicated); an occupied live combo shadows only after an explicit confirm
/// — shadowing is destructive.
pub fn write_binding(
    paths: &Paths,
    action: &Action,
    combo_input: &str,
    live: Option<&[LiveBind]>,
) -> Result<()> {
    let combo = parse_combo(combo_input)?;
    let src = std::fs::read_to_string(paths.keys_lua()).unwrap_or_default();
    let already = action_already_bound(&src, &action.id);
    // The combo being replaced is exempt from live-occupancy checks — it is
    // this action's own current bind, removed by the rewrite below.
    let own_current = if already {
        bound_combos(&src).get(&action.id).cloned()
    } else {
        None
    };

    let entry = render_entry(&paths.current_dir(), action, &combo);
    let next = if already {
        rewrite_action_block(&src, &action.id, &entry)
    } else {
        append_source(&src, &entry)
    };

    // Duplicate combo in keys.lua for another action — checked with this
    // action's block removed, so its own old/new lines can't false-hit.
    let others = if already {
        rewrite_action_block(&src, &action.id, "")
    } else {
        src.clone()
    };
    if let Some(written) = existing_combo_conflict(&others, &combo) {
        anyhow::bail!(
            "combo already used in {} for another action ({written})",
            paths.keys_lua().display()
        );
    }

    match live.map(|l| collision_excluding(&combo, l, own_current.as_ref())) {
        None => println!("note: hyprctl unavailable — skipping live collision check"),
        Some(None) => {}
        Some(Some(hit)) => {
            println!(
                "WARNING: combo {} is already bound ({hit}) — your new bind \
                 shadows it by definition order",
                combo.to_lua_string()
            );
            if !crate::prompt::confirm("Bind anyway (your bind wins)?", false) {
                anyhow::bail!("occupied combo — nothing written");
            }
        }
    }

    write_source(paths, &next)?;
    Ok(())
}

/// Remove an action's opb-marked block from keys.lua (unbind).
pub fn remove_binding(paths: &Paths, action_id: &str) -> Result<()> {
    let src = std::fs::read_to_string(paths.keys_lua()).unwrap_or_default();
    let next = rewrite_action_block(&src, action_id, "");
    if next == src {
        anyhow::bail!("{} has no opb-managed bind to remove", action_id);
    }
    write_source(paths, &next)
}

/// `opb keys set <action> <combo>` — the only non-interactive bind writer.
/// Rebinds in place when the action is already bound; applies live when the
/// shell is running.
pub fn set(paths: &Paths, action_id: &str, combo_input: &str) -> Result<()> {
    let entries = catalog(paths)?;
    let entry = entries
        .iter()
        .find(|e| e.action.id == action_id)
        .with_context(|| format!("unknown action {action_id:?} — see `opb keys list [--all]`"))?;

    if entry.state == "off" {
        println!(
            "note: component {} is disabled — the bind will no-op until you enable it",
            entry.action.plugin
        );
    }

    let live = live_binds();
    write_binding(paths, &entry.action, combo_input, live.as_deref())?;
    println!(
        "bound {} → {}",
        parse_combo(combo_input)?.to_lua_string(),
        action_id
    );
    println!("  wrote {}", paths.keys_lua().display());
    apply_after_write(paths);
    Ok(())
}

// --- interactive editor (`opb keys edit`) ---------------------------------------

/// Pure: actions worth offering interactively — enabled components only
/// (the ones relevant to a running system; binding ahead of enable no-ops).
pub fn enabled_entries(entries: &[Entry]) -> Vec<&Entry> {
    entries.iter().filter(|e| e.state != "off").collect()
}

/// `opb keys edit` — one-list editor over every bindable action. Enter edits
/// a combo (pre-filled with the row's current bind, or the upstream suggestion
/// when unbound); empty input unbinds; esc on the list exits. The list
/// re-renders with live bind state after each action. Occupied combos shadow
/// only after an explicit confirm.
pub fn edit(paths: &Paths) -> Result<()> {
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        bail!("interactive keybinding needs a terminal — use `keys list` / `keys set` instead");
    }
    let theme = dialoguer::theme::ColorfulTheme::default();
    loop {
        let entries = catalog(paths)?;
        let existing = std::fs::read_to_string(paths.keys_lua()).unwrap_or_default();
        let combos = bound_combos(&existing);
        let rows = enabled_entries(&entries);
        if rows.is_empty() {
            println!("no bindable actions — every relevant component is off (see `opb plugin list`)");
            return Ok(());
        }

        let labels: Vec<String> = rows
            .iter()
            .map(|e| {
                let bound = combos.get(&e.action.id);
                let state = bind_state(&e.action, bound);
                let combo = display_combo(&e.action, bound);
                format!(
                    "{:<4} {:<20} {:<14} {}",
                    bind_icon(state),
                    e.action.id,
                    truncate_pad(&combo, 14),
                    e.action.description,
                )
            })
            .collect();
        let picked = match dialoguer::Select::with_theme(&theme)
            .with_prompt("Which action (enter=edit · esc quits)")
            .items(&labels)
            .interact_opt()
        {
            Err(dialoguer::Error::IO(e)) if e.kind() == std::io::ErrorKind::Interrupted => {
                return Ok(())
            }
            Err(e) => return Err(anyhow::anyhow!("selection failed: {e}")),
            Ok(None) => return Ok(()),
            Ok(Some(i)) => i,
        };

        let entry = rows[picked];
        let bound = combos.get(&entry.action.id).cloned();
        let action_id = entry.action.id.clone();
        let action = entry.action.clone();
        match ask_combo(&theme, &action, bound.as_ref())? {
            None => continue, // esc on the input — no change, back to the list
            Some(None) => {
                match remove_binding(paths, &action_id) {
                    Ok(()) => {
                        println!("unbound {action_id}");
                        apply_after_write(paths);
                    }
                    Err(e) => println!("skipped {action_id}: {:#}", e),
                }
            }
            Some(Some(text)) => {
                let live = live_binds();
                match write_binding(paths, &action, &text, live.as_deref()) {
                    Ok(()) => {
                        println!(
                            "bound {} → {}",
                            parse_combo(&text)?.to_lua_string(),
                            action_id
                        );
                        apply_after_write(paths);
                    }
                    // A declined shadow or a duplicate stays per-action; real
                    // I/O errors surface the same way and the flow moves on.
                    Err(e) => println!("skipped {action_id}: {:#}", e),
                }
            }
        }
        // Loop re-renders the list with the fresh bind state.
    }
}

/// Combo editor for one action. Returns:
/// - `Ok(None)` — esc (no change, back to the list)
/// - `Ok(Some(None))` — empty input → unbind
/// - `Ok(Some(Some(text)))` — accepted combo
fn ask_combo(
    theme: &dialoguer::theme::ColorfulTheme,
    action: &Action,
    bound: Option<&Combo>,
) -> Result<Option<Option<String>>> {
    loop {
        let initial = bound
            .map(|c| c.to_lua_string())
            .or_else(|| action.suggested_combo.clone())
            .unwrap_or_default();
        let input = dialoguer::Input::<String>::with_theme(theme)
            .with_prompt(format!(
                "Combo for {} (enter=accept · empty=unbind · esc=cancel)",
                action.id
            ))
            .with_initial_text(initial)
            // Without this, dialoguer rejects an empty submission and
            // re-prompts — clearing the field would never unbind.
            .allow_empty(true);
        match input.interact_text() {
            Ok(text) => {
                let text = text.trim().to_owned();
                if text.is_empty() {
                    return Ok(Some(None));
                }
                if parse_combo(&text).is_ok() {
                    return Ok(Some(Some(text)));
                }
                println!("  invalid combo — upstream style, e.g. SUPER + CTRL + E or XF86AudioPlay");
            }
            Err(dialoguer::Error::IO(e)) if e.kind() == std::io::ErrorKind::Interrupted => {
                return Ok(None)
            }
            Err(e) => return Err(anyhow::anyhow!("combo input failed: {e}")),
        }
    }
}

// --- live registration + watch daemon (`opb up` / `opb down` / `opb keys watch`)
//
// Binds belong to the shell lifecycle, not to enable: `opb up` registers
// keys.lua into the live Hyprland session, `opb down` unregisters it.
// `opb enable` only wires boot autostart.
//
// Registration uses `hyprctl eval`, which dies on the next `hyprctl reload`.
// Survival comes from `opb keys watch` — a tiny daemon spawned by `opb up`
// (and by the boot autostart) that listens on Hyprland's events socket and
// re-registers keys.lua after every `configreloaded` event. The process is
// the mechanism: alive → binds stay applied; gone → binds die on reload
// (which is what `down` means anyway). No config write from `up`.

/// Register every keys.lua bind into the running Hyprland session by
/// evaluating the file — the same source opb.lua's old dofile loaded at boot.
/// Idempotent: hl.bind re-defines an occupied combo (overwrite). Returns the
/// number of binds applied.
///
/// The code is prepended with a newline: hyprctl's arg parser treats a leading
/// `--` (the Lua comments) as a flag terminator and prints usage instead of
/// evaluating (found live).
pub fn apply_live(paths: &Paths) -> Result<usize> {
    let src = std::fs::read_to_string(paths.keys_lua()).unwrap_or_default();
    let count = src.matches("-- opb: ").count();
    if count == 0 {
        return Ok(0);
    }
    let out = std::process::Command::new("hyprctl")
        .arg("eval")
        .arg(format!("\n{src}"))
        .output()
        .context("spawn hyprctl eval")?;
    if !out.status.success() {
        anyhow::bail!(
            "hyprctl eval failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(count)
}

/// Unregister opb's binds via `hl.unbind` per keys.lua combo. Only combos opb
/// wrote are touched; foreign binds are never removed. Best-effort per bind.
pub fn clear_live(paths: &Paths) -> Result<()> {
    let src = std::fs::read_to_string(paths.keys_lua()).unwrap_or_default();
    let combos = bound_combos(&src);
    if combos.is_empty() {
        return Ok(());
    }
    let mut removed = 0usize;
    for (id, combo) in &combos {
        let code = format!("hl.unbind(\"{}\")", combo.to_lua_string().replace('"', "\\\""));
        match std::process::Command::new("hyprctl")
            .arg("eval")
            .arg(&code)
            .output()
        {
            Ok(o) if o.status.success() => removed += 1,
            Ok(o) => eprintln!(
                "opb down: hyprctl unbind {id} failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ),
            Err(e) => eprintln!("opb down: spawn hyprctl unbind: {e}"),
        }
    }
    println!("  removed {removed} opb bind(s)");
    Ok(())
}

/// Path of the Hyprland instance events socket (`.socket2.sock`).
fn events_socket() -> Option<std::path::PathBuf> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let runtime = std::env::var("XDG_RUNTIME_DIR").ok()?;
    Some(std::path::Path::new(&runtime).join("hypr").join(sig).join(".socket2.sock"))
}

/// `opb` keeper entry (re-exec'd by `opb up` with `OPB_WATCH_DAEMON=1`; not a
/// command) — register keys.lua, then re-register after every
/// `configreloaded` event so binds survive `hyprctl reload`. Exits when
/// Hyprland's socket closes (session ended).
pub fn watch(paths: &Paths) -> Result<()> {
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixStream;
    // Guard: never let a second keeper run (a second registrar would
    // double-bind on reload — hl.bind adds, it does not overwrite). A stale
    // pid file from a dead keeper is simply ignored.
    if let Ok(pid_str) = std::fs::read_to_string(paths.watch_pid())
        && let Ok(pid) = pid_str.trim().parse::<u32>()
        && pid != std::process::id()
        && std::path::Path::new(&format!("/proc/{pid}")).exists()
    {
        return Ok(());
    }
    let sock = events_socket()
        .context("HYPRLAND_INSTANCE_SIGNATURE unset — not a Hyprland session?")?;
    let stream = UnixStream::connect(&sock)
        .with_context(|| format!("connect {}", sock.display()))?;
    if let Err(e) = apply_live(paths) {
        eprintln!("opb keeper: initial register failed: {e:#}");
    }
    for line in BufReader::new(stream).lines() {
        let line = line?;
        if line.starts_with("configreloaded")
            && let Err(e) = apply_live(paths)
        {
            eprintln!("opb keeper: re-register after reload failed: {e:#}");
        }
    }
    Ok(()) // EOF — the Hyprland session is gone
}

/// Spawn the detached keeper (re-exec of this binary with `OPB_WATCH_DAEMON`,
/// not a subcommand) and record its pid. Returns whether a new keeper was
/// started (`false` = one is already alive).
pub fn spawn_watch(paths: &Paths) -> Result<bool> {
    if watch_alive(paths) {
        return Ok(false);
    }
    let exe = std::env::current_exe().context("resolve opb binary path")?;
    let child = std::process::Command::new(&exe)
        .env("OPB_WATCH_DAEMON", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("spawn keybind keeper")?;
    let pid_path = paths.watch_pid();
    if let Some(parent) = pid_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    crate::atomic::write(pid_path.as_path(), child.id().to_string().as_bytes())?;
    Ok(true)
}

/// Stop the watch daemon (TERM) and drop its pid file. Best-effort; a stale
/// pid file is simply cleaned up.
pub fn stop_watch(paths: &Paths) {
    let pid_path = paths.watch_pid();
    let Ok(pid_str) = std::fs::read_to_string(&pid_path) else {
        return;
    };
    let Ok(pid) = pid_str.trim().parse::<u32>() else {
        let _ = std::fs::remove_file(&pid_path);
        return;
    };
    let _ = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status();
    let _ = std::fs::remove_file(&pid_path);
}

/// `hyprctl reload config-only` — clears every eval'd bind (including stale
/// old combos an edit left behind), leaving exactly the user's config binds.
fn reload_config_only() -> Result<()> {
    let out = std::process::Command::new("hyprctl")
        .args(["reload", "config-only"])
        .output()
        .context("spawn hyprctl reload")?;
    if !out.status.success() {
        anyhow::bail!(
            "hyprctl reload failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Is the watch daemon alive? Reads its pid file and probes /proc.
pub fn watch_alive(paths: &Paths) -> bool {
    let Ok(pid_str) = std::fs::read_to_string(paths.watch_pid()) else {
        return false;
    };
    let Ok(pid) = pid_str.trim().parse::<u32>() else {
        return false;
    };
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// Apply a just-written keys.lua. `hl.bind` **adds** on a re-bind (never
/// overwrites), so the exactness comes from a reload (clears stale eval'd
/// binds) followed by a **single** re-registration:
/// - keeper running → the reload alone is enough; it re-applies keys.lua.
/// - no keeper → reload + one direct eval, and future reloads lose the binds.
fn apply_after_write(paths: &Paths) {
    if !crate::shell::is_running(paths) {
        println!("  note: shell not running — binds apply on the next `opb up`");
        return;
    }
    if let Err(e) = reload_config_only() {
        println!("  note: {e:#} — binds apply on the next `opb up`");
        return;
    }
    if watch_alive(paths) {
        println!(
            "  reloaded — the keybind keeper re-applied keys.lua \
             (binds survive `hyprctl reload`)"
        );
    } else if let Err(e) = apply_live(paths) {
        println!("  note: {e:#} — binds apply on the next `opb up`");
    } else {
        println!(
            "  reloaded — binds are live, but no keeper is running: start \
             `opb up` to make them survive `hyprctl reload`"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parses_single_line_binds_with_and_without_opts() {
        let src = r#"o.bind("SUPER + SPACE", "Omarchy menu", "omarchy-menu toggle")
o.bind("XF86AudioPlay", "Play", "omarchy-shell media playPause", { locked = true })
o.bind("SUPER+X", "Other", "some-app --flag")
"#;
        // The parser is command-agnostic — non-shell commands come through
        // here and are dropped later, at classification (action_from_cmd).
        assert_eq!(
            parse_binds(src),
            vec![
                UpstreamBind {
                    combo: "SUPER + SPACE".into(),
                    desc: "Omarchy menu".into(),
                    cmd: "omarchy-menu toggle".into(),
                },
                UpstreamBind {
                    combo: "XF86AudioPlay".into(),
                    desc: "Play".into(),
                    cmd: "omarchy-shell media playPause".into(),
                },
                UpstreamBind {
                    combo: "SUPER+X".into(),
                    desc: "Other".into(),
                    cmd: "some-app --flag".into(),
                },
            ]
        );
    }

    #[test]
    fn skips_multiline_function_and_compound_calls() {
        let src = r#"-- comment mentioning o.bind( nothing
o.bind(
    "SUPER + CTRL + code:" .. tostring(panel + 9),
    "Bar panel " .. panel,
    "omarchy-shell -q shell togglePanelAt right " .. panel
)
o.bind("SUPER + CTRL + Z", "Zoom in", function()
  hl.config({ cursor = { zoom_factor = 1 } })
end)
o.bind("ALT + PRINT", "Screenrecording", "omarchy-capture-screenrecording --stop || omarchy-menu toggle capture")
"#;
        // The multiline o.bind( opener has no closing paren on its own line…
        // but its continuation lines don't start with o.bind( either, so all
        // of these produce zero parses except… verify explicitly:
        assert!(parse_binds(src).is_empty());
    }

    #[test]
    fn classifies_shell_toggle_menu_and_component_methods() {
        let binds = |c: &str, d: &str, m: &str| {
            vec![UpstreamBind {
                combo: c.into(),
                desc: d.into(),
                cmd: m.into(),
            }]
        };
        let a = actions_from_binds(&binds("K", "Audio", "omarchy-shell shell toggle omarchy.audio"));
        assert_eq!(a[0].id, "omarchy.audio:toggle");
        assert_eq!(a[0].plugin, "omarchy.audio");

        let n = actions_from_binds(&binds(
            "C",
            "Dismiss last",
            "omarchy-shell notifications dismissOne",
        ));
        assert_eq!(n[0].id, "notifications:dismissOne");
        assert_eq!(n[0].plugin, "omarchy.notifications");

        let q = actions_from_binds(&binds("Q", "Quick", "omarchy-shell -q shell toggle omarchy.clock"));
        assert_eq!(q[0].invocation, "omarchy-shell -q shell toggle omarchy.clock");
        assert_eq!(q[0].plugin, "omarchy.clock");

        let m = actions_from_binds(&binds("S", "Apps menu", "omarchy-menu toggle apps"));
        assert_eq!(m[0].id, "menu:apps");
        assert_eq!(m[0].plugin, "omarchy.menu");

        let mb = actions_from_binds(&binds("M", "Root menu", "omarchy-menu toggle"));
        assert_eq!(mb[0].id, "menu");

        // Not plugin interactions → dropped.
        assert!(actions_from_binds(&binds("T", "Tiling", "hl.dsp.exec_cmd('float')")).is_empty());
        assert!(actions_from_binds(&binds("W", "Web", "omarchy-launch-webapp example.com")).is_empty());
        assert!(actions_from_binds(&binds("R", "Record", "omarchy-capture-screenrecording")).is_empty());
    }

    #[test]
    fn duplicate_ids_keep_first_combo_only() {
        let binds = vec![
            UpstreamBind {
                combo: "XF86AudioNext".into(),
                desc: "Next track".into(),
                cmd: "omarchy-shell media next".into(),
            },
            UpstreamBind {
                combo: "ALT + XF86AudioPlay".into(),
                desc: "Next track".into(),
                cmd: "omarchy-shell media next".into(),
            },
        ];
        let a = actions_from_binds(&binds);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].suggested_combo.as_deref(), Some("XF86AudioNext"));
    }

    #[test]
    fn real_pin_parses_into_a_sane_catalog() {
        // Live contract probe against the actual pin (skipped silently when
        // absent — CI machines have no bootstrap).
        let Ok(pin_dir) = std::env::var("HOME").map(|h| {
            std::path::PathBuf::from(h)
                .join(".local/share/opb/upstream/current")
        }) else {
            return;
        };
        let bindings = pin_dir.join("default/hypr/bindings");
        if !bindings.is_dir() {
            return;
        }
        let mut src = String::new();
        for f in std::fs::read_dir(bindings).unwrap().filter_map(|e| e.ok()) {
            let p = f.path();
            if p.extension().is_some_and(|x| x == "lua") {
                src.push_str(&std::fs::read_to_string(p).unwrap());
            }
        }
        let actions = actions_from_binds(&parse_binds(&src));
        assert!(actions.len() >= 20, "got {}: {:?}", actions.len(), actions);
        assert!(actions.iter().any(|a| a.id == "menu"));
        assert!(actions.iter().any(|a| a.id == "notifications:dismissOne"));
        assert!(actions
            .iter()
            .any(|a| a.id == "omarchy.emojis:toggle" && a.suggested_combo.as_deref() == Some("SUPER + CTRL + E")));
        // Loop-generated positional panel binds never leak in as garbage ids.
        assert!(!actions.iter().any(|a| a.id.contains("panel")));
    }

    #[test]
    fn render_hides_disabled_unless_all_and_honors_plugin_filter() {
        let mk = |id: &str, plugin: &str, state: &'static str, derived: bool| Entry {
            action: Action {
                id: id.into(),
                description: format!("desc {id}"),
                plugin: plugin.into(),
                invocation: "cmd".into(),
                suggested_combo: Some("SUPER + X".into()),
                derived,
            },
            state,
        };
        let entries = vec![
            mk("a:toggle", "omarchy.a", "on", false),
            mk("b:toggle", "omarchy.b", "off", true),
        ];

        let default = render(&entries, "", false, None);
        assert!(default.contains("a:toggle"));
        assert!(!default.contains("b:toggle"));

        let everything = render(&entries, "", true, None);
        assert!(everything.contains("b:toggle"));
        assert!(everything.contains("[derived]"));

        let filtered = render(&entries, "", true, Some("omarchy.b"));
        assert!(!filtered.contains("a:toggle"));
        assert!(filtered.contains("b:toggle"));
    }

    #[test]
    fn render_bind_column_reflects_keys_lua_state() {
        let mk = |id: &str, suggested: Option<&str>, state: &'static str| Entry {
            action: Action {
                id: id.into(),
                description: format!("desc {id}"),
                plugin: "omarchy.x".into(),
                invocation: "cmd".into(),
                suggested_combo: suggested.map(str::to_owned),
                derived: false,
            },
            state,
        };
        let entries = vec![
            mk("u:toggle", Some("SUPER + U"), "on"),   // unbound
            mk("s:toggle", Some("SUPER + S"), "on"),   // bound as suggested
            mk("c:toggle", Some("SUPER + C"), "on"),   // customized
        ];
        let keys = "-- opb: s:toggle | desc s\nhl.bind(\"SUPER + S\", …)\n\
                    -- opb: c:toggle | desc c\nhl.bind(\"CTRL + C\", …)\n";

        let out = render(&entries, keys, true, None);
        let line = |id: &str| out.lines().find(|l| l.contains(id)).unwrap();
        assert!(line("u:toggle").starts_with("—"), "{}", line("u:toggle"));
        assert!(line("s:toggle").starts_with("✓"), "{}", line("s:toggle"));
        assert!(line("c:toggle").starts_with("✎"), "{}", line("c:toggle"));
        assert!(line("c:toggle").contains("CTRL + C"), "current combo shown");
    }

    #[test]
    fn combo_parsing_covers_upstream_styles() {
        let c = parse_combo("SUPER + CTRL + E").unwrap();
        assert_eq!(c.modmask, MOD_SUPER | MOD_CTRL);
        assert_eq!(c.key, "E");
        assert_eq!(c.to_lua_string(), "SUPER + CTRL + E");

        let m = parse_combo("XF86AudioPlay").unwrap();
        assert_eq!(m.modmask, 0);
        assert_eq!(m.key, "XF86AUDIOPLAY");

        let code = parse_combo("SUPER + SHIFT + code:201").unwrap();
        assert_eq!(code.modmask, MOD_SUPER | MOD_SHIFT);
        assert_eq!(code.key, "code:201");

        assert!(parse_combo("").is_err());
        assert!(parse_combo("SUPER +").is_err(), "trailing plus = no key");
        assert!(parse_combo("SUPER + A + B").is_err());
    }

    #[test]
    fn collision_matches_modmask_and_key_case_insensitively() {
        let live = vec![
            LiveBind { modmask: 64, key: "Q".into(), keycode: 0 },
            LiveBind { modmask: 68, key: "down".into(), keycode: 0 },
            LiveBind { modmask: 0, key: "XF86PowerOff".into(), keycode: 0 },
            LiveBind { modmask: 64, key: "".into(), keycode: 210 },
        ];
        let hit = |s: &str| collision_excluding(&parse_combo(s).unwrap(), &live, None);
        assert_eq!(hit("SUPER + q").as_deref(), Some("SUPER + Q"));
        assert_eq!(hit("SUPER + CTRL + Down").as_deref(), Some("SUPER + CTRL + down"));
        assert_eq!(hit("xf86poweroff").as_deref(), Some("XF86PowerOff"));
        assert_eq!(hit("SUPER + code:210").as_deref(), Some("SUPER + code:210"));
        assert_eq!(hit("SUPER + E"), None);
        assert_eq!(hit("SHIFT + SUPER + q"), None, "different mask = free");
    }

    #[test]
    fn entry_render_is_self_contained_and_quoted() {
        let action = Action {
            id: "omarchy.emojis:toggle".into(),
            description: "Emojis \"quoted\"".into(),
            plugin: "omarchy.emojis".into(),
            invocation: "omarchy-shell shell toggle omarchy.emojis".into(),
            suggested_combo: None,
            derived: false,
        };
        let s = render_entry(Path::new("/pins/a"), &action, &parse_combo("SUPER + CTRL + E").unwrap());
        assert!(s.starts_with("-- opb: omarchy.emojis:toggle"));
        // Long-bracket Lua string: both quote flavors inside, no escaping.
        let expected_cmd = "hl.dsp.exec_cmd([[sh -c 'export OMARCHY_PATH=\"/pins/a\"; \
                            export PATH=\"/pins/a/bin:$PATH\"; exec omarchy-shell shell toggle omarchy.emojis']])";
        assert!(s.contains(expected_cmd), "{s}");
        assert!(s.contains("description = \"Emojis \\\"quoted\\\"\""));
        assert!(!s.contains('\r'));
    }

    #[test]
    fn append_source_creates_header_and_normalizes_trailing_newline() {
        // Empty source → header prepended.
        let src = append_source("", "-- first\nhl.bind(X)\n");
        assert!(src.starts_with("-- opb keybinds (user-owned)"));
        assert!(src.ends_with("hl.bind(X)\n"));

        // Source without trailing newline gets one before the next entry.
        let merged = append_source("tail_var = 1", "-- second\nnext_var = 2\n");
        assert!(merged.contains("tail_var = 1\n-- second\n"));
    }

    #[test]
    fn self_conflict_detection_by_combo_and_action_marker() {
        let src = "-- opb: menu:apps | Apps menu\nhl.bind(\"SUPER + ALT + SPACE\", …)\n";
        assert!(action_already_bound(src, "menu:apps"));
        assert!(!action_already_bound(src, "menu"));

        let apps = parse_combo("SUPER + ALT + SPACE").unwrap();
        assert_eq!(
            existing_combo_conflict(src, &apps).as_deref(),
            Some("SUPER + ALT + SPACE")
        );
        assert!(existing_combo_conflict(src, &parse_combo("SUPER + SPACE").unwrap()).is_none());
    }

    fn cand(id: &str, plugin: &str, combo: Option<&str>, state: &'static str) -> Entry {
        Entry {
            action: Action {
                id: id.into(),
                description: format!("desc {id}"),
                plugin: plugin.into(),
                invocation: "cmd".into(),
                suggested_combo: combo.map(str::to_owned),
                derived: false,
            },
            state,
        }
    }

    #[test]
    fn enabled_filters_disabled_but_keeps_bound() {
        let entries = vec![
            cand("a:toggle", "omarchy.a", Some("SUPER + A"), "on"),
            cand("b:toggle", "omarchy.b", Some("SUPER + B"), "off"),
            // derived → no upstream combo → still offered
            cand("c:toggle", "omarchy.c", None, "on"),
        ];

        let got = enabled_entries(&entries);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].action.id, "a:toggle");
        assert_eq!(got[1].action.id, "c:toggle");
    }

    #[test]
    fn bound_combos_parses_markers_and_state_classification() {
        let keys = "-- opb: a:toggle | desc a\nhl.bind(\"SUPER + A\", …)\n\
                    -- opb: b:toggle | desc b\nhl.bind(\"SUPER + SHIFT + B\", …)\n";
        let combos = bound_combos(keys);
        assert_eq!(
            combos.get("a:toggle").map(Combo::to_lua_string).as_deref(),
            Some("SUPER + A")
        );
        assert_eq!(
            combos.get("b:toggle").map(Combo::to_lua_string).as_deref(),
            Some("SUPER + SHIFT + B")
        );
        assert!(!combos.contains_key("c:toggle"));

        let mk = |id: &str, suggested: Option<&str>| Action {
            id: id.into(),
            description: "d".into(),
            plugin: "omarchy.x".into(),
            invocation: "cmd".into(),
            suggested_combo: suggested.map(str::to_owned),
            derived: false,
        };
        // Suggested match → ✓; divergence → ✎; none → —.
        assert_eq!(
            bind_state(&mk("a:toggle", Some("SUPER + A")), combos.get("a:toggle")),
            BindState::Suggested
        );
        assert_eq!(
            bind_state(&mk("b:toggle", Some("SUPER + B")), combos.get("b:toggle")),
            BindState::Customized
        );
        assert_eq!(
            bind_state(&mk("c:toggle", Some("SUPER + C")), combos.get("c:toggle")),
            BindState::Unbound
        );
        // Derived action (no suggestion): any bind reads as customized.
        let derived = Action {
            id: "d:toggle".into(),
            description: "d".into(),
            plugin: "omarchy.x".into(),
            invocation: "cmd".into(),
            suggested_combo: None,
            derived: true,
        };
        assert_eq!(
            bind_state(&derived, combos.get("a:toggle")),
            BindState::Customized
        );
        // Foreign binds (no opb marker) are invisible to the resolver.
        let foreign = "-- mine\nhl.bind(\"SUPER + F\", …)\n";
        assert!(bound_combos(foreign).is_empty());
    }

    fn fixture_paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::new(dir.path().to_path_buf());
        // Minimal fake pin — write_binding resolves it for env-wrapped execs.
        let pin = p.pin_dir("abc");
        std::fs::create_dir_all(pin.join("shell/plugins")).unwrap();
        std::fs::write(pin.join("version"), "4.0.0.alpha\n").unwrap();
        crate::atomic::symlink_flip(&pin, &p.current_dir()).unwrap();
        crate::pin::PinLock::stable("v4.0.0", "abc").save(&p).unwrap();
        (dir, p)
    }

    #[test]
    fn write_binding_occupied_combo_never_shadows_silently() {
        let (_d, p) = fixture_paths();

        let action = Action {
            id: "x:toggle".into(),
            description: "X".into(),
            plugin: "omarchy.x".into(),
            invocation: "cmd".into(),
            suggested_combo: None,
            derived: false,
        };
        let live = vec![LiveBind { modmask: MOD_SUPER, key: "Q".into(), keycode: 0 }];

        // Occupied combo: non-interactive stdin declines the shadow confirm —
        // nothing is written.
        assert!(write_binding(&p, &action, "SUPER + q", Some(&live)).is_err());
        assert!(!p.keys_lua().exists(), "no file created on declined shadow");

        // Free combo writes fine.
        write_binding(&p, &action, "SUPER + Z", Some(&live)).unwrap();
        assert!(p.keys_lua().exists());

        // Rebinding the same action replaces its entry in place — one bind,
        // never a duplicate.
        write_binding(&p, &action, "SUPER + Y", Some(&live)).unwrap();
        let raw = std::fs::read_to_string(p.keys_lua()).unwrap();
        assert_eq!(raw.matches("-- opb: x:toggle").count(), 1, "{raw}");
        assert!(raw.contains("SUPER + Y"), "{raw}");
        assert!(!raw.contains("SUPER + Z"), "{raw}");

        // Unbind removes the block.
        remove_binding(&p, "x:toggle").unwrap();
        let raw = std::fs::read_to_string(p.keys_lua()).unwrap();
        assert!(!raw.contains("x:toggle"), "{raw}");
    }

    #[test]
    fn rewrite_action_block_replaces_only_the_marked_entry() {
        let src = "-- opb: a:toggle | desc a\nhl.bind(\"SUPER + A\", …)\n\
                   -- opb: b:toggle | desc b\nhl.bind(\"SUPER + B\", …)\n";
        let got = rewrite_action_block(src, "a:toggle", "-- opb: a:toggle | desc a\nhl.bind(\"SUPER + X\", …)\n");
        assert!(got.contains("SUPER + X"));
        assert!(!got.contains("SUPER + A"));
        // The untouched neighbor entry survives verbatim.
        assert!(got.contains("SUPER + B"));

        // Empty replacement = removal of exactly that block.
        let removed = rewrite_action_block(src, "b:toggle", "");
        assert!(!removed.contains("b:toggle"));
        assert!(removed.contains("SUPER + A"));
    }
}
