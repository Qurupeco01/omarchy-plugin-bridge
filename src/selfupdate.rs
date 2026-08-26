//! `opb update` — update the opb binary itself from the newest GitHub release
//! (self-update). Distinct from `opb pin update`, which moves the upstream pin.
//!
//! Network shells out instead of pulling an HTTP client — the crate already
//! talks to git remotes via `git` subprocesses, and install.sh downloads with
//! `curl` + verifies with `sha256sum`. Reusing both keeps the release binary
//! small and the behavior identical to the documented installer. Replacing the
//! running binary is safe on Linux: a staging copy in the same directory is
//! renamed over the executable, and the running process keeps its old inode.

use anyhow::{bail, Context, Result};
use semver::Version;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::git;

/// opb's own repository — distinct from `git::REMOTE` (the upstream omarchy
/// repo); the crate never hardcodes a second upstream URL, only this one.
const OPB_REMOTE: &str = "https://github.com/Qurupeco01/omarchy-plugin-bridge";

/// How long the release probe may take before giving up: `opb status` and
/// `opb update --check` must not hang on a blackholed network.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Compile-time version of the running binary (`Cargo.toml`).
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Debug, Clone)]
pub struct Release {
    /// Tag as published, e.g. `v0.3.0`.
    pub tag: String,
    /// The same version, parsed.
    pub version: Version,
}

/// Newest semver release on the remote, `None` when the running binary is
/// already current. `tag` may or may not carry a `v` prefix.
fn newer_release(tag: &str) -> Result<Option<Release>> {
    let version = Version::parse(tag.strip_prefix('v').unwrap_or(tag))
        .with_context(|| format!("opb release tag {tag} is not semver"))?;
    let current = Version::parse(current_version())
        .with_context(|| format!("Cargo.toml version {} is not semver", current_version()))?;
    Ok((version > current).then_some(Release {
        tag: tag.to_owned(),
        version,
    }))
}

/// Probe the remote for a newer opb release, under `timeout`. Errors when the
/// remote is unreachable — callers that must not fail on that (the status
/// probe) map errors to "unknown" themselves.
pub fn remote_release(timeout: Duration) -> Result<Option<Release>> {
    newer_release(&git::latest_tag_timeout(OPB_REMOTE, timeout)?)
}

/// `opb update --check` — report whether a newer release exists, no download.
pub fn check() -> Result<()> {
    match remote_release(PROBE_TIMEOUT) {
        Ok(Some(r)) => {
            println!(
                "opb update: {} -> {} available — run `opb update`",
                current_version(),
                r.tag
            );
            Ok(())
        }
        Ok(None) => {
            println!("opb update: {} is the newest release", current_version());
            Ok(())
        }
        Err(e) => Err(e).context("could not check for updates"),
    }
}

/// `opb update` — check → confirm → download → verify → replace. Skips the
/// confirmation when `yes`.
pub fn run(yes: bool) -> Result<()> {
    let Some(release) = remote_release(PROBE_TIMEOUT)? else {
        println!("opb update: {} is the newest release", current_version());
        return Ok(());
    };
    println!(
        "opb update: {} -> {} (latest GitHub release)",
        current_version(),
        release.tag
    );
    if !yes && !super::prompt::confirm("download and replace the opb binary?", true) {
        bail!("aborted by user");
    }

    let exe = std::env::current_exe().context("locate the running opb binary")?;
    // Replace the real file so any symlink at the install path keeps pointing
    // at a fresh binary.
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    if exe.starts_with("/usr/bin") {
        bail!(
            "opb lives at {} — a package-managed install (AUR). Update it there \
             instead of self-updating.",
            exe.display()
        );
    }

    let target = target_triple()?;
    let tmp = std::env::temp_dir().join(format!("opb-selfupdate-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    let result = (|| -> Result<()> {
        let archive = fetch_release(&tmp, &release, &target)?;
        let bin = extract_archive(&archive, &tmp)?;
        install(&bin, &exe)?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmp); // never leave scratch behind
    result?;

    println!(
        "opb update: installed {} — the next invocation runs the new binary",
        release.tag
    );
    Ok(())
}

/// Prebuilt asset triple — only Linux x86_64 is published (release.yml).
fn target_triple() -> Result<String> {
    if std::env::consts::OS != "linux" {
        bail!("prebuilt opb binaries are Linux-only — build from source (see README)");
    }
    Ok(format!("{}-unknown-linux-gnu", std::env::consts::ARCH))
}

/// Download the release tarball + published checksum into `dir`, then verify
/// the tarball against it. Returns the verified archive path.
fn fetch_release(dir: &Path, release: &Release, target: &str) -> Result<PathBuf> {
    let base = format!("{OPB_REMOTE}/releases/download/{}", release.tag);
    let asset = format!("omarchy-plugin-bridge-{}-{target}.tar.gz", release.version);
    for name in [&asset, &format!("{asset}.sha256")] {
        download(&format!("{base}/{name}"), &dir.join(name))?;
    }
    let archive = dir.join(&asset);
    verify_sha256(&archive, &dir.join(format!("{asset}.sha256")))?;
    Ok(archive)
}

fn download(url: &str, dest: &Path) -> Result<()> {
    let out = Command::new("curl")
        .args(["-fsSL", "--retry", "2", "-o"])
        .arg(dest)
        .arg(url)
        .output()
        .with_context(|| format!("spawn curl {url}"))?;
    if !out.status.success() {
        bail!(
            "curl failed for {url}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Verify the tarball matches the published sha256 (the layout install.sh
/// consumes). Shells out to `sha256sum` rather than pulling a hashing crate —
/// consistent with the project's subprocess effects, keeps the binary small.
fn verify_sha256(archive: &Path, checksum_file: &Path) -> Result<()> {
    let raw = std::fs::read_to_string(checksum_file)
        .with_context(|| format!("read {}", checksum_file.display()))?;
    let expected = first_token(&raw)
        .with_context(|| format!("parse {}", checksum_file.display()))?;
    let out = Command::new("sha256sum")
        .arg(archive)
        .output()
        .context("spawn sha256sum")?;
    if !out.status.success() {
        bail!("sha256sum failed for {}", archive.display());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let actual = first_token(&stdout)
        .with_context(|| format!("parse sha256sum output for {}", archive.display()))?;
    if expected != actual {
        bail!("checksum mismatch for {} — refusing a tampered binary", archive.display());
    }
    Ok(())
}

/// First whitespace-delimited token of a `sha256sum` output line.
fn first_token(s: &str) -> Result<&str> {
    s.split_whitespace()
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty checksum line"))
}

/// Extract `archive` and return the `opb` binary inside. Release tarballs
/// carry `omarchy-plugin-bridge-{version}-{target}/opb`, but the binary is
/// located by walking rather than by guessing the inner dir name.
fn extract_archive(archive: &Path, dir: &Path) -> Result<PathBuf> {
    let out = Command::new("tar")
        .args(["-xzf"])
        .arg(archive)
        .current_dir(dir)
        .output()
        .context("spawn tar -xzf")?;
    if !out.status.success() {
        bail!(
            "tar extraction failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .find(|e| e.file_type().is_file() && e.file_name() == "opb")
        .map(|e| e.into_path())
        .ok_or_else(|| anyhow::anyhow!("no opb binary inside {}", archive.display()))
}

/// Replace the running executable with the freshly verified binary. A staging
/// copy in the same directory plus rename is atomic and safe while the old
/// binary is still running: the process keeps its old inode on Linux.
fn install(bin: &Path, exe: &Path) -> Result<()> {
    let parent = exe.parent().context("running binary has no parent dir")?;
    let staging = parent.join(format!(".opb-{}.new", std::process::id()));
    if let Err(e) = std::fs::copy(bin, &staging) {
        let _ = std::fs::remove_file(&staging);
        return Err(e).with_context(|| {
            format!(
                "cannot write next to {} — if this is a package-managed install \
                 (AUR), update it there instead",
                exe.display()
            )
        });
    }
    // The tarball carries the exec bit; the staging copy may not — make it runnable.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("chmod staging binary {}", staging.display()))?;
    std::fs::rename(&staging, exe)
        .with_context(|| format!("replace {}", exe.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_release_detects_a_newer_tag() {
        let r = newer_release("v9.9.9").unwrap().unwrap();
        assert_eq!(r.tag, "v9.9.9");
        assert_eq!(r.version.to_string(), "9.9.9");
    }

    #[test]
    fn newer_release_accepts_unprefixed_tags() {
        assert!(newer_release("99.0.0").unwrap().is_some());
    }

    #[test]
    fn newer_release_is_none_for_current_or_older() {
        assert!(newer_release(&format!("v{}", current_version())).unwrap().is_none());
        assert!(newer_release("v0.0.1").unwrap().is_none());
    }

    #[test]
    fn newer_release_rejects_non_semver_tags() {
        assert!(newer_release("latest").is_err());
        assert!(newer_release("").is_err());
    }

    #[test]
    fn first_token_takes_the_hash() {
        assert_eq!(
            first_token("0123ab  omarchy-plugin-bridge-0.3.0-x86_64-unknown-linux-gnu.tar.gz\n")
                .unwrap(),
            "0123ab"
        );
        assert!(first_token("\n").is_err());
    }

    #[test]
    fn target_triple_matches_release_asset() {
        if std::env::consts::OS != "linux" {
            return;
        }
        assert_eq!(
            target_triple().unwrap(),
            format!("{}-unknown-linux-gnu", std::env::consts::ARCH)
        );
    }

    #[test]
    fn install_replaces_binary_and_sets_exec_bit() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("new-opb");
        let exe = dir.path().join("opb");
        std::fs::write(&bin, b"new-bytes").unwrap();
        std::fs::write(&exe, b"old-bytes").unwrap();

        install(&bin, &exe).unwrap();

        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::read(&exe).unwrap(), b"new-bytes");
        assert_eq!(std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777, 0o755);
        // No staging file left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".opb-"))
            .collect();
        assert!(leftovers.is_empty(), "staging files left behind: {leftovers:?}");
    }

    #[test]
    fn install_errors_when_directory_is_read_only() {
        use std::os::unix::fs::PermissionsExt;
        // Root bypasses permission checks — the scenario cannot be reproduced.
        if std::process::Command::new("id")
            .arg("-u")
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("opb");
        std::fs::write(&exe, b"old").unwrap();
        let bin = dir.path().join("new");
        std::fs::write(&bin, b"new").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        let err = format!("{:#}", install(&bin, &exe).unwrap_err());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(err.contains("package-managed"), "got: {err}");
    }

    #[test]
    fn verify_sha256_accepts_matching_and_rejects_tampered() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("asset.tar.gz");
        std::fs::write(&archive, b"payload").unwrap();
        let sum = Command::new("sha256sum")
            .arg(&archive)
            .output()
            .unwrap()
            .stdout;
        let sum = String::from_utf8_lossy(&sum)
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned();
        let checksum = dir.path().join("asset.tar.gz.sha256");

        std::fs::write(&checksum, format!("{sum}  asset.tar.gz\n")).unwrap();
        verify_sha256(&archive, &checksum).unwrap();

        std::fs::write(&checksum, "0".repeat(64)).unwrap();
        assert!(verify_sha256(&archive, &checksum).is_err());
    }

    #[test]
    fn extract_archive_locates_the_opb_binary() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("omarchy-plugin-bridge-0.9.9-x86_64-unknown-linux-gnu");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("opb"), b"bin").unwrap();
        let archive = dir.path().join("t.tar.gz");
        let out = Command::new("tar")
            .args(["-czf"])
            .arg(&archive)
            .arg("-C")
            .arg(dir.path())
            .arg(inner.file_name().unwrap())
            .output()
            .unwrap();
        assert!(out.status.success());

        let got = extract_archive(&archive, dir.path()).unwrap();
        assert_eq!(got, inner.join("opb"));
    }
}