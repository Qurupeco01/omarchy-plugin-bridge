//! `opb up` / `opb down` — launch and stop the pinned shell.
//!
//! Launch goes through the `current` symlink (`quickshell -p <current>/shell`),
//! the same path the opb.conf autostart uses — so process discovery matches
//! both an `opb up`-spawned shell and an autostarted one. Liveness is verified
//! by IPC ping, never the spawn exit code (RESEARCH §6: `omarchy-shell -q`
//! swallows all failures).

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::env;
use crate::paths::Paths;
use crate::pin;

/// The shell config dir. Always spelled through the `current` symlink:
/// quickshell matches IPC instances by exact config path, so every launcher
/// (`opb up`, autostart) and every IPC caller (`omarchy-shell`, keybind
/// dispatches) must agree on one form or they cannot see each other.
pub fn shell_dir(paths: &Paths) -> Result<PathBuf> {
    pin::active_dir(paths)?; // bootstrap validation only
    Ok(paths.current_dir().join("shell"))
}

/// Whether any shell process for the current pin is running. Used by
/// `opb update` to decide whether the down-window needs a `down`/`up` pair.
pub fn is_running(paths: &Paths) -> bool {
    match shell_dir(paths) {
        Ok(dir) => !shell_pids(&dir).unwrap_or_default().is_empty(),
        Err(_) => false,
    }
}

/// pgrep pattern for the shell process. The launched cmdline is exactly
/// `quickshell -p <shell_dir>`; the ipc pinger is `quickshell ipc … -p …` and
/// does not contain the contiguous `quickshell -p` prefix. The path is escaped
/// because `pgrep -f` treats the pattern as an extended regex and the home dir
/// is user-controlled.
fn proc_pattern(shell_dir: &Path) -> String {
    format!("quickshell -p {}", escape_ere(&shell_dir.to_string_lossy()))
}

/// Escape regex metacharacters for use as a literal `pgrep -f` pattern.
fn escape_ere(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// PIDs of running shell processes (empty when none; pgrep exits 1).
fn shell_pids(shell_dir: &Path) -> Result<Vec<u32>> {
    let out = Command::new("pgrep")
        .arg("-f")
        .arg(proc_pattern(shell_dir))
        .output()
        .context("spawn pgrep")?;
    if !out.status.success() {
        return Ok(Vec::new()); // exit 1 = no match
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect())
}

/// IPC liveness probe. The only reliable signal is exit 0 AND output `ok`:
/// a config-path error exits 0 with a different message, a missing instance
/// exits 255.
fn ping_ok(shell_dir: &Path) -> bool {    let out = Command::new("timeout")
        .args(["--kill-after=1s", "2s", "quickshell", "ipc", "-p"])
        .arg(shell_dir)
        .args(["call", "shell", "ping"])
        .output();
    matches!(
        out,
        Ok(o) if o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "ok"
    )
}

/// Launch the shell detached and wait until it answers IPC ping.
pub fn up(paths: &Paths) -> Result<()> {
    let shell_dir = shell_dir(paths)?;
    if !shell_pids(&shell_dir)?.is_empty() {
        println!("opb up: shell is already running");
        return Ok(());
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        bail!("WAYLAND_DISPLAY is unset — not a Wayland session?");
    }
    let mut cmd = Command::new("setsid");
    cmd.args(["--fork", "quickshell", "-p"])
        .arg(&shell_dir)
        .envs(env::for_pin(&paths.current_dir()));
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.spawn().context("spawn shell")?;

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if ping_ok(&shell_dir) {
            println!("opb up: shell alive (IPC ping ok)");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let _ = down(paths); // don't leave a broken spawn behind
    bail!("shell did not answer IPC ping within 10s; the spawned process was stopped")
}

/// Stop the shell: TERM, wait briefly, KILL.
pub fn down(paths: &Paths) -> Result<()> {
    let shell_dir = shell_dir(paths)?;
    let pids = shell_pids(&shell_dir)?;
    if pids.is_empty() {
        println!("opb down: shell is not running");
        return Ok(());
    }
    signal("TERM", &shell_dir);
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !shell_pids(&shell_dir)?.is_empty() {
        std::thread::sleep(Duration::from_millis(100));
    }
    if !shell_pids(&shell_dir)?.is_empty() {
        signal("KILL", &shell_dir);
    }
    println!("opb down: stopped {} quickshell process(es)", pids.len());
    Ok(())
}

fn signal(sig: &str, shell_dir: &Path) {
    let _ = Command::new("pkill")
        .args([format!("-{sig}"), "-f".into()])
        .arg(proc_pattern(shell_dir))
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_pattern_is_contiguous_quickshell_p() {
        let dir = Path::new("/s/current/shell");
        assert_eq!(proc_pattern(dir), "quickshell -p /s/current/shell");
    }

    #[test]
    fn proc_pattern_escapes_regex_metachars_in_home() {
        let dir = Path::new("/home/user+1/.local/share/opb/upstream/current/shell");
        assert_eq!(
            proc_pattern(dir),
            r"quickshell -p /home/user\+1/\.local/share/opb/upstream/current/shell"
        );
    }

    #[test]
    fn not_bootstrapped_errors() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        assert!(shell_dir(&paths).is_err());
    }
}