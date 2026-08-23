//! Enabled conflict-relevant components derived from `shell.json`
//! (CONCEPT §3.4 storage rules, mirroring PluginRegistry.isEnabled).
//!
//! Only the three matrix components (§10) are needed here — no conflict row
//! exists for third-party plugins, so bar.layout/plugins[] need no parsing.

use crate::doctor::conflicts::{BAR, NOTIFICATIONS, POLKIT};
use std::collections::HashSet;

/// Which conflict-matrix components are enabled for the given config.
/// Pure: `{}` (stock default) means everything first-party is on — that is the
/// contract, an absent `disabledPlugins` disables nothing.
pub fn enabled_components(config: &serde_json::Value) -> Vec<String> {
    let disabled: HashSet<&str> = config["disabledPlugins"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut out = Vec::new();
    let bar_id = config["bar"]["id"].as_str().unwrap_or(BAR);
    if bar_id == BAR {
        out.push(BAR.to_owned());
    }
    for comp in [NOTIFICATIONS, POLKIT] {
        if !disabled.contains(comp) {
            out.push(comp.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(json: serde_json::Value) -> serde_json::Value {
        json
    }

    #[test]
    fn all_off_config_enables_only_bar() {
        let cfg = crate::shelljson::generate(&[
            "omarchy.lock".to_owned(),
            NOTIFICATIONS.to_owned(),
            POLKIT.to_owned(),
        ]);
        assert_eq!(enabled_components(&cfg), [BAR]);
    }

    #[test]
    fn stock_empty_config_enables_everything_first_party() {
        // No disabledPlugins → nothing disabled (upstream contract).
        assert_eq!(
            enabled_components(&config(serde_json::json!({}))),
            [BAR, NOTIFICATIONS, POLKIT]
        );
    }

    #[test]
    fn removing_a_component_from_disabled_enables_it() {
        let mut cfg = crate::shelljson::generate(&[NOTIFICATIONS.to_owned(), POLKIT.to_owned()]);
        // `select enable` drops the id from disabledPlugins[].
        cfg["disabledPlugins"] = serde_json::json!([POLKIT]);
        assert_eq!(enabled_components(&cfg), [BAR, NOTIFICATIONS]);
    }

    #[test]
    fn custom_bar_id_disables_the_omarchy_bar_row() {
        let mut cfg = crate::shelljson::generate(&[NOTIFICATIONS.to_owned(), POLKIT.to_owned()]);
        cfg["bar"]["id"] = serde_json::json!("mylayout.bar");
        assert_eq!(enabled_components(&cfg), Vec::<String>::new());
    }

    #[test]
    fn explicit_omarchy_bar_id_is_enabled() {
        let cfg = crate::shelljson::generate(&[]);
        // Empty disabledPlugins disables nothing: bar + all first-party on.
        assert_eq!(
            enabled_components(&cfg),
            [BAR, NOTIFICATIONS, POLKIT]
        );
    }
}