//! `opb select enable/disable <id>` — applies the selection model (CONCEPT
//! §4) by surgically editing the generated `shell.json`, then reloads a
//! running shell via IPC. Ids are resolved read-only against pin + user
//! manifests; the edit itself is pure (`selection`) and lands atomically.

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::atomic;
use crate::paths::Paths;
use crate::pin;
use crate::selection::{self, Outcome, PluginKind, Section};
use crate::shelljson;

/// One select operation.
#[derive(Debug, Clone)]
pub enum Action {
    Enable { id: String, section: Section },
    Disable { id: String },
}

/// Resolve an id against pin + user manifests → how it renders.
/// First-party ids come from `<pin>/shell/plugins/**`, user plugins from
/// `$HOME/.config/omarchy/plugins/**` (D5: same dir a real Omarchy uses).
pub fn resolve_kind(paths: &Paths, id: &str) -> Result<PluginKind> {
    let manifests = scan_all(paths)?;
    match manifests.iter().find(|m| m.id == id) {
        Some(m) => Ok(if m.kinds.iter().any(|k| k == "bar-widget") {
            PluginKind::BarWidget
        } else {
            PluginKind::Regular
        }),
        None => bail!(
            "unknown plugin id '{id}' — not in the pinned tree or ~/.config/omarchy/plugins \
             (see available ids with `opb select list`)"
        ),
    }
}

fn scan_all(paths: &Paths) -> Result<Vec<shelljson::ManifestInfo>> {
    let mut out = Vec::new();
    if let Ok(pin_dir) = pin::active_dir(paths) {
        out.extend(shelljson::scan_manifests(&pin_dir.join("shell/plugins"))?);
    }
    let user = user_plugins_dir(paths);
    out.extend(shelljson::scan_manifests(&user)?);
    Ok(out)
}

fn user_plugins_dir(paths: &Paths) -> std::path::PathBuf {
    paths.omarchy_config_dir().join("plugins")
}

/// Load + parse shell.json. Missing file is a hard error: without it we would
/// be editing a file the shell never wrote (D6: our generated file is the
/// authoritative starting point).
pub fn load_doc(paths: &Paths) -> Result<Value> {
    let path = paths.shell_json();
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read {} — run `opb bootstrap` first", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

/// Apply an action to shell.json (no IPC). Returns the outcome plus whether
/// anything was written.
pub fn apply(paths: &Paths, action: &Action) -> Result<Outcome> {
    match action {
        Action::Enable { id, section } => {
            let kind = resolve_kind(paths, id)?;
            let mut doc = load_doc(paths)?;
            let outcome = selection::enable(&mut doc, id, kind, *section)?;
            write_if_changed(paths, &doc, &outcome)?;
            Ok(outcome)
        }
        Action::Disable { id } => {
            // The bar is refused before manifest resolution so disabling it
            // works even though its manifest kind is `bar`.
            let mut doc = load_doc(paths)?;
            let outcome = selection::disable(&mut doc, id)?;
            write_if_changed(paths, &doc, &outcome)?;
            Ok(outcome)
        }
    }
}

fn write_if_changed(paths: &Paths, doc: &Value, outcome: &Outcome) -> Result<()> {
    if !outcome.changed() {
        return Ok(());
    }
    atomic::write(&paths.shell_json(), shelljson::render(doc).as_bytes())
        .context("write shell.json")
}

/// Plugins roots for display purposes (`select list`, C4).
#[allow(dead_code)] // consumed by `opb select list` in the next commit
pub fn plugins_roots(paths: &Paths) -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Ok(pin_dir) = pin::active_dir(paths) {
        roots.push(pin_dir.join("shell/plugins"));
    }
    roots.push(user_plugins_dir(paths));
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::BAR_ID;

    /// Fake install: current → pin with two first-party plugins; one user plugin.
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
        atomic::write(&paths.shell_json(), shelljson::render(&doc).as_bytes()).unwrap();
    }

    #[test]
    fn resolves_widget_and_regular_kinds() {
        let (_d, paths) = fixture();
        assert_eq!(
            resolve_kind(&paths, "omarchy.clock").unwrap(),
            PluginKind::BarWidget
        );
        assert_eq!(
            resolve_kind(&paths, "omarchy.idle").unwrap(),
            PluginKind::Regular
        );
        assert_eq!(
            resolve_kind(&paths, "cool.panel").unwrap(),
            PluginKind::Regular
        );
    }

    #[test]
    fn unknown_id_lists_where_to_look() {
        let (_d, paths) = fixture();
        let err = resolve_kind(&paths, "ghost").unwrap_err();
        assert!(err.to_string().contains("unknown plugin id"), "got: {err}");
    }

    #[test]
    fn missing_shell_json_points_at_bootstrap() {
        let (_d, paths) = fixture();
        let err = load_doc(&paths).unwrap_err();
        assert!(err.to_string().contains("opb bootstrap"), "got: {err}");
    }

    #[test]
    fn enable_writes_file_disable_restores_it_byte_identical() {
        let (_d, paths) = fixture();
        seed_shell_json(&paths);
        let original = std::fs::read_to_string(paths.shell_json()).unwrap();

        let out = apply(
            &paths,
            &Action::Enable {
                id: "omarchy.clock".to_owned(),
                section: Section::Center,
            },
        )
        .unwrap();
        assert!(out.changed());
        let edited = std::fs::read_to_string(paths.shell_json()).unwrap();
        assert!(edited.contains("\"center\": ["), "got: {edited}");
        let doc: Value = serde_json::from_str(&edited).unwrap();
        assert_eq!(doc["bar"]["layout"]["center"][0], "omarchy.clock");
        assert_ne!(edited, original);

        let out = apply(&paths, &Action::Disable { id: "omarchy.clock".to_owned() }).unwrap();
        assert!(out.changed());
        assert_eq!(std::fs::read_to_string(paths.shell_json()).unwrap(), original);
    }

    #[test]
    fn unchanged_outcome_skips_the_write() {
        let (_d, paths) = fixture();
        seed_shell_json(&paths);
        let enable = || {
            apply(
                &paths,
                &Action::Enable {
                    id: "omarchy.clock".to_owned(),
                    section: Section::Center,
                },
            )
            .unwrap()
        };
        assert!(enable().changed());
        let after = std::fs::read_to_string(paths.shell_json()).unwrap();
        // Second identical enable: no change, file untouched.
        assert!(!enable().changed());
        assert_eq!(std::fs::read_to_string(paths.shell_json()).unwrap(), after);
    }

    #[test]
    fn bar_disable_refused_before_any_write() {
        let (_d, paths) = fixture();
        seed_shell_json(&paths);
        let err = apply(&paths, &Action::Disable { id: BAR_ID.to_owned() }).unwrap_err();
        assert!(err.to_string().contains("cannot be disabled"), "got: {err}");
    }

    #[test]
    fn third_party_enable_lands_in_plugins_array() {
        let (_d, paths) = fixture();
        seed_shell_json(&paths);
        apply(
            &paths,
            &Action::Enable {
                id: "cool.panel".to_owned(),
                section: Section::Right,
            },
        )
        .unwrap();
        let doc: Value =
            serde_json::from_str(&std::fs::read_to_string(paths.shell_json()).unwrap()).unwrap();
        assert_eq!(doc["plugins"][0], "cool.panel");
    }

    #[test]
    fn plugins_roots_cover_pin_and_user_dirs() {
        let (_d, paths) = fixture();
        let roots = plugins_roots(&paths);
        assert_eq!(roots.len(), 2);
        assert!(roots[0].ends_with("shell/plugins"));
        assert!(roots[1].ends_with(".config/omarchy/plugins"));
    }
}
