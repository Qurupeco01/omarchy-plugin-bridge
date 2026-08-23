//! Thin git process effects — no `git2` (D4): shell out to `git`.

use anyhow::{bail, Context, Result};
use semver::Version;
use std::path::Path;
use std::process::Command;

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
    let out = Command::new("git")
        .args(["ls-remote", "--tags", url])
        .output()
        .context("spawn git ls-remote --tags")?;
    if !out.status.success() {
        bail!(
            "git ls-remote failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse_latest_tag(&String::from_utf8_lossy(&out.stdout))
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