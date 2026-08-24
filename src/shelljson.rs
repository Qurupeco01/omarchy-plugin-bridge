//! shell.json generation and first-party id discovery (CONCEPT §3.4, D6/D7).
//!
//! The shell enables every first-party non-bar plugin unless it appears in
//! `disabledPlugins[]` (PluginRegistry.isEnabled). An all-off config therefore
//! lists exactly those ids there. Bar-widgets too: empty layout means nothing
//! renders, but loading the component is not "off".

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
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

/// Outcome of a down-window reconciliation (CONCEPT D14).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Supplied renames that matched at least one occurrence in the doc.
    pub renamed: Vec<(String, String)>,
    /// First-party ids dropped from `disabledPlugins[]`: the new pin no
    /// longer ships them and they were not renamed.
    pub pruned: Vec<String>,
    /// Ids the new pin ships that the old pin did not (rename targets
    /// excluded — those are tracked under `renamed`).
    pub appeared: Vec<String>,
    /// True when `appeared` ids were appended to `disabledPlugins[]`,
    /// preserving an all-off selection; false = left absent, i.e. enabled by
    /// upstream default (mixed-selection users keep upstream's posture).
    pub appeared_kept_off: bool,
}

impl ReconcileReport {
    pub fn is_noop(&self) -> bool {
        self.renamed.is_empty() && self.pruned.is_empty() && self.appeared.is_empty()
    }
}

/// Reconcile a user-mutated `shell.json` against a new pin's first-party id
/// set (D14 — the only sanctioned opb write post-bootstrap; call it with the
/// shell down). Pure: takes ownership of `old_doc`, returns the rewritten doc
/// plus a report.
///
/// - renames apply across `bar.layout` entries (string or `{id: …}` object)
///   and `disabledPlugins[]`, mirroring upstream's own jq migrations;
/// - surviving ids keep their persisted state, third-party entries and any
///   unknown keys pass through untouched;
/// - gone ids are pruned from `disabledPlugins[]` only when opb tracks them
///   as first-party of the previous pin;
/// - appeared ids preserve an all-off posture (kept off), else follow the
///   upstream default (on) — both reported.
pub fn reconcile(
    old_doc: serde_json::Value,
    old_ids: &[String],
    new_ids: &[String],
    renames: &[(String, String)],
) -> Result<(serde_json::Value, ReconcileReport)> {
    let mut doc = old_doc;
    let index_of: HashMap<&str, usize> = renames
        .iter()
        .enumerate()
        .map(|(i, (from, _))| (from.as_str(), i))
        .collect();
    let old_set: std::collections::HashSet<&str> =
        old_ids.iter().map(String::as_str).collect();
    let new_set: std::collections::HashSet<&str> =
        new_ids.iter().map(String::as_str).collect();

    // 1) bar.layout entries: strings and {id} objects.
    let mut applied: Vec<bool> = vec![false; renames.len()];
    if let Some(layout) = doc.pointer_mut("/bar/layout").and_then(|l| l.as_object_mut()) {
        for section in layout.values_mut() {
            let Some(entries) = section.as_array_mut() else {
                continue;
            };
            for entry in entries.iter_mut() {
                rename_entry(entry, &index_of, renames, &mut applied);
            }
        }
    }

    // 2) disabledPlugins[].
    let disabled = match doc.get_mut("disabledPlugins") {
        Some(d) => d
            .as_array_mut()
            .context("shell.json disabledPlugins is not an array")?,
        None => bail!("shell.json has no disabledPlugins array"),
    };
    let all_off_before = old_set.iter().all(|id| {
        disabled
            .iter()
            .any(|v| v.as_str() == Some(*id))
    });

    let mut out: Vec<serde_json::Value> = Vec::with_capacity(disabled.len());
    let mut pruned = Vec::new();
    for value in disabled.drain(..) {
        let Some(s) = value.as_str() else {
            out.push(value); // non-string entries pass through untouched
            continue;
        };
        if let Some(&i) = index_of.get(s) {
            applied[i] = true;
            out.push(serde_json::Value::String(renames[i].1.clone()));
        } else if old_set.contains(s) && !new_set.contains(s) {
            pruned.push(s.to_owned()); // gone upstream, not renamed → drop
        } else {
            out.push(value);
        }
    }

    // 3) Appeared ids: keep an all-off posture off, otherwise leave absent.
    let target_set: std::collections::HashSet<&str> =
        renames.iter().map(|(_, to)| to.as_str()).collect();
    let mut appeared: Vec<String> = new_ids
        .iter()
        .filter(|id| !old_set.contains(id.as_str()) && !target_set.contains(id.as_str()))
        .cloned()
        .collect();
    appeared.sort();
    let appeared_kept_off = all_off_before && !appeared.is_empty();
    if appeared_kept_off {
        out.extend(appeared.iter().map(|id| serde_json::Value::String(id.clone())));
    }

    dedupe(&mut out);
    *disabled = out;

    let renamed = renames
        .iter()
        .zip(applied)
        .filter(|(_, hit)| *hit)
        .map(|((from, to), _)| (from.clone(), to.clone()))
        .collect();
    Ok((
        doc,
        ReconcileReport {
            renamed,
            pruned,
            appeared,
            appeared_kept_off,
        },
    ))
}

/// Rewrite one layout entry in place if it names a rename source. Returns
/// whether it matched.
fn rename_entry(
    entry: &mut serde_json::Value,
    index_of: &HashMap<&str, usize>,
    renames: &[(String, String)],
    applied: &mut [bool],
) -> bool {
    match entry {
        serde_json::Value::String(s) => {
            if let Some(&i) = index_of.get(s.as_str()) {
                *s = renames[i].1.clone();
                applied[i] = true;
                true
            } else {
                false
            }
        }
        serde_json::Value::Object(o) => {
            let Some(id) = o.get("id").and_then(|v| v.as_str()) else {
                return false;
            };
            let Some(&i) = index_of.get(id) else {
                return false;
            };
            o.insert("id".to_owned(), serde_json::Value::String(renames[i].1.clone()));
            applied[i] = true;
            true
        }
        _ => false,
    }
}

/// Dedupe by serialized form, preserving first occurrence order.
fn dedupe(values: &mut Vec<serde_json::Value>) {
    let mut seen = std::collections::HashSet::new();
    values.retain(|v| seen.insert(v.to_string()));
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

    // --- reconcile (D14) ---

    use serde_json::json;

    fn ids<const N: usize>(arr: [&str; N]) -> Vec<String> {
        arr.iter().map(|s| s.to_string()).collect()
    }

    fn doc(disabled: &[&str], layout: serde_json::Value) -> serde_json::Value {
        let disabled: Vec<_> = disabled.iter().map(|s| json!(s)).collect();
        json!({
            "version": 1,
            "bar": { "id": "omarchy.bar", "layout": layout },
            "disabledPlugins": disabled,
            "plugins": [],
        })
    }

    fn disabled_of(v: &serde_json::Value) -> Vec<String> {
        v["disabledPlugins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_owned())
            .collect()
    }

    const OLD_IDS: [&str; 3] = ["omarchy.clock", "omarchy.lock", "omarchy.model-usage"];
    const NEW_IDS: [&str; 3] = ["omarchy.agents", "omarchy.clock", "omarchy.lock"];
    const RENAMES: [(&str, &str); 1] = [("omarchy.model-usage", "omarchy.agents")];

    #[test]
    fn rename_applies_across_layout_strings_objects_and_disabled() {
        let d = doc(
            &["omarchy.clock", "omarchy.model-usage"],
            json!({
                "left": ["omarchy.clock"],
                "center": [{ "id": "omarchy.model-usage", "hideTotal": true }],
                "right": [],
            }),
        );
        let (v, report) =
            reconcile(d, &ids(OLD_IDS), &ids(NEW_IDS), &renames(RENAMES)).unwrap();

        assert_eq!(v["bar"]["layout"]["center"][0]["id"], "omarchy.agents");
        // Object settings survive the id rewrite.
        assert_eq!(v["bar"]["layout"]["center"][0]["hideTotal"], true);
        assert_eq!(disabled_of(&v), ["omarchy.clock", "omarchy.agents"]);
        assert_eq!(report.renamed, vec![("omarchy.model-usage".into(), "omarchy.agents".into())]);
        assert!(report.pruned.is_empty());
        // agents is a rename target → not reported as appeared.
        assert!(report.appeared.is_empty());
    }

    #[test]
    fn rename_target_keeps_disabled_state() {
        // model-usage was explicitly off; after rename, agents must be off too.
        let d = doc(&["omarchy.model-usage"], json!({}));
        let (v, _) = reconcile(d, &ids(OLD_IDS), &ids(NEW_IDS), &renames(RENAMES)).unwrap();
        assert_eq!(disabled_of(&v), ["omarchy.agents"]);
    }

    #[test]
    fn unmatched_rename_is_not_reported_as_applied() {
        let d = doc(&["omarchy.clock"], json!({}));
        let renames = renames([("omarchy.never-existed", "omarchy.whatever")]);
        let (_, report) = reconcile(
            d,
            &ids(["omarchy.clock"]),
            &ids(["omarchy.clock"]),
            &renames,
        )
        .unwrap();
        assert!(report.renamed.is_empty());
    }

    #[test]
    fn gone_first_party_id_is_pruned_and_reported() {
        let old = ids(["omarchy.clock", "omarchy.gone"]);
        let new = ids(["omarchy.clock"]);
        let d = doc(&["omarchy.clock", "omarchy.gone"], json!({}));
        let (v, report) = reconcile(d, &old, &new, &[]).unwrap();

        assert_eq!(disabled_of(&v), ["omarchy.clock"]);
        assert_eq!(report.pruned, ["omarchy.gone"]);
    }

    #[test]
    fn unknown_entries_in_disabled_pass_through_untouched() {
        let old = ids(["omarchy.clock"]);
        let new = ids(["omarchy.clock"]);
        let d = doc(&["omarchy.clock", "user.third-party"], json!({}));
        let (v, report) = reconcile(d, &old, &new, &[]).unwrap();

        assert_eq!(disabled_of(&v), ["omarchy.clock", "user.third-party"]);
        assert!(report.is_noop());
    }

    #[test]
    fn appeared_ids_keep_all_off_posture_off() {
        // Every old first-party id disabled → selection was "everything off";
        // a bump must not silently enable the new component.
        let old = ids(["omarchy.clock"]);
        let new = ids(["omarchy.clock", "omarchy.fresh"]);
        let d = doc(&["omarchy.clock"], json!({}));
        let (v, report) = reconcile(d, &old, &new, &[]).unwrap();

        assert_eq!(disabled_of(&v), ["omarchy.clock", "omarchy.fresh"]);
        assert_eq!(report.appeared, ["omarchy.fresh"]);
        assert!(report.appeared_kept_off);
    }

    #[test]
    fn appeared_ids_follow_upstream_default_under_mixed_selection() {
        // lock enabled (not listed), clock off → mixed posture; new components
        // surface at upstream default (on) and are reported.
        let old = ids(["omarchy.clock", "omarchy.lock"]);
        let new = ids(["omarchy.clock", "omarchy.lock", "omarchy.fresh"]);
        let d = doc(&["omarchy.clock"], json!({}));
        let (v, report) = reconcile(d, &old, &new, &[]).unwrap();

        assert_eq!(disabled_of(&v), ["omarchy.clock"]);
        assert_eq!(report.appeared, ["omarchy.fresh"]);
        assert!(!report.appeared_kept_off);
    }

    #[test]
    fn third_party_content_and_unknown_keys_pass_through() {
        let mut d = doc(&[], json!({}));
        d["plugins"] = json!([{ "id": "user.cool-widget", "path": "/x" }]);
        d["customKey"] = json!({ "nested": [1, 2] });
        let (v, _) =
            reconcile(d.clone(), &ids(OLD_IDS), &ids(NEW_IDS), &[]).unwrap();

        assert_eq!(v["plugins"], d["plugins"]);
        assert_eq!(v["customKey"], d["customKey"]);
        assert_eq!(v["version"], 1);
        assert_eq!(v["bar"]["id"], "omarchy.bar");
    }

    #[test]
    fn malformed_disabled_plugins_is_an_error() {
        let d = json!({ "disabledPlugins": "not-an-array" });
        let err = reconcile(d, &ids(OLD_IDS), &ids(NEW_IDS), &[])
            .unwrap_err()
            .to_string();
        assert!(err.contains("not an array"), "got: {err}");

        let d = json!({});
        assert!(reconcile(d, &ids(OLD_IDS), &ids(NEW_IDS), &[]).is_err());
    }

    #[test]
    fn rename_collision_in_disabled_dedupes_preserving_order() {
        // Both source and target were disabled; after mapping both entries
        // collapse into one.
        let old = ids(["omarchy.old-a"]);
        let new = ids(["omarchy.new-b"]);
        let renames = renames([("omarchy.old-a", "omarchy.new-b")]);
        let d = doc(&["omarchy.new-b", "omarchy.old-a"], json!({}));
        let (v, _) = reconcile(d, &old, &new, &renames).unwrap();
        assert_eq!(disabled_of(&v), ["omarchy.new-b"]);
    }

    fn renames<const N: usize>(arr: [(&str, &str); N]) -> Vec<(String, String)> {
        arr.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }
}