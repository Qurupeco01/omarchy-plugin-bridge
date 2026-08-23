//! Surgical `shell.json` edits — the storage rules of CONCEPT §3.4 as pure
//! functions over a parsed document.
#![allow(dead_code)] // wired into `opb select` in the next commit
//!
//! Storage rules implemented:
//! - first-party (`omarchy.*`) non-bar: enabled unless listed in
//!   `disabledPlugins[]`; disable = list it (+ scrub active spots so state
//!   stays unambiguous), enable = delist it
//! - third-party: enabled ⇔ id appears somewhere (`plugins[]`,
//!   `bar.layout.*`); enable = append, disable = scrub everywhere (never
//!   listed in `disabledPlugins[]`)
//! - bar-widgets additionally live in a bar layout section to render;
//!   enabling places them there unless already present anywhere
//! - the bar itself cannot be toggled off (D7)
//!
//! Invariants preserved by every edit:
//! - unknown top-level fields and existing key order are untouched
//!   (serde_json `preserve_order`)
//! - `disabledPlugins[]` stays sorted + deduped (generated files are sorted;
//!   this makes enable→disable round-trips byte-identical on our own output)
//! - layout/`plugins[]` arrays keep their order; additions append

use anyhow::{bail, Result};
use serde_json::{Value, Map};

pub const BAR_ID: &str = "omarchy.bar";

/// Bar layout section for widget placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Left,
    Center,
    Right,
}

impl Section {
    fn key(self) -> &'static str {
        match self {
            Section::Left => "left",
            Section::Center => "center",
            Section::Right => "right",
        }
    }
}

/// How the plugin renders — decides whether enabling also needs a layout slot.
/// Callers derive it from the manifest's `kinds`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginKind {
    BarWidget,
    Regular,
}

/// First-party namespace is reserved upstream (CONCEPT §3.1), so the prefix
/// alone decides which storage rule applies.
pub fn is_first_party(id: &str) -> bool {
    id.starts_with("omarchy.")
}

/// Result of an edit: whether anything changed and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Changed(&'static str),
    Unchanged(&'static str),
}

impl Outcome {
    pub fn changed(&self) -> bool {
        matches!(self, Outcome::Changed(_))
    }

    pub fn note(&self) -> &'static str {
        match self {
            Outcome::Changed(s) | Outcome::Unchanged(s) => s,
        }
    }
}

/// Enable `id`: delist from `disabledPlugins[]` (first-party) and make it
/// active (`layout` slot for widgets, else `plugins[]` for third-party).
pub fn enable(doc: &mut Value, id: &str, kind: PluginKind, section: Section) -> Result<Outcome> {
    if id == BAR_ID {
        // Always active via bar.id; nothing to do.
        return Ok(Outcome::Unchanged("the bar is always enabled"));
    }
    let mut changed = false;
    if is_first_party(id) && take_id(disabled_mut(doc), id).is_some() {
        changed = true;
    }
    if kind == PluginKind::BarWidget {
        if find_in_layout(doc, id).is_some() {
            return Ok(into_outcome(changed, "already placed in the bar layout"));
        }
        layout_section_mut(doc, section)?.push(Value::String(id.to_owned()));
        return Ok(Outcome::Changed("added to the bar layout"));
    }
    if !is_first_party(id) && !array_contains(plugins_mut(doc), id) {
        plugins_mut(doc).push(Value::String(id.to_owned()));
        return Ok(Outcome::Changed("added to plugins"));
    }
    Ok(into_outcome(changed, "already enabled"))
}

/// Disable `id`: first-party → sorted insert into `disabledPlugins[]`;
/// anything → scrubbed from every active location.
pub fn disable(doc: &mut Value, id: &str) -> Result<Outcome> {
    if id == BAR_ID {
        bail!("the bar cannot be disabled (D7)");
    }
    let mut changed = false;
    for section in [Section::Left, Section::Center, Section::Right] {
        let arr = match layout_section_opt(doc, section) {
            Some(a) => a,
            None => continue,
        };
        if take_id(arr, id).is_some() {
            changed = true;
        }
    }
    if take_id(plugins_mut(doc), id).is_some() {
        changed = true;
    }
    if is_first_party(id) && !array_contains(disabled_mut(doc), id) {
        insert_sorted(disabled_mut(doc), id);
        return Ok(Outcome::Changed(if changed {
            "removed from active spots and disabled"
        } else {
            "listed in disabledPlugins"
        }));
    }
    Ok(into_outcome(changed, "was not enabled"))
}

fn into_outcome(changed: bool, unchanged_note: &'static str) -> Outcome {
    if changed {
        Outcome::Changed("enabled")
    } else {
        Outcome::Unchanged(unchanged_note)
    }
}

/// The `disabledPlugins` array, created when absent.
fn disabled_mut(doc: &mut Value) -> &mut Vec<Value> {
    doc.as_object_mut()
        .expect("shell.json root must be an object")
        .entry("disabledPlugins")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("disabledPlugins must be an array")
}

/// The `plugins` array, created when absent.
fn plugins_mut(doc: &mut Value) -> &mut Vec<Value> {
    doc.as_object_mut()
        .expect("shell.json root must be an object")
        .entry("plugins")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("plugins must be an array")
}

/// `bar.layout.<section>`, creating missing containers along the way.
fn layout_section_mut(doc: &mut Value, section: Section) -> Result<&mut Vec<Value>> {
    let obj = doc
        .as_object_mut()
        .filter(|o| o.contains_key("bar"))
        .ok_or_else(|| anyhow::anyhow!("shell.json has no bar object"))?;
    let bar = obj.get_mut("bar").unwrap();
    let bar_obj = bar
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("bar must be an object"))?;
    let layout = bar_obj
        .entry("layout")
        .or_insert_with(|| Value::Object(Map::new()));
    let layout_obj = layout
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("bar.layout must be an object"))?;
    layout_obj
        .entry(section.key().to_owned())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("bar.layout.{} must be an array", section.key()))
}

fn layout_section_opt(doc: &mut Value, section: Section) -> Option<&mut Vec<Value>> {
    let bar = doc.get_mut("bar")?.as_object_mut()?;
    let layout = bar.get_mut("layout")?.as_object_mut()?;
    layout.get_mut(section.key())?.as_array_mut()
}

/// Find which layout section holds `id`, if any.
fn find_in_layout(doc: &Value, id: &str) -> Option<Section> {
    let layout = doc.get("bar")?.get("layout")?.as_object()?;
    for key in ["left", "center", "right"] {
        if layout
            .get(key)
            .and_then(|v| v.as_array())
            .is_some_and(|a| array_contains(a, id))
        {
            return Some(match key {
                "left" => Section::Left,
                "center" => Section::Center,
                _ => Section::Right,
            });
        }
    }
    None
}

fn array_contains(arr: &[Value], id: &str) -> bool {
    arr.iter().any(|v| v.as_str() == Some(id))
}

/// Remove the first occurrence of `id`; true when found.
fn take_id(arr: &mut Vec<Value>, id: &str) -> Option<()> {
    let pos = arr.iter().position(|v| v.as_str() == Some(id))?;
    arr.remove(pos);
    Some(())
}

/// Sorted, dedup-preserving insertion (invariant: disabledPlugins is sorted).
fn insert_sorted(arr: &mut Vec<Value>, id: &str) {
    let pos = arr.partition_point(|v| v.as_str().is_some_and(|s| s < id));
    if arr.get(pos).and_then(|v| v.as_str()) != Some(id) {
        arr.insert(pos, Value::String(id.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shelljson;

    fn all_off_doc() -> Value {
        serde_json::from_value(shelljson::generate(&[
            "omarchy.clock".to_owned(),
            "omarchy.lock".to_owned(),
        ]))
        .unwrap()
    }

    fn clock_enabled_doc(section: Section) -> Value {
        let mut doc = all_off_doc();
        enable(&mut doc, "omarchy.clock", PluginKind::BarWidget, section).unwrap();
        doc
    }

    fn section_of(doc: &Value, s: Section) -> Vec<String> {
        doc["bar"]["layout"][s.key()]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn first_party_enable_delists_from_disabled_plugins() {
        let mut doc = all_off_doc();
        let out = enable(
            &mut doc,
            "omarchy.lock",
            PluginKind::Regular,
            Section::Right,
        )
        .unwrap();
        assert!(out.changed());
        let disabled: Vec<_> = doc["disabledPlugins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(disabled, ["omarchy.clock"]);
        // Regular first-party does not touch plugins[] or layout.
        assert!(doc["plugins"].as_array().unwrap().is_empty());
        assert!(section_of(&doc, Section::Right).is_empty());
    }

    #[test]
    fn widget_enable_places_in_requested_section() {
        let doc = clock_enabled_doc(Section::Center);
        assert!(!doc["disabledPlugins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some("omarchy.clock")));
        assert_eq!(section_of(&doc, Section::Center), ["omarchy.clock"]);
        assert!(section_of(&doc, Section::Left).is_empty());
        assert!(section_of(&doc, Section::Right).is_empty());
    }

    #[test]
    fn widget_already_in_layout_is_not_moved_or_duplicated() {
        let mut doc = clock_enabled_doc(Section::Left);
        let out = enable(
            &mut doc,
            "omarchy.clock",
            PluginKind::BarWidget,
            Section::Right,
        )
        .unwrap();
        assert_eq!(out, Outcome::Unchanged("already placed in the bar layout"));
        assert_eq!(section_of(&doc, Section::Left), ["omarchy.clock"]);
        assert!(section_of(&doc, Section::Right).is_empty());
    }

    #[test]
    fn third_party_widget_enable_only_touches_layout() {
        let mut doc = all_off_doc();
        enable(&mut doc, "my.widget", PluginKind::BarWidget, Section::Left).unwrap();
        assert_eq!(section_of(&doc, Section::Left), ["my.widget"]);
        assert!(doc["disabledPlugins"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v.as_str() != Some("my.widget")));
        assert!(doc["plugins"].as_array().unwrap().is_empty());
    }

    #[test]
    fn third_party_regular_enable_appends_to_plugins() {
        let mut doc = all_off_doc();
        enable(&mut doc, "cool.panel", PluginKind::Regular, Section::Right).unwrap();
        assert_eq!(
            doc["plugins"].as_array().unwrap().len(),
            1,
            "got: {doc}"
        );
        assert_eq!(doc["plugins"][0], "cool.panel");
    }

    #[test]
    fn bar_enable_is_unchanged() {
        let mut doc = all_off_doc();
        let out = enable(
            &mut doc,
            BAR_ID,
            PluginKind::Regular,
            Section::Right,
        )
        .unwrap();
        assert_eq!(out, Outcome::Unchanged("the bar is always enabled"));
    }

    #[test]
    fn bar_disable_refused() {
        let mut doc = all_off_doc();
        assert!(disable(&mut doc, BAR_ID).is_err());
    }

    #[test]
    fn first_party_disable_inserts_sorted_and_scrubs_active_spots() {
        let mut doc = clock_enabled_doc(Section::Right); // clock active, not in disabledPlugins
        let out = disable(&mut doc, "omarchy.clock").unwrap();
        assert!(out.changed());
        let disabled: Vec<_> = doc["disabledPlugins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        // Sorted position between omarchy.lock? No: clock < lock alphabetically… c > l? 'c' < 'l'.
        assert_eq!(disabled, ["omarchy.clock", "omarchy.lock"]);
        assert!(section_of(&doc, Section::Right).is_empty());
        assert!(doc["plugins"].as_array().unwrap().is_empty());
    }

    #[test]
    fn third_party_disable_scrubs_everywhere_and_never_disables_lists() {
        let mut doc = all_off_doc();
        enable(&mut doc, "cool.panel", PluginKind::Regular, Section::Right).unwrap();
        let out = disable(&mut doc, "cool.panel").unwrap();
        assert!(out.changed());
        assert!(doc["plugins"].as_array().unwrap().is_empty());
        assert!(doc["disabledPlugins"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v.as_str() != Some("cool.panel")));
    }

    #[test]
    fn disabling_absent_third_party_is_unchanged() {
        let mut doc = all_off_doc();
        let out = disable(&mut doc, "ghost.plugin").unwrap();
        assert_eq!(out, Outcome::Unchanged("was not enabled"));
    }

    #[test]
    fn unknown_top_level_fields_survive_every_edit() {
        let mut doc = all_off_doc();
        doc["customField"] = serde_json::json!({ "nested": [1, 2] });
        let before = doc.clone();
        enable(&mut doc, "omarchy.lock", PluginKind::Regular, Section::Right).unwrap();
        disable(&mut doc, "omarchy.lock").unwrap();
        assert_eq!(doc["customField"], before["customField"]);
    }

    #[test]
    fn key_order_preserved_through_edits() {
        // Hand-built doc with keys deliberately NOT in generated order.
        let raw = r#"{"zzz":1,"disabledPlugins":["omarchy.clock","omarchy.lock"],"aaa":2,"bar":{"id":"omarchy.bar","layout":{"right":[],"left":[],"center":[]}},"plugins":[]}"#;
        let mut doc: Value = serde_json::from_str(raw).unwrap();
        enable(&mut doc, "omarchy.lock", PluginKind::Regular, Section::Right).unwrap();
        let rendered = shelljson::render(&doc);
        let zzz = rendered.find("\"zzz\"").unwrap();
        let aaa = rendered.find("\"aaa\"").unwrap();
        assert!(zzz < aaa, "original key order lost: {rendered}");
        let right = rendered.find("\"right\"").unwrap();
        let left = rendered.find("\"left\"").unwrap();
        assert!(right < left, "layout key order lost: {rendered}");
    }

    #[test]
    fn round_trip_on_generated_doc_is_byte_identical() {
        let original = shelljson::render(&shelljson::generate(&[
            "omarchy.clock".to_owned(),
            "omarchy.lock".to_owned(),
        ]));

        let mut doc = all_off_doc();
        enable(&mut doc, "omarchy.clock", PluginKind::BarWidget, Section::Right).unwrap();
        disable(&mut doc, "omarchy.clock").unwrap();
        assert_eq!(shelljson::render(&doc), original);

        let mut doc = all_off_doc();
        enable(&mut doc, "cool.panel", PluginKind::Regular, Section::Left).unwrap();
        disable(&mut doc, "cool.panel").unwrap();
        assert_eq!(shelljson::render(&doc), original);

        // Idempotence: repeated same-direction edits change nothing further,
        // and a full off→on→off cycle lands back on the generated state.
        let mut doc = all_off_doc();
        enable(&mut doc, "omarchy.lock", PluginKind::Regular, Section::Left).unwrap();
        let once = shelljson::render(&doc);
        enable(&mut doc, "omarchy.lock", PluginKind::Regular, Section::Left).unwrap();
        assert_eq!(shelljson::render(&doc), once);
        disable(&mut doc, "omarchy.lock").unwrap();
        disable(&mut doc, "omarchy.lock").unwrap();
        let mut fresh = all_off_doc();
        disable(&mut fresh, "omarchy.lock").unwrap();
        assert_eq!(shelljson::render(&doc), shelljson::render(&fresh));
    }

    #[test]
    fn missing_containers_are_created_leniently() {
        // Hand-trimmed doc without disabledPlugins/plugins keys.
        let mut doc: Value = serde_json::from_str(
            r#"{"version":1,"bar":{"id":"omarchy.bar","layout":{"left":[],"center":[],"right":[]}}}"#,
        )
        .unwrap();
        enable(&mut doc, "omarchy.lock", PluginKind::Regular, Section::Right).unwrap();
        disable(&mut doc, "omarchy.lock").unwrap();
        assert!(doc["disabledPlugins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some("omarchy.lock")));
    }

    #[test]
    fn no_bar_object_is_an_error_for_widget_placement() {
        let mut doc: Value = serde_json::from_str(r#"{"version":1}"#).unwrap();
        let err = enable(&mut doc, "w", PluginKind::BarWidget, Section::Right).unwrap_err();
        assert!(err.to_string().contains("no bar object"), "got: {err}");
    }

    #[test]
    fn is_first_party_by_reserved_namespace() {
        assert!(is_first_party("omarchy.lock"));
        assert!(!is_first_party("third.party"));
        assert!(!is_first_party("omarchish")); // prefix must be the full segment
    }
}
