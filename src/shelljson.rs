//! shell.json generation and first-party id discovery (CONCEPT §3.4, D6/D7).
//!
//! The shell enables every first-party non-bar plugin unless it appears in
//! `disabledPlugins[]` (PluginRegistry.isEnabled). An all-off config therefore
//! lists exactly those ids there. Bar-widgets too: empty layout means nothing
//! renders, but loading the component is not "off".

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// Kind marking the bar itself — never disabled (D7), stays active via `bar.id`.
const BAR_KIND: &str = "bar";

/// Build the complete all-off config (D6): every first-party non-bar plugin in
/// `disabledPlugins[]`, empty bar layout, no third-party plugins. Deterministic
/// for a given id list (sorted, deduped).
pub fn generate(first_party_non_bar: &[String]) -> serde_json::Value {
    let mut disabled = first_party_non_bar.to_vec();
    disabled.sort();
    disabled.dedup();
    serde_json::json!({
        "version": 1,
        "bar": {
            "id": "omarchy.bar",
            "layout": {
                "left": [],
                "center": [],
                "right": [],
            },
        },
        "disabledPlugins": disabled,
        "plugins": [],
    })
}

/// Pretty-print with a trailing newline — readable, git-diffable file.
pub fn render(value: &serde_json::Value) -> String {
    let mut s = serde_json::to_string_pretty(value).expect("json serialization cannot fail");
    s.push('\n');
    s
}

/// Read-only manifest projection — ids and kinds only. No validation logic
/// (anti-duplication §5.2): upstream `omarchy plugin validate` owns that.
#[derive(Debug, Deserialize)]
struct ManifestId {
    id: String,
    #[serde(default)]
    kinds: Vec<String>,
}

/// One scanned manifest: id + declared kinds.
#[derive(Debug, Clone)]
pub struct ManifestInfo {
    pub id: String,
    pub kinds: Vec<String>,
}

/// Scan a plugins root read-only, mirroring PluginRegistry's
/// `find -mindepth 2 -maxdepth 3 -type f \( -name manifest.json
/// -o -name '*.manifest.json' \)`. Malformed manifests are skipped,
/// matching upstream's warn-and-continue scan. Sorted by id, deduped.
pub fn scan_manifests(plugins_dir: &Path) -> Result<Vec<ManifestInfo>> {
    if !plugins_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(plugins_dir)
        .min_depth(2)
        .max_depth(3)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if name != "manifest.json" && !name.ends_with(".manifest.json") {
            continue;
        }
        let raw = std::fs::read(entry.path())
            .with_context(|| format!("read {}", entry.path().display()))?;
        let manifest: ManifestId = match serde_json::from_slice(&raw) {
            Ok(m) => m,
            Err(_) => continue, // malformed manifest: upstream skips it too
        };
        out.push(ManifestInfo {
            id: manifest.id,
            kinds: manifest.kinds,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    Ok(out)
}

/// Sorted ids of every first-party plugin except the bar itself (kind `bar`)
/// — exactly the ids an all-off config must place in `disabledPlugins[]`.
pub fn first_party_non_bar_ids(plugins_dir: &Path) -> Result<Vec<String>> {
    Ok(scan_manifests(plugins_dir)?
        .into_iter()
        .filter(|m| !m.kinds.iter().any(|k| k == BAR_KIND))
        .map(|m| m.id)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn empty_input_is_minimal_bar_only() {
        let v = generate(&[]);
        assert_eq!(v["bar"]["id"], "omarchy.bar");
        for section in ["left", "center", "right"] {
            assert!(v["bar"]["layout"][section].as_array().unwrap().is_empty());
        }
        assert!(v["disabledPlugins"].as_array().unwrap().is_empty());
        assert!(v["plugins"].as_array().unwrap().is_empty());
    }

    #[test]
    fn all_off_disables_every_first_party_id_sorted() {
        let ids = vec!["omarchy.lock".to_owned(), "omarchy.clock".to_owned()];
        let v = generate(&ids);
        let disabled: Vec<_> = v["disabledPlugins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert_eq!(disabled, ["omarchy.clock", "omarchy.lock"]);
    }

    #[test]
    fn render_is_pretty_and_newline_terminated() {
        let s = render(&generate(&["omarchy.x".to_owned()]));
        assert!(s.starts_with("{\n"), "got: {s}");
        assert!(s.ends_with("}\n"));
    }

    #[test]
    fn output_is_deterministic() {
        let ids = vec!["b".to_owned(), "a".to_owned()];
        assert_eq!(render(&generate(&ids)), render(&generate(&ids)));
    }

    fn write_manifest(dir: &Path, id: &str, kinds: &[&str]) {
        fs::create_dir_all(dir).unwrap();
        let v = serde_json::json!({ "schemaVersion": 1, "id": id, "kinds": kinds });
        fs::write(dir.join("manifest.json"), serde_json::to_vec(&v).unwrap()).unwrap();
    }

    #[test]
    fn scan_mirrors_upstream_find() {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(&dir.path().join("bar"), "omarchy.bar", &["bar"]);
        write_manifest(&dir.path().join("clipboard"), "omarchy.clipboard", &["overlay"]);
        write_manifest(&dir.path().join("services/idle"), "omarchy.idle", &["service"]);
        write_manifest(&dir.path().join("panels/weather"), "omarchy.weather", &["panel"]);
        // Sibling manifest pattern (bar/widgets/Clock.manifest.json).
        fs::create_dir_all(dir.path().join("bar/widgets")).unwrap();
        let clock = serde_json::json!({ "id": "omarchy.clock", "kinds": ["bar-widget"] });
        fs::write(
            dir.path().join("bar/widgets/Clock.manifest.json"),
            serde_json::to_vec(&clock).unwrap(),
        )
        .unwrap();
        // Depth 4 is out of upstream's maxdepth 3 range → ignored.
        write_manifest(&dir.path().join("services/deep/nested"), "omarchy.deep", &["service"]);

        let ids = first_party_non_bar_ids(dir.path()).unwrap();
        assert_eq!(
            ids,
            ["omarchy.clipboard", "omarchy.clock", "omarchy.idle", "omarchy.weather"]
        );
    }

    #[test]
    fn scan_skips_malformed_manifests() {
        let dir = tempfile::tempdir().unwrap();
        write_manifest(&dir.path().join("ok"), "omarchy.ok", &["overlay"]);
        fs::create_dir_all(dir.path().join("broken")).unwrap();
        fs::write(dir.path().join("broken/manifest.json"), "not json").unwrap();
        assert_eq!(first_party_non_bar_ids(dir.path()).unwrap(), ["omarchy.ok"]);
    }

    #[test]
    fn scan_of_missing_or_empty_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(first_party_non_bar_ids(dir.path()).unwrap().is_empty());
        assert!(
            first_party_non_bar_ids(&dir.path().join("missing")).unwrap().is_empty()
        );
    }
}