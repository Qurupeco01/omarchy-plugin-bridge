//! Process environment for spawning anything inside the pinned tree
//! (CONCEPT §4 Environment handling): `OMARCHY_PATH` set to the pin dir,
//! `bin/` prepended to PATH so every `omarchy-*` helper resolves.

use std::path::Path;

/// Env pairs per the D-env rules, applied on top of the inherited environment.
pub fn for_pin(pin_dir: &Path) -> Vec<(String, String)> {
    build(pin_dir, &std::env::var("PATH").unwrap_or_default())
}

/// Pure: pairs for an explicit inherited PATH.
pub fn build(pin_dir: &Path, current_path: &str) -> Vec<(String, String)> {
    vec![
        (
            "OMARCHY_PATH".to_owned(),
            pin_dir.to_string_lossy().into_owned(),
        ),
        ("PATH".to_owned(), prepend_bin(pin_dir, current_path)),
    ]
}

/// `pin/bin:` prepended to the existing PATH.
fn prepend_bin(pin_dir: &Path, current_path: &str) -> String {
    let mut entries = vec![pin_dir.join("bin")];
    entries.extend(std::env::split_paths(current_path));
    std::env::join_paths(entries)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| pin_dir.join("bin").to_string_lossy().into_owned())
}

/// The shell fragment spelling the same contract as [`build`]: env exports
/// ending in `;`, ready to prefix an `exec …` inside a `sh -c '…'`. Used by
/// the keys.lua writer; the Lua twin lives in hypr's `opb_exec` (keep in
/// sync — same exports, same order).
pub fn shell_exports(pin_dir: &Path) -> String {
    format!(
        "export OMARCHY_PATH=\"{pin}\"; export PATH=\"{pin}/bin:$PATH\";",
        pin = pin_dir.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepend_bin_prefixes_pin_bin() {
        let pin = Path::new("/p/omarchy@abc");
        assert_eq!(prepend_bin(pin, "/usr/bin:/bin"), "/p/omarchy@abc/bin:/usr/bin:/bin");
    }

    #[test]
    fn build_sets_omarchy_path_and_prepends() {
        let pin = Path::new("/p/omarchy@abc");
        let env = build(pin, "/usr/bin");
        assert_eq!(env[0], ("OMARCHY_PATH".to_owned(), "/p/omarchy@abc".to_owned()));
        assert_eq!(env[1], ("PATH".to_owned(), "/p/omarchy@abc/bin:/usr/bin".to_owned()));
    }
}
