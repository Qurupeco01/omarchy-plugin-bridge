//! Bare `opb plugin edit` — interactive editor over every plugin. Widgets
//! (kind `bar-widget`) get a section — left/center/right/off; everything else
//! gets on/off. Actions resolve to a forwarded `bin/omarchy plugin
//! enable|disable <id>` so upstream stays the single writer of shell.json via
//! its own IPC (D13). Requires a running shell — mutations are IPC-driven.
//!
//! Upstream persists shell.json asynchronously (FileView setText is
//! non-blocking), so after a forwarded mutation the editor polls the file
//! until the target state lands before rendering the next list — re-reading
//! once would race the write and show a stale state that looks like the
//! action failed.
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
    pub kinds: Vec<String>,
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
                kinds: s.info.kinds.clone(),
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
        wait_for_state(paths, c, action)?;
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

/// Wait for the async shell.json write to land after a forwarded mutation.
/// The IPC call returns "ok" before the shell's FileView write finishes
/// (setText is non-blocking), so a single reload races it and shows a stale
/// state that looks like the action failed. Poll until the target state is
/// on disk; bounded — on timeout the mutation still succeeded over IPC, so
/// just surface the last-known state rather than erroring.
fn wait_for_state(paths: &Paths, c: &Candidate, action: &str) -> Result<()> {
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);
    const BUDGET: std::time::Duration = std::time::Duration::from_secs(2);
    let deadline = std::time::Instant::now() + BUDGET;
    loop {
        let doc = plugin_list::load_doc(paths)?;
        if state_matches(&doc, c, action) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Ok(());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Does the doc already reflect the outcome the action targets? Mirrors
/// [`candidates`] state computation, so the wait converges exactly when the
/// displayed state would flip.
fn state_matches(doc: &serde_json::Value, c: &Candidate, action: &str) -> bool {
    if c.is_widget {
        return if action == "off" {
            plugin_list::placed_section(doc, &c.id).is_none()
        } else {
            plugin_list::placed_section(doc, &c.id) == Some(action)
        };
    }
    let expected = if action == "on" { "on" } else { "off" };
    plugin_list::state_of(doc, &c.kinds, &c.id) == expected
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

    fn candidate(id: &str, is_widget: bool) -> Candidate {
        Candidate {
            id: id.to_owned(),
            origin: "first-party",
            state: "off".to_owned(),
            is_widget,
            kinds: if is_widget {
                vec!["bar-widget".to_owned()]
            } else {
                vec!["service".to_owned()]
            },
        }
    }

    fn temp_paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::from_parts(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
        );
        std::fs::create_dir_all(paths.omarchy_config_dir()).unwrap();
        (dir, paths)
    }

    #[test]
    fn state_matches_tracks_the_action_target() {
        let doc = serde_json::json!({
            "version": 1,
            "disabledPlugins": [],
            "bar": { "layout": {
                "left": ["omarchy.clock"], "center": [], "right": []
            } },
            "plugins": ["cool.panel"],
        });
        let clock = candidate("omarchy.clock", true);
        // Placed left: "off" has not landed yet, "left" already holds.
        assert!(!state_matches(&doc, &clock, "off"));
        assert!(state_matches(&doc, &clock, "left"));
        // Widget not placed: only "off" holds.
        let agents = candidate("omarchy.agents", true);
        assert!(state_matches(&doc, &agents, "off"));
        assert!(!state_matches(&doc, &agents, "center"));
        // First-party service enabled (unlisted): "on" holds, "off" pending.
        let idle = candidate("omarchy.idle", false);
        assert!(state_matches(&doc, &idle, "on"));
        assert!(!state_matches(&doc, &idle, "off"));
        // Third-party panel present in plugins[]: on, not off.
        let panel = Candidate {
            id: "cool.panel".to_owned(),
            origin: "user",
            state: "on".to_owned(),
            is_widget: false,
            kinds: vec!["panel".to_owned()],
        };
        assert!(state_matches(&doc, &panel, "on"));
        assert!(!state_matches(&doc, &panel, "off"));
        // Listing the service disabled flips both ways.
        let mut off_doc = doc;
        off_doc["disabledPlugins"] = serde_json::json!(["omarchy.idle"]);
        assert!(state_matches(&off_doc, &idle, "off"));
        assert!(!state_matches(&off_doc, &idle, "on"));
    }

    #[test]
    fn waits_for_the_async_shell_write_to_land() {
        let (_d, paths) = temp_paths();
        let start_doc = serde_json::json!({
            "version": 1,
            "disabledPlugins": [],
            "bar": { "layout": {
                "left": ["omarchy.clock"], "center": [], "right": []
            } },
            "plugins": [],
        });
        std::fs::write(paths.shell_json(), crate::shelljson::render(&start_doc)).unwrap();
        let clock = candidate("omarchy.clock", true);

        // The shell writes asynchronously: simulate the write landing ~120ms
        // after the IPC call returned.
        let target = serde_json::json!({
            "version": 1,
            "disabledPlugins": [],
            "bar": { "layout": { "left": [], "center": [], "right": [] } },
            "plugins": [],
        });
        let write_path = paths.shell_json();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(120));
            std::fs::write(write_path, crate::shelljson::render(&target)).unwrap();
        });

        let start = std::time::Instant::now();
        wait_for_state(&paths, &clock, "off").unwrap();
        assert!(
            start.elapsed() >= std::time::Duration::from_millis(100),
            "returned before the delayed write could have landed"
        );
        handle.join().unwrap();
        let doc = plugin_list::load_doc(&paths).unwrap();
        assert!(plugin_list::placed_section(&doc, "omarchy.clock").is_none());
    }

    #[test]
    fn noop_action_returns_without_polling() {
        let (_d, paths) = temp_paths();
        let doc = serde_json::json!({
            "version": 1,
            "disabledPlugins": [],
            "bar": { "layout": {
                "left": ["omarchy.clock"], "center": [], "right": []
            } },
            "plugins": [],
        });
        std::fs::write(paths.shell_json(), crate::shelljson::render(&doc)).unwrap();
        let clock = candidate("omarchy.clock", true);

        let start = std::time::Instant::now();
        wait_for_state(&paths, &clock, "left").unwrap();
        assert!(
            start.elapsed() < std::time::Duration::from_millis(100),
            "a no-op must not spin the poll loop"
        );
    }
}