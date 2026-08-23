//! Session environment sanity checks (pure: env values in, CheckResult out).

use crate::check::CheckResult;

/// Wayland session present — quickshell cannot attach without it.
pub fn check_wayland(display: Option<&str>) -> CheckResult {
    match display {
        Some(d) if !d.is_empty() => CheckResult::pass_info("WAYLAND_DISPLAY", d),
        _ => CheckResult::fail("WAYLAND_DISPLAY", "not set (no Wayland session)"),
    }
}

/// Desktop environment detection — informational, hyprctl presence is the real signal.
pub fn check_desktop(desktop: Option<&str>) -> CheckResult {
    match desktop {
        Some(d) if d.split(':').any(|c| c == "Hyprland") => {
            CheckResult::pass_info("XDG_CURRENT_DESKTOP", d)
        }
        Some(d) => CheckResult::warn(
            "XDG_CURRENT_DESKTOP",
            format!("session is {d:?}, expected Hyprland"),
        ),
        None => CheckResult::warn("XDG_CURRENT_DESKTOP", "not set"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::Status;

    #[test]
    fn wayland_set_passes() {
        assert_eq!(
            check_wayland(Some("wayland-1")).status,
            Status::Pass(Some("wayland-1".into()))
        );
        assert!(matches!(check_wayland(Some("")).status, Status::Fail(_)));
    }

    #[test]
    fn wayland_unset_fails() {
        assert_eq!(
            check_wayland(None),
            CheckResult::fail("WAYLAND_DISPLAY", "not set (no Wayland session)")
        );
    }

    #[test]
    fn desktop_hyprland_passes() {
        assert_eq!(
            check_desktop(Some("Hyprland")).status,
            Status::Pass(Some("Hyprland".into()))
        );
        // e.g. Hyprland:sway set by some setups
        assert!(matches!(
            check_desktop(Some("Hyprland:uwsm")).status,
            Status::Pass(_)
        ));
    }

    #[test]
    fn desktop_other_warns() {
        assert_eq!(
            check_desktop(Some("GNOME")).status,
            Status::Warn("session is \"GNOME\", expected Hyprland".into())
        );
        assert_eq!(check_desktop(None).status, Status::Warn("not set".into()));
    }
}