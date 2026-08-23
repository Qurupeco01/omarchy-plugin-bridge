//! Session-bus conflict scan (CONCEPT §10 matrix).
//! Gated per component: a conflict row is checked only when its omarchy
//! component is enabled (same rule as tier 2). With nothing enabled there are
//! no conflict checks at all — bootstrap-time doctor is tier 1 only.
//! Detection is process ownership on the session bus — never binary presence:
//! an installed-but-idle binary registers no connection and collides with nothing.

use crate::check::CheckResult;

pub const NOTIFICATIONS: &str = "omarchy.notifications";
pub const POLKIT: &str = "omarchy.polkit";
pub const BAR: &str = "omarchy.bar";

/// Process names of daemons that would collide with `omarchy.notifications`.
pub const NOTIFICATION_DAEMONS: &[&str] = &["mako", "dunst", "swaync", "fnott"];

/// Polkit auth agents colliding with `omarchy.polkit`. Prefix-matched: busctl
/// truncates the PROCESS column to 15 chars (`polkit-gnome-authentication-agent-1`
/// shows up as `polkit-gnome-au`).
pub const POLKIT_AGENTS: &[&str] = &["hyprpolkitagent", "polkit-gnome", "polkit-kde"];

/// Other bars — coexistence is fine, our layout starts empty (§10).
pub const OTHER_BARS: &[&str] = &["waybar", "eww"];

/// Pure: extract the PROCESS column from `busctl --user list --no-pager` output.
/// Column 3 (index 2) on every non-header row; works for both `:1.xx` unique
/// names and well-known-name rows. Deduplicated: one process may hold several
/// bus connections and thus appear many times.
pub fn parse_processes(busctl_out: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    busctl_out
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            if fields.next()? == "NAME" {
                return None; // header row
            }
            fields.nth(1).map(str::to_owned)
        })
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Pure: §10 matrix match against live bus processes, restricted to enabled
/// components. Unknown components are ignored.
pub fn scan(processes: &[String], enabled: &[&str]) -> Vec<CheckResult> {
    let mut out = Vec::new();
    for p in processes {
        if enabled.contains(&NOTIFICATIONS)
            && NOTIFICATION_DAEMONS.iter().any(|n| p == n)
        {
            out.push(CheckResult::warn(
                NOTIFICATIONS,
                format!("{p} owns the notifications bus name"),
            ));
        }
        if enabled.contains(&POLKIT) && POLKIT_AGENTS.iter().any(|n| p.starts_with(n)) {
            out.push(CheckResult::warn(
                POLKIT,
                format!("{p} is a polkit auth agent"),
            ));
        }
        if enabled.contains(&BAR) && OTHER_BARS.iter().any(|n| p == n) {
            out.push(CheckResult::info(
                BAR,
                format!("{p} runs alongside; coexistence fine (empty layout)"),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::Status;

    /// Representative `busctl --user list` snapshot (format captured from a live session).
    const SNAPSHOT: &str = "\
NAME                                            PID PROCESS         USER       CONNECTION    UNIT              SESSION DESCRIPTION
:1.3                                            981 Hyprland        qurupeco01 :1.3          session-3.scope   3       -
:1.11                                          1057 hyprpolkitagent qurupeco01 :1.11         user@1000.service -       -
:1.29                                          1241 mako            qurupeco01 :1.29         user@1000.service -       -
:1.99                                          2001 polkit-gnome-au qurupeco01 :1.99         user@1000.service -       -
:1.77                                          1888 waybar          qurupeco01 :1.77         session-3.scope   3       -
";

    #[test]
    fn parses_process_column_skipping_header() {
        assert_eq!(
            parse_processes(SNAPSHOT),
            vec![
                "Hyprland".to_string(),
                "hyprpolkitagent".to_string(),
                "mako".to_string(),
                "polkit-gnome-au".to_string(),
                "waybar".to_string(),
            ]
        );
    }

    #[test]
    fn dedupes_processes_with_multiple_connections() {
        let multi = format!("{SNAPSHOT}:1.78  1888 waybar      qurupeco01 :1.78 session-3.scope 3 -\n");
        let procs = parse_processes(&multi);
        assert_eq!(procs.iter().filter(|p| *p == "waybar").count(), 1);
    }

    #[test]
    fn nothing_enabled_skips_all_conflict_checks() {
        // mako + waybar are live, but no component enabled → no rows.
        let procs = parse_processes(SNAPSHOT);
        assert!(scan(&procs, &[]).is_empty());
    }

    #[test]
    fn checks_only_enabled_components() {
        let procs = parse_processes(SNAPSHOT);
        // Only notifications enabled: polkit agent and waybar must not appear.
        let hits = scan(&procs, &[NOTIFICATIONS]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, NOTIFICATIONS);
        assert!(matches!(hits[0].status, Status::Warn(_)));
    }

    #[test]
    fn all_components_enabled_detects_matrix_conflicts() {
        let procs = parse_processes(SNAPSHOT);
        let hits = scan(&procs, &[NOTIFICATIONS, POLKIT, BAR]);
        let warns: Vec<_> = hits.iter().filter(|c| matches!(c.status, Status::Warn(_))).collect();
        // mako + hyprpolkitagent + polkit-gnome(prefix match)
        assert_eq!(warns.len(), 3);
        assert!(warns.iter().any(|c| c.name == NOTIFICATIONS));
        assert!(warns.iter().any(|c| c.name == POLKIT));
        assert!(hits.iter().any(|c| c.name == BAR));
    }

    #[test]
    fn bar_is_informational_only() {
        let procs = parse_processes(SNAPSHOT);
        let hits = scan(&procs, &[BAR]);
        let bar = hits.iter().find(|c| c.name == BAR).expect("waybar detected");
        assert!(matches!(bar.status, Status::Info(_)));
    }

    #[test]
    fn idle_installed_binary_is_not_a_conflict() {
        // dunst installed but not running: absent from the bus, no hit.
        let procs = vec!["Hyprland".to_string(), "kitty".to_string()];
        assert!(scan(&procs, &[NOTIFICATIONS, POLKIT, BAR]).is_empty());
    }

    #[test]
    fn unknown_enabled_ids_are_ignored() {
        let procs = parse_processes(SNAPSHOT);
        assert!(scan(&procs, &["some.unknown"]).is_empty());
    }
}