//! Atomic filesystem effects: no half-written state ever lands on disk
//! (tmp file + rename; ROADMAP Phase 2 invariant).
//!
//! File writes delegate the core to the battle-tested `atomicwrites` crate.
//! opb adds the two things it lacks: parent-dir creation and a durability
//! fsync of the destination directory after the rename. Symlink flips (D9) are
//! plain std — no crate covers that, and it is only a handful of lines.
#![allow(dead_code)] // consumed by bootstrap (C3+)

use anyhow::{Context, Result};
use atomicwrites::{AtomicFile, OverwriteBehavior};
use std::io::Write;
use std::path::Path;

/// Write `bytes` to `path` atomically: `atomicwrites` stages a temp file in
/// the same directory, we fsync it inside its write callback, then it renames
/// over the target. Parent directories are created first. On any failure the
/// temp file is removed and the target left untouched.
pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("target path has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create {}", parent.display()))?;
    let af = AtomicFile::new(path, OverwriteBehavior::AllowOverwrite);
    af.write(|f| {
        f.write_all(bytes)?;
        f.sync_all()
    })
    .with_context(|| format!("write {}", path.display()))?;
    fsync_dir(parent)?;
    Ok(())
}

/// Atomically repoint `link` (a symlink) at `target` (D9 pin flip): create a
/// temp symlink, rename over `link`. Replaces a previous symlink or creates it
/// on first bootstrap.
pub fn symlink_flip(target: &Path, link: &Path) -> Result<()> {
    let parent = link
        .parent()
        .context("link path has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create {}", parent.display()))?;
    let file = link
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "link".to_owned());
    let tmp = parent.join(format!(".{file}.tmp{}", std::process::id()));
    std::os::unix::fs::symlink(target, &tmp)
        .with_context(|| format!("create symlink {}", tmp.display()))?;
    let result = std::fs::rename(&tmp, link)
        .with_context(|| format!("rename symlink onto {}", link.display()));
    fsync_dir(parent)?;
    result?;
    Ok(())
}

fn fsync_dir(dir: &Path) -> Result<()> {
    std::fs::File::open(dir)?
        .sync_all()
        .context("fsync directory")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn write_lands_content_at_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nested/file.txt");
        write(&target, b"hello").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"hello");
        // No stray tmp files left behind.
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("nested"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "tmp files left behind: {leftovers:?}");
    }

    #[test]
    fn write_replaces_existing_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("file.txt");
        write(&target, b"old").unwrap();
        write(&target, b"new").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }

    #[test]
    fn symlink_flip_creates_and_repoints() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("current");
        let first = dir.path().join("omarchy@aaa");
        let second = dir.path().join("omarchy@bbb");

        symlink_flip(&first, &link).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), first);

        symlink_flip(&second, &link).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), second);
    }
}