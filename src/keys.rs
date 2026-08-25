//! Phase 5 keybinds — the action catalog (`opb keys list`).
//!
//! The catalog is **derived at runtime from the pin** (ROADMAP C2): parse
//! upstream's `default/hypr/bindings/*.lua` for shell-facing `o.bind` calls,
//! then add derived toggle entries for toggleable components that upstream
//! never bound. Nothing here is a static table — pin bumps re-derive
//! automatically, and the bindings lua paths are contract-watch sentinels.
//!
//! Enable-state gating mirrors the x-ray (plugin_list::state_of): same
//! storage rules, read-only, headless.

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

// --- rendering ------------------------------------------------------------------

/// Render rows honoring filters: default hides disabled components; `all`
/// includes them; `plugin` narrows to exact component id(s).
pub fn render(entries: &[Entry], all: bool, plugin: Option<&str>) -> String {
    let mut out = String::new();
    let width_id = entries
        .iter()
        .map(|e| e.action.id.len())
        .max()
        .unwrap_or(5)
        .max(5);
    let header = format!(
        "{:<state_w$}  {:<id_w$}  {:<14}  {}",
        "STATE",
        "ACTION",
        "COMBO/SUGGESTED",
        "DESCRIPTION",
        state_w = 6,
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
        let combo = e.action.suggested_combo.as_deref().unwrap_or("-");
        out.push_str(&format!(
            "{:<state_w$}  {:<id_w$}  {:<14}  {}{}\n",
            e.state,
            e.action.id,
            truncate_pad(combo, 14),
            e.action.description,
            if e.action.derived { " [derived]" } else { "" },
            state_w = 6,
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

/// Pure: does this combo collide with any live bind? Returns a human-readable
/// description of the first hit.
pub fn collision_with(combo: &Combo, live: &[LiveBind]) -> Option<String> {
    live.iter()
        .find(|b| combo.matches_live(b))
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
        "export OMARCHY_PATH=\"{}\"; export PATH=\"{}/bin:$PATH\"; exec {}",
        pin_dir.display(),
        pin_dir.display(),
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
-- opb keybinds (user-owned). Entries are added by `opb keys set`; hand-edit
-- freely — opb appends, never rewrites existing lines.
-- Loaded via ~/.config/hypr/opb.lua (require(\"opb\") in your Hyprland config).

";

/// Append an entry atomically, creating the file with its header when absent.
///
/// Guard rail: a syntactically-broken keys.lua errors on *every* Hyprland
/// reload and kills the user's other opb binds with it. When `luac` is
/// available, the resulting file is parse-checked before landing and the
/// write is refused (original file untouched) on failure.
pub fn append_entry(paths: &Paths, entry: &str) -> Result<()> {
    use std::path::Path;
    let path = paths.keys_lua();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let mut next = if path.exists() {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
        if !raw.ends_with('\n') && !raw.is_empty() {
            raw + "\n"
        } else {
            raw
        }
    } else {
        KEYS_HEADER.to_owned()
    };
    next.push_str(entry);
    if let Some(reported) = lua_parse_error(&next) {
        bail!(
            "refusing to write {}: generated entry does not parse ({reported}) — \
             this is an opb bug, please report it",
            path.display()
        );
    }
    crate::atomic::write(Path::new(&path), next.as_bytes())?;
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

/// Interactive [y/N] confirmation (default no — shadowing is destructive).
fn confirm(question: &str) -> bool {
    use std::io::Write;
    print!("{question} [y/N] ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok();
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Outcome of one binding write attempt.
#[derive(Debug, PartialEq)]
pub enum BindOutcome {
    Written,
    /// Not written; reason is shown to the user by the caller.
    Skipped(&'static str),
}

/// Shared write path for `keys set` and `keys import-suggested`: parse,
/// collision-check, duplicate-check, atomic append.
///
/// `allow_shadow = true` (single-bind override) prompts/overrides on an
/// occupied combo; `false` (bulk import) skips occupied combos loudly —
/// a mass import must never silently shadow the user's binds.
pub fn write_binding(
    paths: &Paths,
    action: &Action,
    combo_input: &str,
    live: Option<&[LiveBind]>,
    allow_shadow: bool,
) -> Result<BindOutcome> {
    let combo = parse_combo(combo_input)?;

    match live.map(|l| collision_with(&combo, l)) {
        None => println!("note: hyprctl unavailable — skipping live collision check"),
        Some(None) => {}
        Some(Some(hit)) => {
            if !allow_shadow {
                return Ok(BindOutcome::Skipped("combo already bound"));
            }
            println!(
                "WARNING: combo {} is already bound ({hit}) — your new bind \
                 shadows it by definition order",
                combo.to_lua_string()
            );
            if !confirm("Bind anyway (your bind wins)?") {
                anyhow::bail!("occupied combo — nothing written");
            }
        }
    }

    let path = paths.keys_lua();
    let src = std::fs::read_to_string(&path).unwrap_or_default();
    if action_already_bound(&src, &action.id) {
        anyhow::bail!(
            "{} is already bound in {} — remove its line there first",
            action.id,
            path.display()
        );
    }
    if let Some(written) = existing_combo_conflict(&src, &combo) {
        anyhow::bail!(
            "combo already used in {} for another action ({written})",
            path.display()
        );
    }

    // The env wrapper spells the tree through `current` like every other
    // launcher/caller — quickshell IPC matches by exact path (see shell_dir).
    append_entry(
        paths,
        &render_entry(&paths.current_dir(), action, &combo),
    )?;
    Ok(BindOutcome::Written)
}

/// Reload prompt shared by both writers. Returns whether a reload ran.
fn maybe_reload(yes: bool) -> bool {
    let do_reload = yes
        || (std::io::IsTerminal::is_terminal(&std::io::stdin()) && {
            use std::io::Write;
            print!("Run `hyprctl reload` now? [Y/n] ");
            std::io::stdout().flush().ok();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).ok();
            !matches!(line.trim().to_ascii_lowercase().as_str(), "n" | "no")
        });
    if !do_reload {
        println!("run `hyprctl reload` when ready");
        return false;
    }
    match std::process::Command::new("hyprctl").arg("reload").status() {
        Ok(s) if s.success() => {
            println!("reloaded — bind(s) are live");
            true
        }
        _ => {
            println!("hyprctl reload failed — run it manually when ready");
            false
        }
    }
}

/// `opb keys set <action> <combo>` — the only bind writer (D11).
pub fn set(paths: &Paths, action_id: &str, combo_input: &str, yes: bool) -> Result<()> {
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
    match write_binding(paths, &entry.action, combo_input, live.as_deref(), true)? {
        BindOutcome::Written => println!(
            "bound {} → {}",
            parse_combo(combo_input)?.to_lua_string(),
            action_id
        ),
        BindOutcome::Skipped(reason) => anyhow::bail!("{reason} — nothing written"),
    }
    println!("  wrote {}", paths.keys_lua().display());
    maybe_reload(yes);
    Ok(())
}

// --- import-suggested (C4) ------------------------------------------------------

/// Pure: upstream-declared candidates worth offering — has a suggested combo,
/// passes the plugin filter, not already bound in keys.lua.
pub fn select_candidates<'a>(
    entries: &'a [Entry],
    existing_keys_lua: &str,
    plugin_filter: Option<&str>,
) -> Vec<&'a Entry> {
    entries
        .iter()
        .filter(|e| e.action.suggested_combo.is_some())
        .filter(|e| {
            plugin_filter
                .is_none_or(|p| e.action.plugin == p || e.action.id == p)
        })
        .filter(|e| !action_already_bound(existing_keys_lua, &e.action.id))
        .collect()
}

fn truncate(s: &str, w: usize) -> String {
    if s.chars().count() > w {
        s.chars().take(w.saturating_sub(1)).collect::<String>() + "…"
    } else {
        s.to_owned()
    }
}

/// Render the candidate table (shared by the preview and the non-tty path).
fn render_candidates(candidates: &[&Entry], live: Option<&[LiveBind]>) -> String {
    let mut out = String::from("  COMBO                 ACTION                         STATE     CONFLICT\n");
    for c in candidates {
        let combo = c.action.suggested_combo.as_deref().unwrap_or("-");
        let parsed = parse_combo(combo);
        let hit = parsed
            .as_ref()
            .ok()
            .and_then(|c| live.and_then(|l| collision_with(c, l)));
        out.push_str(&format!(
            "  {:<21} {:<31} {:<9} {}\n",
            truncate(combo, 21),
            truncate(&c.action.id, 31),
            c.state,
            hit.unwrap_or_else(|| "-".into()),
        ));
    }
    out
}

/// `opb keys import-suggested [--plugin <id>] [--yes]` — opt-in bulk import
/// of upstream's binding table as suggestions (CONCEPT §4 Keybind model).
///
/// Interactive: arrow-key multi-select (space toggles, enter confirms, ESC
/// aborts). `--yes` accepts every candidate whose combo is free and skips
/// occupied ones loudly — a bulk import never silently shadows anything.
/// Non-interactive without `--yes`: renders the table read-only.
pub fn import_suggested(paths: &Paths, plugin_filter: Option<&str>, yes: bool) -> Result<()> {
    let entries = catalog(paths)?;
    let existing = std::fs::read_to_string(paths.keys_lua()).unwrap_or_default();
    let live = live_binds();
    let candidates = select_candidates(&entries, &existing, plugin_filter);

    if candidates.is_empty() {
        println!("opb keys import-suggested: nothing to import");
        println!("  (every upstream bind is already present, or no component matches)");
        return Ok(());
    }

    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
    if !interactive && !yes {
        println!("opb keys import-suggested: candidates (nothing written):");
        print!("{}", render_candidates(&candidates, live.as_deref()));
        println!("\nre-run from a terminal to select, or with --yes to accept all free combos");
        return Ok(());
    }

    // Classify occupancy once up front.
    enum Occupancy {
        Free,
        Occupied(String),
    }
    let classified: Vec<(&&Entry, Occupancy)> = candidates
        .iter()
        .map(|c| {
            let occ = c
                .action
                .suggested_combo
                .as_deref()
                .and_then(|s| parse_combo(s).ok())
                .and_then(|c| live.as_deref().and_then(|l| collision_with(&c, l)));
            (
                c,
                match occ {
                    Some(hit) => Occupancy::Occupied(hit),
                    None => Occupancy::Free,
                },
            )
        })
        .collect();

    let chosen: Vec<usize> = if yes {
        // Bulk accept: free combos only, never silent shadowing.
        classified
            .iter()
            .enumerate()
            .filter(|(_, (_, o))| matches!(o, Occupancy::Free))
            .map(|(i, _)| i)
            .collect()
    } else {
        let labels: Vec<String> = classified
            .iter()
            .map(|(c, o)| {
                let occ = match o {
                    Occupancy::Free => String::new(),
                    Occupancy::Occupied(by) => format!("  ⚠ occupies: {by}"),
                };
                format!(
                    "{:<22} {:<30} [{}]",
                    truncate(c.action.suggested_combo.as_deref().unwrap_or("-"), 22),
                    truncate(&c.action.description, 30),
                    c.action.plugin,
                ) + &occ
            })
            .collect();
        let selection = dialoguer::MultiSelect::with_theme(&dialoguer::theme::ColorfulTheme::default())
            .with_prompt("Import keybinds (↑↓ move · space toggles · a/all · enter confirms · esc aborts)")
            .items_checked(&labels.iter().map(|l| (l.clone(), false)).collect::<Vec<_>>())
            .report(false)
            .interact_opt();
        match selection {
            Err(dialoguer::Error::IO(e)) if e.kind() == std::io::ErrorKind::Interrupted => Vec::new(),
            Err(e) => return Err(anyhow::anyhow!("selection failed: {e}")),
            Ok(None) => {
                println!("aborted — nothing written");
                return Ok(());
            }
            Ok(Some(idx)) => idx,
        }
    };

    let mut written = 0usize;
    for i in chosen {
        let (c, _occ) = &classified[i];
        // Bulk (--yes) mode never shadows; single selections may override
        // explicitly through the normal confirm path.
        let allow_shadow = !yes;
        let combo = c.action.suggested_combo.as_deref().unwrap_or_default();
        match write_binding(paths, &c.action, combo, live.as_deref(), allow_shadow)? {
            BindOutcome::Written => {
                written += 1;
                println!("bound {} → {}", combo, c.action.id);
            }
            BindOutcome::Skipped(reason) => {
                println!("skipped {}: {reason}", c.action.id);
            }
        }
    }

    let occupied = classified
        .iter()
        .filter(|(_, o)| matches!(o, Occupancy::Occupied(_)))
        .count();
    if occupied > 0 {
        println!(
            "{occupied} candidate(s) skipped: their upstream combo is already bound on this \
             system — bind individually with `opb keys set <action> <combo>` if you want to shadow"
        );
    }
    if written > 0 {
        println!("wrote {} entr{} to {}", written, if written == 1 { "y" } else { "ies" }, paths.keys_lua().display());
        maybe_reload(yes);
    } else {
        println!("nothing written");
    }
    Ok(())
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

        let default = render(&entries, false, None);
        assert!(default.contains("a:toggle"));
        assert!(!default.contains("b:toggle"));

        let everything = render(&entries, true, None);
        assert!(everything.contains("b:toggle"));
        assert!(everything.contains("[derived]"));

        let filtered = render(&entries, true, Some("omarchy.b"));
        assert!(!filtered.contains("a:toggle"));
        assert!(filtered.contains("b:toggle"));
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
        let hit = |s: &str| collision_with(&parse_combo(s).unwrap(), &live);
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
    fn append_creates_with_header_and_appends_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::new(dir.path().to_path_buf());
        append_entry(&p, "-- first\nhl.bind(X)\n").unwrap();
        let raw = std::fs::read_to_string(p.keys_lua()).unwrap();
        assert!(raw.starts_with("-- opb keybinds (user-owned)"));
        assert!(raw.ends_with("hl.bind(X)\n"));

        // File without trailing newline gets one before the next entry.
        // (Fixture entries must be valid Lua — append_entry parse-checks.)
        std::fs::write(p.keys_lua(), "tail_var = 1").unwrap();
        append_entry(&p, "-- second\nnext_var = 2\n").unwrap();
        let raw = std::fs::read_to_string(p.keys_lua()).unwrap();
        assert!(raw.contains("tail_var = 1\n-- second\n"));
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
    fn candidate_selection_filters_combo_plugin_and_already_bound() {
        let entries = vec![
            cand("a:toggle", "omarchy.a", Some("SUPER + A"), "on"),
            cand("b:toggle", "omarchy.b", Some("SUPER + B"), "off"),
            // derived → no upstream combo → never a candidate
            cand("c:toggle", "omarchy.c", None, "on"),
        ];
        let existing = "-- opb: a:toggle | desc a\n";

        let all = select_candidates(&entries, "", None);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].action.id, "a:toggle");

        let filtered = select_candidates(&entries, "", Some("omarchy.b"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].action.id, "b:toggle");

        // Already bound → excluded.
        assert!(select_candidates(&entries, existing, None)
            .iter()
            .all(|e| e.action.id != "a:toggle"));
    }

    #[test]
    fn write_binding_bulk_mode_never_shadows() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::new(dir.path().to_path_buf());
        // Minimal fake pin — write_binding resolves it for env-wrapped execs.
        let pin = p.pin_dir("abc");
        std::fs::create_dir_all(pin.join("shell/plugins")).unwrap();
        std::fs::write(pin.join("version"), "4.0.0.alpha\n").unwrap();
        crate::atomic::symlink_flip(&pin, &p.current_dir()).unwrap();
        crate::pin::PinLock::stable("v4.0.0", "abc").save(&p).unwrap();

        let action = Action {
            id: "x:toggle".into(),
            description: "X".into(),
            plugin: "omarchy.x".into(),
            invocation: "cmd".into(),
            suggested_combo: None,
            derived: false,
        };
        let live = vec![LiveBind { modmask: MOD_SUPER, key: "Q".into(), keycode: 0 }];

        // Bulk mode (allow_shadow=false): occupied combo skipped, nothing written.
        let out = write_binding(&p, &action, "SUPER + q", Some(&live), false).unwrap();
        assert_eq!(out, BindOutcome::Skipped("combo already bound"));
        assert!(!p.keys_lua().exists(), "no file created on skip");

        // Free combo writes fine in the same mode.
        let out = write_binding(&p, &action, "SUPER + Z", Some(&live), false).unwrap();
        assert_eq!(out, BindOutcome::Written);
        assert!(p.keys_lua().exists());

        // Second attempt for the same action is refused outright.
        assert!(write_binding(&p, &action, "SUPER + Y", Some(&live), false).is_err());
    }
}
