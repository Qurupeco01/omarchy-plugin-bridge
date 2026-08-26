//! Pin lock state — what upstream is pinned to (CONCEPT §6).
//!
//! `upstream.lock` records `{channel, ref, commit}`. Only the `stable` channel
//! exists today (tag pinning); `master`/agile tracking is deferred. `channel`
//! is kept as data so the schema is forward-compatible.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::atomic;
use crate::paths::Paths;

pub const CHANNEL_STABLE: &str = "stable";

/// Oldest upstream line opb supports: the plugin architecture began with
/// omarchy quattro; older trees have no plugin set to reconcile against
/// (CONCEPT §8 support floor).
pub const MIN_SUPPORTED: (u64, u64, u64) = (4, 0, 0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinLock {
    pub channel: String,
    /// Upstream ref as requested: tag or branch. Serialized as `ref`.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Resolved full commit sha the pin dir was cloned at.
    pub commit: String,
    /// Generation we can flip back to (kept on disk by retention pruning).
    /// `None` right after bootstrap; set by every later flip so that
    /// rollback is always a mirror image. Absent field in older lock files
    /// deserializes as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<PreviousPin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviousPin {
    pub reference: String,
    pub commit: String,
}

impl PinLock {
    pub fn stable(reference: impl Into<String>, commit: impl Into<String>) -> Self {
        Self {
            channel: CHANNEL_STABLE.to_owned(),
            reference: reference.into(),
            commit: commit.into(),
            previous: None,
        }
    }

    /// Chain this pin onto `lock` as its rollback generation.
    pub fn with_previous(mut self, lock: &Self) -> Self {
        self.previous = Some(PreviousPin {
            reference: lock.reference.clone(),
            commit: lock.commit.clone(),
        });
        self
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string(self).context("serialize upstream.lock")
    }

    pub fn from_toml(s: &str) -> Result<Self> {
        toml::from_str(s).context("parse upstream.lock")
    }

    /// Load the lock file; `None` when not bootstrapped yet.
    pub fn load(paths: &Paths) -> Result<Option<Self>> {
        let file = paths.lock_file();
        if !file.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&file)
            .with_context(|| format!("read {}", file.display()))?;
        Ok(Some(Self::from_toml(&raw)?))
    }

    /// Persist atomically — never a half-written lock.
    pub fn save(&self, paths: &Paths) -> Result<()> {
        let raw = self.to_toml()?;
        atomic::write(&paths.lock_file(), raw.as_bytes())
    }
}

/// The active pin: lock-preferred, falling back to deriving from the
/// `current` link name (covers a lock-less but linked state). The single
/// resolution path for every command.
pub fn active_pin(paths: &Paths) -> Result<Option<(String, PathBuf)>> {
    if let Some(lock) = PinLock::load(paths)? {
        return Ok(Some((lock.commit.clone(), paths.pin_dir(&lock.commit))));
    }
    match std::fs::read_link(paths.current_dir()) {
        Ok(target) => {
            let name = target
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let commit = name
                .strip_prefix(crate::paths::PIN_DIRNAME_PREFIX)
                .unwrap_or(&name)
                .to_owned();
            Ok(Some((commit, target)))
        }
        Err(_) => Ok(None),
    }
}

/// Resolved active pin dir. Errors when not bootstrapped.
pub fn active_dir(paths: &Paths) -> Result<PathBuf> {
    match active_pin(paths)? {
        Some((_, dir)) => Ok(dir),
        None => bail!("not bootstrapped — run `opb bootstrap` first"),
    }
}

/// Abbreviated commit sha for display (8 chars).
pub(crate) fn short(commit: &str) -> String {
    commit.chars().take(8).collect()
}

/// Tolerant parse of upstream's `version` file content (observed: `4.0.0.alpha`
/// — not valid semver): first three numeric components, `-` treated as a
/// separator, missing components are zero. `None` when unparseable.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut out = [0u64; 3];
    for (i, seg) in s.replace('-', ".").split('.').take(3).enumerate() {
        let digits: String = seg.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return None;
        }
        out[i] = digits.parse().ok()?;
    }
    Some((out[0], out[1], out[2]))
}

/// Enforce the support floor against `version`-file content. Returns the
/// parsed version on success.
pub fn ensure_floor(version_file: &str) -> Result<(u64, u64, u64)> {
    let v = parse_version(version_file).ok_or_else(|| {
        anyhow::anyhow!("unparseable upstream version {:?}", version_file.trim())
    })?;
    if v < MIN_SUPPORTED {
        bail!(
            "upstream version {version_file} predates quattro ({}.{}) — \
             the plugin architecture does not exist before v4.0.0",
            MIN_SUPPORTED.0,
            MIN_SUPPORTED.1
        );
    }
    Ok(v)
}

/// A tree is a supported pin only if its `version` file clears the floor.
pub fn ensure_supported_tree(tree: &Path) -> Result<()> {
    let file = tree.join("version");
    let raw = std::fs::read_to_string(&file)
        .with_context(|| format!("read {}", file.display()))?;
    ensure_floor(&raw)
        .map(|_| ())
        .with_context(|| format!("checkout {} unsupported", tree.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let lock = PinLock::stable("v4.0.0", "0123456789abcdef0123456789abcdef01234567");
        let s = lock.to_toml().unwrap();
        assert_eq!(PinLock::from_toml(&s).unwrap(), lock);
    }

    #[test]
    fn previous_generation_round_trips_and_reads_legacy_files() {
        let next = PinLock::stable("v4.1.0", "aaa")
            .with_previous(&PinLock::stable("v4.0.0", "bbb"));
        let s = next.to_toml().unwrap();
        assert_eq!(PinLock::from_toml(&s).unwrap(), next);

        // A lock written before generations existed must keep loading.
        let legacy = "channel = \"stable\"\nref = \"v4.0.0\"\ncommit = \"abc\"\n";
        let lock = PinLock::from_toml(legacy).unwrap();
        assert_eq!(lock.previous, None);
    }

    #[test]
    fn serializes_ref_as_literal_ref_key() {
        let lock = PinLock::stable("v4.0.0", "abc");
        let s = lock.to_toml().unwrap();
        assert!(s.contains("ref = \"v4.0.0\""), "got: {s}");
    }

    #[test]
    fn channel_defaults_to_stable() {
        assert_eq!(PinLock::stable("x", "y").channel, CHANNEL_STABLE);
    }

    #[test]
    fn rejects_malformed_toml() {
        assert!(PinLock::from_toml("channel = 5").is_err());
    }

    #[test]
    fn load_save_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        assert!(PinLock::load(&paths).unwrap().is_none());

        let lock = PinLock::stable("v4.0.0", "0123456789abcdef0123456789abcdef01234567");
        lock.save(&paths).unwrap();
        assert_eq!(PinLock::load(&paths).unwrap().unwrap(), lock);
    }

    #[test]
    fn parse_version_handles_upstream_shapes() {
        assert_eq!(parse_version("4.0.0.alpha"), Some((4, 0, 0)));
        assert_eq!(parse_version("4.0.0"), Some((4, 0, 0)));
        assert_eq!(parse_version("v4.1.0\n"), Some((4, 1, 0)));
        assert_eq!(parse_version("4.1"), Some((4, 1, 0)));
        assert_eq!(parse_version("4.2.9-x.y"), Some((4, 2, 9)));
    }

    #[test]
    fn parse_version_rejects_garbage() {
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("banana"), None);
        // Non-numeric component before three numbers are read.
        assert_eq!(parse_version("4.0.alpha"), None);
    }

    #[test]
    fn floor_accepts_quattro_and_newer() {
        assert!(ensure_floor("4.0.0.alpha").is_ok());
        assert!(ensure_floor("4.1.0").is_ok());
        assert!(ensure_floor("5.0.0").is_ok());
    }

    /// Pin dir + `current` link + lock (fixture for active_pin).
    fn linked_fixture() -> (tempfile::TempDir, Paths, String) {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        let commit = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0";
        let pin_dir = paths.pin_dir(commit);
        fs::create_dir_all(&pin_dir).unwrap();
        fs::write(pin_dir.join("version"), "4.0.0.alpha\n").unwrap();
        atomic::symlink_flip(&pin_dir, &paths.current_dir()).unwrap();
        (dir, paths, commit.to_owned())
    }

    #[test]
    fn active_pin_prefers_lock_over_link() {
        let (_d, paths, commit) = linked_fixture();
        PinLock::stable("v4.0.0", &commit).save(&paths).unwrap();

        let (got, dir) = active_pin(&paths).unwrap().unwrap();
        assert_eq!(got, commit);
        assert_eq!(dir, paths.pin_dir(&commit));
    }

    #[test]
    fn active_pin_derives_commit_from_link_without_lock() {
        let (_d, paths, commit) = linked_fixture();

        let (got, dir) = active_pin(&paths).unwrap().unwrap();
        assert_eq!(got, commit);
        assert_eq!(dir, paths.pin_dir(&commit));
    }

    #[test]
    fn active_pin_is_none_unbootstrapped_and_active_dir_bails() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        assert!(active_pin(&paths).unwrap().is_none());
        assert!(active_dir(&paths).is_err());
    }

    #[test]
    fn floor_refuses_pre_quattro_and_garbage() {
        let err = ensure_floor("3.9.9").unwrap_err().to_string();
        assert!(err.contains("predates quattro"), "got: {err}");
        assert!(ensure_floor("nonsense").is_err());
    }

    #[test]
    fn supported_tree_check_reads_version_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            ensure_supported_tree(dir.path()).is_err(),
            "missing version file must fail"
        );
        std::fs::write(dir.path().join("version"), "4.0.0.alpha\n").unwrap();
        assert!(ensure_supported_tree(dir.path()).is_ok());
        std::fs::write(dir.path().join("version"), "3.0.1").unwrap();
        assert!(ensure_supported_tree(dir.path()).is_err());
    }
}