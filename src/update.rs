//! `opb update` planning core (Phase 4, C2) — what would change?
//!
//! Resolves the update target (`--ref`, defaulting to the newest stable tag),
//! fetches current + target refs into a scratch repo and produces a preview
//! scoped to `shell/` + `bin/` — the only upstream surfaces opb depends on.
//! Effects live in `git.rs`; this module orchestrates and renders.
//!
//! Both refs are fetched into a scratch-local namespace (`refs/opb/{from,to}`)
//! via explicit refspecs: bare-name fetches populate only `FETCH_HEAD` and
//! would leave nothing to diff. The pinned checkout itself is immutable (D9)
//! and shallow — it cannot serve as a diff base, and fetching by commit sha
//! is not portable across remotes. Caveat kept honest: for a moved branch pin
//! the preview diffs against the branch tip, not the recorded commit; tags
//! (the only channel today) always resolve exactly.

use anyhow::{Context, Result};

use crate::git;
use crate::paths::Paths;
use crate::pin::{self, PinLock};

/// Paths the preview is scoped to — the contract set (RESEARCH §1 scope).
const SCOPE: [&str; 2] = ["shell", "bin"];

#[derive(Debug, PartialEq, Eq)]
pub struct UpdatePlan {
    /// Target ref as requested/resolved (e.g. `v4.1.0`).
    pub reference: String,
    /// Ref recorded in `upstream.lock` (diff base).
    pub previous_reference: String,
    /// Commits touching the scoped paths (`sha subject` per entry).
    pub log: Vec<String>,
    /// Diffstat summary over the scoped paths (empty when identical).
    pub stat: String,
}

impl UpdatePlan {
    pub fn is_up_to_date(&self) -> bool {
        self.reference == self.previous_reference
    }
}

/// Build the update plan for `requested` (or the newest stable tag when
/// `None`). Network-touching: ls-remote for the default target, scratch fetch
/// otherwise. The target must clear the quattro support floor before any
/// preview is produced.
pub fn plan(paths: &Paths, requested: Option<&str>) -> Result<UpdatePlan> {
    plan_from(paths, requested, git::REMOTE)
}

fn plan_from(paths: &Paths, requested: Option<&str>, remote: &str) -> Result<UpdatePlan> {
    let lock =
        PinLock::load(paths)?.context("not bootstrapped — run `opb bootstrap` first")?;
    let reference = match requested {
        Some(r) => r.to_owned(),
        None => git::latest_tag(remote)?,
    };
    if reference == lock.reference {
        return Ok(UpdatePlan {
            reference,
            previous_reference: lock.reference,
            log: Vec::new(),
            stat: String::new(),
        });
    }

    let scratch = paths
        .upstream_dir()
        .join(format!(".update-tmp{}", std::process::id()));
    std::fs::create_dir_all(&scratch)
        .with_context(|| format!("create {}", scratch.display()))?;
    let result = build_plan(&scratch, &lock.reference, &reference, remote);
    let _ = std::fs::remove_dir_all(&scratch); // never leave scratch behind
    result
}

fn build_plan(
    scratch: &std::path::Path,
    from: &str,
    to: &str,
    remote: &str,
) -> Result<UpdatePlan> {
    git::init(scratch)?;
    git::remote_add(scratch, remote)?;
    let from_spec = format!("+{from}:refs/opb/from");
    let to_spec = format!("+{to}:refs/opb/to");
    git::fetch(scratch, &[&from_spec, &to_spec])?;
    pin::ensure_floor(
        &git::show_file(scratch, "refs/opb/to", "version")
            .with_context(|| format!("target ref {to} has no version file"))?,
    )
    .with_context(|| format!("target ref {to} rejected"))?;
    let range = "refs/opb/from..refs/opb/to";
    Ok(UpdatePlan {
        reference: to.to_owned(),
        previous_reference: from.to_owned(),
        log: git::log_oneline(scratch, range, &SCOPE)?,
        stat: git::diff_stat(scratch, range, &SCOPE)?,
    })
}

/// Human-readable preview for the confirm step of `opb update`.
#[allow(dead_code)] // consumed by `opb update` CLI wiring in C4
pub fn render_preview(plan: &UpdatePlan) -> String {
    let mut out = format!(
        "opb update: {} -> {}",
        plan.previous_reference, plan.reference
    );
    if plan.is_up_to_date() {
        out.push_str("\n  already up to date\n");
        return out;
    }
    if plan.log.is_empty() {
        out.push_str("\n  no commits touch shell/ or bin/ (contract set unchanged)");
    } else {
        out.push_str("\ncommits touching shell/ or bin/:");
        for line in &plan.log {
            out.push_str(&format!("\n  {line}"));
        }
    }
    if !plan.stat.is_empty() {
        out.push_str("\nfiles changed:");
        for line in plan.stat.lines() {
            out.push_str(&format!("\n  {line}"));
        }
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    /// Build a local origin repo with two tags whose delta touches scoped and
    /// unscoped files. Returns (temp root, origin path).
    fn fixture_origin() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("origin");
        git_init_at(&repo);
        commit(&repo, "base", &[("version", "4.0.0.alpha"), ("shell/a.qml", "a")]);
        git(&repo, &["tag", "v4.0.0"]);
        commit(
            &repo,
            "bump shell",
            &[("shell/b.qml", "b"), ("README.md", "docs")],
        );
        git(&repo, &["tag", "v4.1.0"]);
        commit(&repo, "pre-quattro drop", &[("version", "3.0.1")]);
        git(&repo, &["tag", "v3.0.2"]);
        (dir, repo)
    }

    fn git_init_at(repo: &Path) {
        std::fs::create_dir_all(repo).unwrap();
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["init", "--quiet", "--initial-branch=main"])
            .output()
            .unwrap();
        assert!(out.status.success());
    }

    fn commit(repo: &Path, msg: &str, files: &[(&str, &str)]) {
        for (name, content) in files {
            let p = repo.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
        git(repo, &["add", "."]);
        git(
            repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "-m",
                msg,
            ],
        );
    }

    fn git(repo: &Path, args: &[&str]) {
        let out = Command::new("git").arg("-C").arg(repo).args(args).output();
        assert!(out.is_ok_and(|o| o.status.success()), "git {args:?} failed");
    }

    #[test]
    fn render_preview_up_to_date() {
        let p = UpdatePlan {
            reference: "v4.0.0".into(),
            previous_reference: "v4.0.0".into(),
            log: vec![],
            stat: String::new(),
        };
        let s = render_preview(&p);
        assert!(s.contains("v4.0.0 -> v4.0.0"));
        assert!(s.contains("already up to date"));
    }

    #[test]
    fn render_preview_lists_scoped_commits_and_stat() {
        let p = UpdatePlan {
            reference: "v4.1.0".into(),
            previous_reference: "v4.0.0".into(),
            log: vec!["abcdef0 bump shell".into()],
            stat: " shell/b.qml | 1 +\n 1 file changed".into(),
        };
        let s = render_preview(&p);
        assert!(s.contains("abcdef0 bump shell"));
        assert!(s.contains("shell/b.qml | 1 +"));
    }

    #[test]
    fn render_preview_notes_empty_scoped_diff() {
        let p = UpdatePlan {
            reference: "v4.1.0".into(),
            previous_reference: "v4.0.0".into(),
            log: vec![],
            stat: String::new(),
        };
        assert!(render_preview(&p).contains("no commits touch"));
    }

    fn paths_with_lock(home: &Path, from_ref: &str) -> Paths {
        let paths = Paths::new(home.to_path_buf());
        PinLock::stable(from_ref, "0123456789abcdef0123456789abcdef01234567")
            .save(&paths)
            .unwrap();
        paths
    }

    #[test]
    fn plan_diffs_are_scoped_to_shell_and_bin() {
        let (tmp, origin) = fixture_origin();
        let paths = paths_with_lock(tmp.path(), "v4.0.0");

        let plan =
            plan_from(&paths, Some("v4.1.0"), origin.to_str().unwrap()).unwrap();

        assert_eq!(plan.reference, "v4.1.0");
        assert!(!plan.is_up_to_date());
        assert!(plan.log.iter().any(|l| l.contains("bump shell")));
        assert!(
            !plan.log.iter().any(|l| l.contains("README")),
            "unscoped commits must be filtered"
        );
        // shell/b.qml is in scope; README.md is not.
        assert!(plan.stat.contains("shell/b.qml"));
        assert!(!plan.stat.contains("README.md"), "stat must be scoped too");
    }

    #[test]
    fn plan_refuses_targets_below_the_quattro_floor() {
        let (tmp, origin) = fixture_origin();
        let paths = paths_with_lock(tmp.path(), "v4.0.0");

        let err = format!(
            "{:#}",
            plan_from(&paths, Some("v3.0.2"), origin.to_str().unwrap()).unwrap_err()
        );

        assert!(err.contains("predates quattro"), "got: {err}");
        // No scratch left behind on failure.
        assert!(paths.upstream_dir().read_dir().unwrap().next().is_none());
    }

    #[test]
    fn plan_short_circuits_when_already_on_target() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_with_lock(dir.path(), "v4.0.0");

        let plan = plan_from(&paths, Some("v4.0.0"), "/unused").unwrap();

        assert!(plan.is_up_to_date());
        assert!(plan.log.is_empty());
    }

    #[test]
    fn plan_requires_a_lock() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        let err = plan_from(&paths, Some("v4.1.0"), "/unused")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not bootstrapped"), "got: {err}");
    }

    #[test]
    fn plan_cleans_up_scratch_on_success() {
        let (tmp, origin) = fixture_origin();
        let paths = paths_with_lock(tmp.path(), "v4.0.0");
        plan_from(&paths, Some("v4.1.0"), origin.to_str().unwrap()).unwrap();
        assert!(paths.upstream_dir().read_dir().unwrap().next().is_none());
    }
}
