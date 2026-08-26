//! Thin git process effects — no `git2` (D4): shell out to `git`.

use anyhow::{bail, Context, Result};
use semver::Version;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Upstream repository (CONCEPT §1). Phases never embed a second URL.
pub const REMOTE: &str = "https://github.com/basecamp/omarchy";

/// Shallow-clone a single ref (tag or branch) into `dest`.
pub fn clone_shallow(url: &str, reference: &str, dest: &Path) -> Result<()> {
    let out = Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            "--single-branch",
            "--branch",
            reference,
            url,
        ])
        .arg(dest)
        .output()
        .context("spawn git clone")?;
    if !out.status.success() {
        bail!(
            "git clone failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Resolve the checked-out commit of a clone to its full sha.
pub fn rev_parse_head(dir: &Path) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .context("spawn git rev-parse")?;
    if !out.status.success() {
        bail!(
            "git rev-parse failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Highest semver release tag reachable on the remote, resolved by
/// `git ls-remote --tags` (no clone). Pure parse separated for testing.
pub fn latest_tag(url: &str) -> Result<String> {
    parse_latest_tag(&ls_remote(url, None)?)
}

/// `latest_tag` under a deadline — the opb self-update probe must not hang
/// `opb status` on a blackholed network, so the child is killed on expiry.
pub fn latest_tag_timeout(url: &str, timeout: Duration) -> Result<String> {
    parse_latest_tag(&ls_remote(url, Some(timeout))?)
}

/// Run `git ls-remote --tags <url>`, optionally killing the child past a
/// deadline, and return the raw output on success.
fn ls_remote(url: &str, timeout: Option<Duration>) -> Result<String> {
    let mut child = Command::new("git")
        .args(["ls-remote", "--tags", url])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn git ls-remote --tags")?;
    if let Some(t) = timeout {
        let deadline = Instant::now() + t;
        loop {
            if child.try_wait().context("wait for git ls-remote")?.is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!("git ls-remote timed out after {t:?} — cannot reach {url}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let out = child.wait_with_output().context("read git ls-remote output")?;
    if !out.status.success() {
        bail!(
            "git ls-remote failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parse `ls-remote --tags` output (one `sha<TAB>refs/tags/<name>` per line,
/// dereferenced tags additionally as `refs/tags/<name>^{}`) and pick the
/// highest semver tag. `v` prefixes are tolerated; non-semver tags are ignored.
fn parse_latest_tag(output: &str) -> Result<String> {
    let mut best: Option<(Version, String)> = None;
    for line in output.lines() {
        let Some((_, refname)) = line.split_once('\t') else {
            continue;
        };
        if refname.ends_with("^{}") {
            continue; // deref line duplicates the tag's commit
        }
        let name = refname.rsplit('/').next().unwrap_or_default();
        let version = name
            .strip_prefix('v')
            .or(Some(name))
            .and_then(|s| Version::parse(s).ok());
        if let Some(v) = version
            && best.as_ref().is_none_or(|(bv, _)| v > *bv)
        {
            best = Some((v, name.to_owned()));
        }
    }
    best.map(|(_, name)| name)
        .ok_or_else(|| anyhow::anyhow!("no semver release tags found at {REMOTE}"))
}

/// Run `git -C <dir> <args>`, returning trimmed stdout. Bails with stderr on
/// failure. Shared plumbing for the read-only inspection commands below.
fn exec(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .with_context(|| format!("spawn git {}", args.first().unwrap_or(&"?")))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Init an empty repo — scratch space for comparing two remote refs.
pub fn init(dir: &Path) -> Result<()> {
    exec(dir, &["init", "--quiet"]).map(|_| ())
}

/// Register `origin` in a scratch repo.
pub fn remote_add(dir: &Path, url: &str) -> Result<()> {
    exec(dir, &["remote", "add", "origin", url]).map(|_| ())
}

/// Fetch the named refs (tags or branches) from `origin` into a scratch repo.
pub fn fetch(dir: &Path, refs: &[&str]) -> Result<()> {
    let mut args: Vec<&str> = vec!["fetch", "--quiet", "origin"];
    args.extend(refs.iter().copied());
    exec(dir, &args).map(|_| ())
}

/// Read one file out of a fetched ref (`git show <rev>:<path>`).
pub fn show_file(dir: &Path, rev: &str, path: &str) -> Result<String> {
    exec(dir, &["show", &format!("{rev}:{path}")])
}

/// Scoped commit list between two refs, e.g. commits touching only `shell/`
/// and `bin/`. One `sha subject` string per entry.
pub fn log_oneline(dir: &Path, range: &str, paths: &[&str]) -> Result<Vec<String>> {
    let mut args: Vec<&str> = vec!["log", "--oneline", range, "--"];
    args.extend(paths);
    let out = exec(dir, &args)?;
    Ok(if out.is_empty() {
        Vec::new()
    } else {
        out.lines().map(str::to_owned).collect()
    })
}

/// Scoped diffstat summary between two refs.
pub fn diff_stat(dir: &Path, range: &str, paths: &[&str]) -> Result<String> {
    let mut args: Vec<&str> = vec!["diff", "--stat", range, "--"];
    args.extend(paths);
    exec(dir, &args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags() -> &'static str {
        "\
abc\trefs/tags/v3.0.1
def\trefs/tags/v3.0.1^{}
1a2b\trefs/tags/v4.0.0
1a2b\trefs/tags/v4.0.0^{}
zzz\trefs/tags/not-a-version
ccc\trefs/tags/v4.0.0-rc.1
"
    }

    #[test]
    fn picks_highest_release_tag() {
        assert_eq!(parse_latest_tag(tags()).unwrap(), "v4.0.0");
    }

    #[test]
    fn ignores_deref_lines_and_non_semver() {
        let out = "\
x\trefs/tags/foo
y\trefs/tags/v1.2.3^{}
z\trefs/tags/v1.2.3
";
        assert_eq!(parse_latest_tag(out).unwrap(), "v1.2.3");
    }

    #[test]
    fn no_semver_tags_is_an_error() {
        assert!(parse_latest_tag("a\trefs/tags/nope\n").is_err());
        assert!(parse_latest_tag("").is_err());
    }
}