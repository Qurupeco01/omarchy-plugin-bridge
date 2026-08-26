//! Bare `opb plugin place` — interactive bar placement. The x-ray supplies
//! the unplaced bar-widgets; selection resolves to a forwarded
//! `bin/omarchy plugin enable <id> --section <left|center|right>` so upstream
//! stays the single writer of shell.json via its own IPC (D13). Requires a
//! running shell — placement is IPC-driven.

use anyhow::{bail, Context, Result};

use crate::paths::Paths;
use crate::plugin_list::{self, Scanned};

/// One placeable widget: id + where its manifest lives.
#[derive(Debug)]
pub struct Candidate {
    pub id: String,
    pub origin: &'static str,
}

const SECTIONS: [&str; 3] = ["left", "center", "right"];

/// Pure: bar-widgets not occupying any layout slot — the placeable set.
pub fn unplaced_widgets(doc: &serde_json::Value, scanned: &[Scanned]) -> Vec<Candidate> {
    scanned
        .iter()
        .filter(|s| s.info.kinds.iter().any(|k| k == "bar-widget"))
        .filter(|s| !plugin_list::is_placed_in_layout(doc, &s.info.id))
        .map(|s| Candidate { id: s.info.id.clone(), origin: s.origin })
        .collect()
}

pub fn run(paths: &Paths) -> Result<()> {
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        bail!(
            "interactive placement needs a terminal — use \
             `opb plugin enable <id> --section left|center|right` instead"
        );
    }
    if !paths.current_dir().is_symlink() {
        bail!("not bootstrapped — run `opb bootstrap` first");
    }
    if !crate::shell::is_running(paths) {
        bail!("the shell is not running — placement goes through its IPC; run `opb up` first");
    }

    let theme = dialoguer::theme::ColorfulTheme::default();
    loop {
        let doc = plugin_list::load_doc(paths)?;
        let scanned = plugin_list::scan_all(paths)?;
        let candidates = unplaced_widgets(&doc, &scanned);
        if candidates.is_empty() {
            println!("nothing to place — every bar widget occupies a slot (see `opb plugin list`)");
            return Ok(());
        }

        let labels: Vec<String> = candidates
            .iter()
            .map(|c| format!("{:<34} [{}]", c.id, c.origin))
            .collect();
        let picked = dialoguer::Select::with_theme(&theme)
            .with_prompt("Place which plugin (esc quits)")
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
        let id = candidates[i].id.clone();

        let section = match dialoguer::Select::with_theme(&theme)
            .with_prompt(format!("Bar section for {id}"))
            .items(&SECTIONS)
            .default(0)
            .interact_opt()
        {
            Err(dialoguer::Error::IO(e)) if e.kind() == std::io::ErrorKind::Interrupted => {
                return Ok(())
            }
            Err(e) => return Err(anyhow::anyhow!("section selection failed: {e}")),
            Ok(None) => continue, // esc on the section → back to the plugin list
            Ok(Some(i)) => SECTIONS[i],
        };

        // Upstream owns placement end-to-end: forward verbatim over IPC.
        place(paths, &id, section)?;

        if !again() {
            return Ok(());
        }
    }
}

fn place(paths: &Paths, id: &str, section: &str) -> Result<()> {
    let pin_dir = paths.current_dir();
    let omarchy = pin_dir.join("bin/omarchy");
    if !omarchy.is_file() {
        bail!("upstream helper missing: {}", omarchy.display());
    }
    let status = std::process::Command::new(&omarchy)
        .arg("plugin")
        .arg("enable")
        .arg(id)
        .arg("--section")
        .arg(section)
        .envs(crate::env::for_pin(&pin_dir))
        .status()
        .with_context(|| format!("spawn {}", omarchy.display()))?;
    if status.success() {
        println!("{id} → {section}");
        Ok(())
    } else {
        bail!("upstream exited nonzero — shell.json may be unchanged");
    }
}

/// After a successful placement, Enter keeps placing (default yes).
fn again() -> bool {
    crate::prompt::confirm("Place another?", true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shelljson;

    fn scan(id: &str, kinds: &[&str], origin: &'static str) -> Scanned {
        Scanned {
            info: shelljson::ManifestInfo {
                id: id.to_owned(),
                kinds: kinds.iter().map(|k| k.to_string()).collect(),
            },
            origin,
        }
    }

    #[test]
    fn only_unplaced_bar_widgets_are_placeable() {
        let doc = serde_json::json!({
            "disabledPlugins": [],
            "bar": { "layout": { "left": ["omarchy.clock"], "center": [], "right": [] } }
        });
        let scanned = vec![
            scan("omarchy.clock", &["bar-widget"], "first-party"),
            scan("omarchy.cpu", &["bar-widget"], "first-party"),
            scan("omarchy.notifications", &["service"], "first-party"),
            scan("cool.panel", &["panel"], "user"),
            scan("omarchy.bar", &["bar"], "first-party"),
        ];

        let got = unplaced_widgets(&doc, &scanned);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].id, "omarchy.cpu");

        // Once placed, it drops out.
        let doc = serde_json::json!({
            "disabledPlugins": [],
            "bar": { "layout": { "left": ["omarchy.clock", "omarchy.cpu"], "center": [], "right": [] } }
        });
        assert!(unplaced_widgets(&doc, &scanned).is_empty());
    }
}
