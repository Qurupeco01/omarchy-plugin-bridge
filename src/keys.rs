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

use anyhow::Result;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
