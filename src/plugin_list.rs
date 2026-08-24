//! Bare `opb plugin list` — the read-only x-ray over plugin/component state
//! (D13: mutations belong to upstream alone, via `opb plugin enable/disable`).
//! Rows = manifests × shell.json storage rules × live §10 conflicts.
//! Intercepted because upstream's list is IPC-only (dead headless) and shows
//! no conflicts; `--json` and every other arg vector forward verbatim.

use anyhow::{Context, Result};
use serde_json::Value;

use crate::paths::Paths;
use crate::pin;
use crate::shelljson;

fn scan_all(paths: &Paths) -> Result<Vec<Scanned>> {
    let mut out = Vec::new();
    if let Ok(pin_dir) = pin::active_dir(paths) {
        out.extend(
            shelljson::scan_manifests(&pin_dir.join("shell/plugins"))?
                .into_iter()
                .map(|info| Scanned { info, origin: "first-party" }),
        );
    }
    let user = user_plugins_dir(paths);
    out.extend(
        shelljson::scan_manifests(&user)?
            .into_iter()
            .map(|info| Scanned { info, origin: "user" }),
    );
    Ok(out)
}

/// A manifest plus where it was found.
pub struct Scanned {
    pub info: shelljson::ManifestInfo,
    pub origin: &'static str,
}

fn user_plugins_dir(paths: &Paths) -> std::path::PathBuf {
    paths.omarchy_config_dir().join("plugins")
}

/// Load + parse shell.json.
pub fn load_doc(paths: &Paths) -> Result<Value> {
    let path = paths.shell_json();
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read {} — run `opb bootstrap` first", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

// --- storage rules, read-only (CONCEPT §3.4) -------------------------------

/// First-party namespace is reserved upstream (§3.1), so the prefix alone
/// decides which storage rule applies.
pub fn is_first_party(id: &str) -> bool {
    // `omarchy.` with the dot: "omarchish" etc. don't match.
    id.starts_with("omarchy.")
}

/// Is `id` listed in `disabledPlugins[]`?
fn is_listed_disabled(doc: &Value, id: &str) -> bool {
    doc.get("disabledPlugins")
        .and_then(|v| v.as_array())
        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(id)))
}

/// Does `id` appear in any bar layout section?
fn is_placed_in_layout(doc: &Value, id: &str) -> bool {
    doc.get("bar")
        .and_then(|bar| bar.get("layout"))
        .and_then(|l| l.as_object())
        .is_some_and(|layout| {
            layout.values().any(|v| {
                v.as_array().is_some_and(|a| a.iter().any(|e| e.as_str() == Some(id)))
            })
        })
}

/// Does `id` appear in `plugins[]`?
fn is_in_plugins(doc: &Value, id: &str) -> bool {
    doc.get("plugins")
        .and_then(|v| v.as_array())
        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(id)))
}

// --- x-ray ------------------------------------------------------------------

/// One list row — display only, no validation logic (§5.2).
pub struct Row {
    pub id: String,
    pub origin: &'static str,
    pub kind: String,
    pub state: &'static str,
    pub conflict: Option<String>,
}

/// Pure: enabled/disabled state — storage rules for services/panels,
/// upstream widget semantics (layout membership) for bar-widgets.
pub fn state_of(doc: &Value, kinds: &[String], id: &str) -> &'static str {
    if kinds.iter().any(|k| k == "bar") {
        return "bar";
    }
    let placed = is_placed_in_layout(doc, id);
    // Upstream widget semantics (PluginRegistry: isEnabled ≠ "sits in the
    // bar"): a widget renders iff it occupies a layout slot.
    if kinds.iter().any(|k| k == "bar-widget") {
        return if placed { "on" } else { "off" };
    }
    if is_first_party(id) {
        if is_listed_disabled(doc, id) {
            "off"
        } else {
            "on"
        }
    } else if placed || is_in_plugins(doc, id) {
        "on"
    } else {
        "off"
    }
}

/// Assemble display rows from manifests × shell.json × live bus state.
pub fn list_rows(paths: &Paths, processes: &[String]) -> Result<Vec<Row>> {
    let doc = load_doc(paths)?;
    let scanned = scan_all(paths)?;
    // Conflict scan is gated on enabled matrix components (§10), same as doctor.
    let enabled = crate::doctor::shellcfg::enabled_components(&doc);
    let refs: Vec<&str> = enabled.iter().map(String::as_str).collect();
    let conflicts = crate::doctor::conflicts::scan(processes, &refs);

    let mut rows: Vec<Row> = scanned
        .into_iter()
        .map(|s| {
            let state = state_of(&doc, &s.info.kinds, &s.info.id);
            let conflict = conflicts
                .iter()
                .find(|c| c.name == s.info.id)
                .map(|c| match &c.status {
                    crate::check::Status::Warn(d) | crate::check::Status::Info(d) => {
                        d.clone()
                    }
                    _ => String::new(),
                })
                .filter(|d| !d.is_empty());
            Row {
                id: s.info.id.clone(),
                origin: s.origin,
                kind: if s.info.kinds.is_empty() {
                    "-".to_owned()
                } else {
                    s.info.kinds.join(",")
                },
                state,
                conflict,
            }
        })
        .collect();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(rows)
}

/// Aligned plain-text table; conflict column only present when any row has one.
pub fn render_rows(rows: &[Row]) -> String {
    let show_conflict = rows.iter().any(|r| r.conflict.is_some());
    let w_state = rows.iter().map(|r| r.state.len()).max().unwrap_or(3).max(5);
    let w_id = rows.iter().map(|r| r.id.len()).max().unwrap_or(2).max(2);
    let w_origin = rows
        .iter()
        .map(|r| r.origin.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let w_kind = rows.iter().map(|r| r.kind.len()).max().unwrap_or(4).max(4);

    let header = if show_conflict {
        format!(
            "{:<w_state$}  {:<w_id$}  {:<w_origin$}  {:<w_kind$}  CONFLICT\n",
            "STATE",
            "ID",
            "ORIGIN",
            "KIND",
            w_state = w_state,
            w_id = w_id,
            w_origin = w_origin,
            w_kind = w_kind,
        )
    } else {
        format!(
            "{:<w_state$}  {:<w_id$}  {:<w_origin$}  {}\n",
            "STATE",
            "ID",
            "ORIGIN",
            "KIND",
            w_state = w_state,
            w_id = w_id,
            w_origin = w_origin,
        )
    };
    let mut out = header;
    for r in rows {
        let line = if show_conflict {
            format!(
                "{:<w_state$}  {:<w_id$}  {:<w_origin$}  {:<w_kind$}  {}\n",
                r.state,
                r.id,
                r.origin,
                r.kind,
                r.conflict.as_deref().unwrap_or(""),
                w_state = w_state,
                w_id = w_id,
                w_origin = w_origin,
                w_kind = w_kind,
            )
        } else {
            format!(
                "{:<w_state$}  {:<w_id$}  {:<w_origin$}  {}\n",
                r.state,
                r.id,
                r.origin,
                r.kind,
                w_state = w_state,
                w_id = w_id,
                w_origin = w_origin,
            )
        };
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake install: current → pin with first-party plugins; one user plugin.
    fn fixture() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let pin = dir.path().join("pin");
        // Clock uses upstream's sibling-manifest layout; idle a plain one.
        let clock = pin.join("shell/plugins/bar/widgets");
        std::fs::create_dir_all(&clock).unwrap();
        std::fs::write(
            clock.join("Clock.manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "id": "omarchy.clock",
                "kinds": ["bar-widget"]
            }))
            .unwrap(),
        )
        .unwrap();
        let idle = pin.join("shell/plugins/services/idle");
        std::fs::create_dir_all(&idle).unwrap();
        std::fs::write(
            idle.join("manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "id": "omarchy.idle",
                "kinds": ["service"]
            }))
            .unwrap(),
        )
        .unwrap();
        for (sub, id) in [
            ("notifications", "omarchy.notifications"),
            ("polkit", "omarchy.polkit"),
        ] {
            let d = pin.join("shell/plugins/services").join(sub);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("manifest.json"),
                serde_json::to_vec(&serde_json::json!({
                    "schemaVersion": 1,
                    "id": id,
                    "kinds": ["service"]
                }))
                .unwrap(),
            )
            .unwrap();
        }
        let user = dir.path().join("home/.config/omarchy/plugins/cool.panel");
        std::fs::create_dir_all(&user).unwrap();
        let m = serde_json::json!({ "schemaVersion": 1, "id": "cool.panel", "kinds": ["panel"] });
        std::fs::write(user.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();

        let upstream = dir.path().join("data/opb/upstream");
        std::fs::create_dir_all(&upstream).unwrap();
        std::os::unix::fs::symlink(&pin, upstream.join("current")).unwrap();

        let paths = Paths::from_parts(
            dir.path().join("home"),
            dir.path().join("data"),
            dir.path().join("home/.config"),
        );
        (dir, paths)
    }

    fn seed_shell_json(paths: &Paths) {
        let doc = crate::shelljson::generate(
            &crate::shelljson::first_party_non_bar_ids(
                &paths.current_dir().join("shell/plugins"),
            )
            .unwrap(),
        );
        std::fs::write(paths.shell_json(), shelljson::render(&doc)).unwrap();
    }

    #[test]
    fn missing_shell_json_points_at_bootstrap() {
        let (_d, paths) = fixture();
        let err = load_doc(&paths).unwrap_err();
        assert!(err.to_string().contains("opb bootstrap"), "got: {err}");
    }

    #[test]
    fn states_follow_storage_rules() {
        let mut doc = all_off();
        // First-party off (listed), third-party off (absent).
        assert_eq!(state_of(&doc, &["bar-widget".into()], "omarchy.clock"), "off");
        assert_eq!(state_of(&doc, &["panel".into()], "cool.panel"), "off");
        // Widget semantics: layout membership is the whole truth — even
        // unlisted (upstream's disable leaves it that way), absent from the
        // bar means off.
        doc["disabledPlugins"] = serde_json::json!(["omarchy.idle"]);
        assert_eq!(state_of(&doc, &["bar-widget".into()], "omarchy.clock"), "off");
        doc["bar"]["layout"]["left"] = serde_json::json!(["omarchy.clock"]);
        assert_eq!(state_of(&doc, &["bar-widget".into()], "omarchy.clock"), "on");
        // Third-party on via plugins[].
        assert_eq!(state_of(&doc, &["panel".into()], "cool.panel"), "off");
        doc["plugins"] = serde_json::json!(["cool.panel"]);
        assert_eq!(state_of(&doc, &["panel".into()], "cool.panel"), "on");
        // Bar kind is always active regardless of everything else.
        assert_eq!(state_of(&doc, &["bar".into()], "omarchy.bar"), "bar");
    }

    fn all_off() -> serde_json::Value {
        serde_json::from_value(crate::shelljson::generate(&[
            "omarchy.clock".to_owned(),
            "omarchy.notifications".to_owned(),
            "omarchy.polkit".to_owned(),
        ]))
        .unwrap()
    }

    #[test]
    fn list_rows_reflect_doc_and_gate_conflicts_on_enabled_set() {
        let (_d, paths) = fixture();
        seed_shell_json(&paths);
        // All-off: no conflicts even with every colliding daemon live.
        let procs = vec!["mako".to_owned(), "hyprpolkitagent".to_owned(), "waybar".to_owned()];
        let rows = list_rows(&paths, &procs).unwrap();
        let clock = rows.iter().find(|r| r.id == "omarchy.clock").unwrap();
        assert_eq!((clock.state, clock.origin), ("off", "first-party"));
        assert!(clock.conflict.is_none());
        let panel = rows.iter().find(|r| r.id == "cool.panel").unwrap();
        assert_eq!(panel.origin, "user");

        // Enable notifications the way the shell would: delist from
        // disabledPlugins[] (D13: upstream is the writer; we simulate its write).
        let mut doc: Value =
            serde_json::from_str(&std::fs::read_to_string(paths.shell_json()).unwrap()).unwrap();
        doc["disabledPlugins"] = serde_json::json!(
            doc["disabledPlugins"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|v| v.as_str() != Some("omarchy.notifications"))
                .collect::<Vec<_>>()
        );
        std::fs::write(paths.shell_json(), shelljson::render(&doc)).unwrap();
        let rows = list_rows(&paths, &procs).unwrap();
        let notif = rows.iter().find(|r| r.id == "omarchy.notifications").unwrap();
        assert_eq!(notif.state, "on");
        assert!(
            notif.conflict.as_deref().is_some_and(|c| c.contains("mako")),
            "got: {:?}",
            notif.conflict
        );
        // Polkit stays clean: agent running but component still disabled.
        let polkit = rows.iter().find(|r| r.id == "omarchy.polkit").unwrap();
        assert!(polkit.conflict.is_none());
    }

    #[test]
    fn render_aligns_and_hides_empty_conflict_column() {
        let rows = vec![Row {
            id: "a".to_owned(),
            origin: "user",
            kind: "panel".to_owned(),
            state: "on",
            conflict: None,
        }];
        let out = render_rows(&rows);
        assert!(!out.contains("CONFLICT"), "got: {out}");
        assert_eq!(
            out.lines().nth(1).unwrap().split_whitespace().collect::<Vec<_>>(),
            ["on", "a", "user", "panel"]
        );

        let warned = vec![Row {
            id: "b".to_owned(),
            origin: "first-party",
            kind: "-".to_owned(),
            state: "on",
            conflict: Some("mako owns the notifications bus name".to_owned()),
        }];
        let out = render_rows(&warned);
        assert!(out.contains("CONFLICT"), "got: {out}");
        assert!(out.lines().nth(1).unwrap().ends_with("mako owns the notifications bus name"));
    }

    #[test]
    fn first_party_detection_is_namespace_exact() {
        assert!(is_first_party("omarchy.lock"));
        assert!(!is_first_party("third.party"));
        assert!(!is_first_party("omarchish")); // prefix must be the full segment
    }
}
