//! Bare `opb plugin edit` — interactive editor over every plugin. Widgets
//! (kind `bar-widget`) get a section — left/center/right/off; everything else
//! gets on/off. Actions resolve to a forwarded `bin/omarchy plugin
//! enable|disable <id>` so upstream stays the single writer of shell.json via
//! its own IPC (D13). Requires a running shell — mutations are IPC-driven.
//!
//! The editor is one list, not nested screens: pick a plugin, its action
//! selector appears right below the still-visible list, and the loop keeps
//! going until Esc on the list. Only the bar itself (kind `bar`) is excluded.

use anyhow::{bail, Context, Result};

use crate::paths::Paths;
use crate::plugin_list::{self, Scanned};

/// One editable plugin: id, where its manifest lives, its state, and whether
/// it occupies a bar slot.
#[derive(Debug)]
pub struct Candidate {
    pub id: String,
    pub origin: &'static str,
    pub state: String,
    pub is_widget: bool,
}

const WIDGET_ACTIONS: [&str; 4] = ["left", "center", "right", "off"];
const TOGGLE_ACTIONS: [&str; 2] = ["on", "off"];

/// Pure: every editable plugin (the bar itself excluded) with its state — a
/// section or "off" for widgets, on/off for everything else.
pub fn candidates(doc: &serde_json::Value, scanned: &[Scanned]) -> Vec<Candidate> {
    scanned
        .iter()
        .filter(|s| !s.info.kinds.iter().any(|k| k == "bar"))
        .map(|s| {
            let is_widget = s.info.kinds.iter().any(|k| k == "bar-widget");
            let state = if is_widget {
                plugin_list::placed_section(doc, &s.info.id).unwrap_or("off").to_owned()
            } else {
                plugin_list::state_of(doc, &s.info.kinds, &s.info.id).to_owned()
            };
            Candidate {
                id: s.info.id.clone(),
                origin: s.origin,
                state,
                is_widget,
            }
        })
        .collect()
}

pub fn run(paths: &Paths) -> Result<()> {
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        bail!(
            "interactive editing needs a terminal — use \
             `opb plugin enable <id> [--section left|center|right]` instead"
        );
    }
    if !paths.current_dir().is_symlink() {
        bail!("not bootstrapped — run `opb bootstrap` first");
    }
    if !crate::shell::is_running(paths) {
        bail!("the shell is not running — edits go through its IPC; run `opb up` first");
    }

    let theme = dialoguer::theme::ColorfulTheme::default();
    loop {
        let doc = plugin_list::load_doc(paths)?;
        let scanned = plugin_list::scan_all(paths)?;
        let list = candidates(&doc, &scanned);
        if list.is_empty() {
            println!("no plugins found (see `opb plugin list`)");
            return Ok(());
        }

        let labels: Vec<String> = list
            .iter()
            .map(|c| format!("{:<34} [{}]  {}", c.id, c.origin, c.state))
            .collect();
        let picked = dialoguer::Select::with_theme(&theme)
            .with_prompt("Which plugin (esc quits)")
            .items(&labels)
            .interact_opt();
        let i = match picked {
            Err(dialoguer::Error::IO(e)) if e.kind() == std::io::ErrorKind::Interrupted => {
                return Ok(())
            }
            Err(e) => return Err(anyhow::anyhow!("selection failed: {e}")),
            Ok(None) => return Ok(()),
            Ok(Some(i)) => i,
        };
        let c = &list[i];
        let id = c.id.clone();

        let actions: &[&str] = if c.is_widget { &WIDGET_ACTIONS } else { &TOGGLE_ACTIONS };
        let action_labels: Vec<String> = actions.iter().map(|a| a.to_string()).collect();
        let picked_action = dialoguer::Select::with_theme(&theme)
            .with_prompt(format!("{id} ({}) →", c.state))
            .items(&action_labels)
            .default(0)
            .interact_opt();
        let action = match picked_action {
            Err(dialoguer::Error::IO(e)) if e.kind() == std::io::ErrorKind::Interrupted => {
                return Ok(())
            }
            Err(e) => return Err(anyhow::anyhow!("action selection failed: {e}")),
            Ok(None) => continue, // esc on the action → back to the list
            Ok(Some(i)) => actions[i],
        };

        // Upstream owns mutations end-to-end: forward verbatim over IPC.
        if c.is_widget && action == "off" {
            forward(paths, &["disable", &id])?;
        } else if c.is_widget {
            forward(paths, &["enable", &id, "--section", action])?;
        } else if action == "on" {
            forward(paths, &["enable", &id])?;
        } else {
            forward(paths, &["disable", &id])?;
        }
        println!("{id} → {action}");
    }
}

fn forward(paths: &Paths, args: &[&str]) -> Result<()> {
    let pin_dir = paths.current_dir();
    let omarchy = pin_dir.join("bin/omarchy");
    if !omarchy.is_file() {
        bail!("upstream helper missing: {}", omarchy.display());
    }
    let status = std::process::Command::new(&omarchy)
        .arg("plugin")
        .args(args)
        .envs(crate::env::for_pin(&pin_dir))
        .status()
        .with_context(|| format!("spawn {}", omarchy.display()))?;
    if status.success() {
        Ok(())
    } else {
        bail!("upstream exited nonzero — shell.json may be unchanged");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(id: &str, kinds: &[&str], origin: &'static str) -> Scanned {
        Scanned {
            info: crate::shelljson::ManifestInfo {
                id: id.to_owned(),
                kinds: kinds.iter().map(|k| k.to_string()).collect(),
            },
            origin,
        }
    }

    #[test]
    fn lists_every_plugin_with_widget_vs_on_off_state() {
        let doc = serde_json::json!({
            "disabledPlugins": ["omarchy.background"],
            "bar": { "layout": {
                "left": ["omarchy.clock"],
                "center": [{ "id": "omarchy.media" }],
                "right": []
            } }
        });
        let scanned = vec![
            scan("omarchy.clock", &["bar-widget"], "first-party"),
            scan("omarchy.media", &["bar-widget"], "first-party"),
            scan("omarchy.agents", &["bar-widget"], "first-party"),
            scan("omarchy.background", &["service"], "first-party"),
            scan("omarchy.bar", &["bar"], "first-party"),
        ];

        let got = candidates(&doc, &scanned);
        // The bar itself is not editable.
        assert_eq!(got.len(), 4, "{got:?}");
        let by_id = |id: &str| got.iter().find(|c| c.id == id).unwrap();
        // Widgets report their section — string and object entries alike.
        assert_eq!(by_id("omarchy.clock").state, "left");
        assert!(by_id("omarchy.clock").is_widget);
        assert_eq!(by_id("omarchy.media").state, "center");
        assert!(by_id("omarchy.media").is_widget);
        assert_eq!(by_id("omarchy.agents").state, "off");
        assert!(by_id("omarchy.agents").is_widget);
        // Non-widgets report on/off, never a section.
        assert!(!by_id("omarchy.background").is_widget);
        assert_eq!(by_id("omarchy.background").state, "off");
    }
}