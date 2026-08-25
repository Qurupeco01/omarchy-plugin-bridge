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

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::Path;

use crate::atomic;
use crate::bootstrap;
use crate::git;
use crate::paths::Paths;
use crate::pin::{self, short, PinLock};
use crate::shelljson;

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
        &git::show_file(scratch, "refs/opb/to", "version").with_context(|| {
            format!(
                "target ref {to} has no version file — pre-quattro trees are \
                 unsupported (plugin architecture starts at v4.0.0)"
            )
        })?,
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

/// `opb update` options (C4).
#[derive(Debug, Default, Clone)]
pub struct UpdateOptions {
    /// Target ref; `None` = newest stable tag.
    pub reference: Option<String>,
    /// Explicit id renames applied during down-window reconciliation
    /// (`--rename old=new`).
    pub renames: Vec<(String, String)>,
/// Skip the interactive confirm.
pub yes: bool,
}

/// `opb update rollback` options.
#[derive(Debug, Default, Clone)]
pub struct RollbackOptions {
    /// Explicit id renames applied during down-window reconciliation.
    pub renames: Vec<(String, String)>,
    /// Skip the interactive confirm.
    pub yes: bool,
}

/// Parse one `--rename old=new` argument.
pub fn parse_rename(arg: &str) -> Result<(String, String)> {
    let (from, to) = arg
        .split_once('=')
        .context("--rename expects OLD=NEW (e.g. --rename omarchy.model-usage=omarchy.agents)")?;
    if from.is_empty() || to.is_empty() {
        bail!("--rename sides must be non-empty: {arg}");
    }
    Ok((from.to_owned(), to.to_owned()))
}

/// Execute the update flow end-to-end (CONCEPT §6):
/// preview → confirm → validated fresh clone → dependency re-run → down-window
/// (down → flip + lock save → shell.json reconciliation → up; restart only
/// happens when the shell was running before). The old pin dir stays on disk
/// for rollback (C5 prunes generations).
pub fn run(paths: &Paths, opts: &UpdateOptions) -> Result<()> {
    let plan = plan(paths, opts.reference.as_deref())?;
    print!("{}", render_preview(&plan));
    if plan.is_up_to_date() {
        return Ok(());
    }
    confirm_msg("proceed?", opts.yes)?;

    let lock = PinLock::load(paths)?.context("not bootstrapped — run `opb bootstrap` first")?;
    let old_pin_dir = pin::active_dir(paths)?;
    let old_ids = shelljson::first_party_non_bar_ids(&old_pin_dir.join("shell/plugins"))?;

    // Dependency re-run gates the flip: a FAIL must stop the update before
    // the pin moves.
    let report = crate::checks::dependency_checks();
    print!("{}", report.render());
    if report.exit_code() >= exit_fail() {
        bail!("dependency checks failed — resolve them before updating");
    }

    // Fresh clone, validated in tmp before it can become the active pin.
    let tmp = paths
        .upstream_dir()
        .join(format!(".clone-tmp{}", std::process::id()));
    git::clone_shallow(git::REMOTE, &plan.reference, &tmp)?;
    let commit_result = (|| -> Result<String> {
        let commit = git::rev_parse_head(&tmp)?;
        bootstrap::ensure_pin_usable(&tmp, &commit)
            .with_context(|| format!("clone of {} is not usable", plan.reference))?;
        Ok(commit)
    })();
    let commit = match commit_result {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
    };
    let new_pin_dir = paths.pin_dir(&commit);

    // Down-window: no other writer exists while the shell is down (D13 carve-out).
    let was_running = crate::shell::is_running(paths);
    if was_running {
        crate::shell::down(paths)?;
    }
    install_clone(&tmp, &new_pin_dir)?;
    atomic::symlink_flip(&new_pin_dir, &paths.current_dir())?;
    PinLock::stable(&plan.reference, &commit)
        .with_previous(&lock)
        .save(paths)?;

    // Sanctioned shell.json write (D14): reconcile against the new pin's ids.
    reconcile_shell_json(paths, &old_ids, &new_pin_dir, &opts.renames)?;

    prune_generations(paths)?;

    if was_running {
        crate::shell::up(paths)?;
    } else {
        println!("opb update: shell was not running — start with `opb up`");
    }
    println!(
        "opb update: now pinned at {} ({}) — previous generation kept for rollback",
        short(&commit),
        plan.reference
    );
    Ok(())
}

/// `opb update rollback` — flip back to the previous generation through the
/// same down-window discipline as an update (including reconciliation against
/// the restored pin's id set). The flip is symmetric: the generation we leave
/// becomes the new previous, so a second rollback undoes it. Retention keeps
/// exactly two generations on disk.
pub fn rollback(paths: &Paths, opts: &RollbackOptions) -> Result<()> {
    let lock = PinLock::load(paths)?.context("not bootstrapped — run `opb bootstrap` first")?;
    let Some(prev) = lock.previous.clone() else {
        bail!(
            "no previous generation to roll back to — rollback exists only \
             right after an update"
        )
    };
    let prev_dir = paths.pin_dir(&prev.commit);
    if !prev_dir.is_dir() {
        bail!(
            "previous generation dir for {} is gone — cannot roll back",
            short(&prev.commit)
        );
    }
    println!(
        "opb update rollback: {} ({}) -> {} ({})",
        short(&lock.commit),
        lock.reference,
        short(&prev.commit),
        prev.reference
    );
    confirm_msg("proceed?", opts.yes)?;

    let leaving_dir = pin::active_dir(paths)?;
    let leaving_ids =
        shelljson::first_party_non_bar_ids(&leaving_dir.join("shell/plugins"))?;

    let was_running = crate::shell::is_running(paths);
    if was_running {
        crate::shell::down(paths)?;
    }
    atomic::symlink_flip(&prev_dir, &paths.current_dir())?;
    PinLock::stable(&prev.reference, &prev.commit)
        .with_previous(&lock)
        .save(paths)?;

    reconcile_shell_json(paths, &leaving_ids, &prev_dir, &opts.renames)?;
    prune_generations(paths)?;

    if was_running {
        crate::shell::up(paths)?;
    } else {
        println!("opb update rollback: shell was not running — start with `opb up`");
    }
    println!("opb update rollback: now pinned at {} ({})", short(&prev.commit), prev.reference);
    Ok(())
}

/// Keep exactly two generations on disk: the active pin and its recorded
/// previous. Any other `omarchy@*` dir is removed. Transient `.clone-tmp*` /
/// `.update-tmp*` dirs belong to their own flows and are left alone.
pub fn prune_generations(paths: &Paths) -> Result<()> {
    let Some(lock) = PinLock::load(paths)? else {
        return Ok(()); // nothing pinned yet; nothing to prune
    };
    let mut keep: Vec<&str> = vec![&lock.commit];
    if let Some(prev) = &lock.previous {
        keep.push(&prev.commit);
    }
    for entry in std::fs::read_dir(paths.upstream_dir())
        .with_context(|| format!("read {}", paths.upstream_dir().display()))?
    {
        let entry = entry.with_context(|| "read upstream dir entry")?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(crate::paths::PIN_DIRNAME_PREFIX) {
            continue;
        }
        let commit = name
            .strip_prefix(crate::paths::PIN_DIRNAME_PREFIX)
            .unwrap_or(&name);
        if keep.contains(&commit) {
            continue;
        }
        std::fs::remove_dir_all(entry.path()).with_context(|| {
            format!("prune old generation {}", entry.path().display())
        })?;
        println!("opb update: pruned old generation {commit}");
    }
    Ok(())
}

/// Move a validated fresh clone into its final generation slot. A dir for
/// this commit can already exist when rolling forward to the generation a
/// previous rollback kept — it is opb-owned state for the exact commit we just
/// re-cloned and validated, so replacing it with the fresh clone is safe.
fn install_clone(tmp: &Path, new_pin_dir: &Path) -> Result<()> {
    if new_pin_dir.is_dir() {
        std::fs::remove_dir_all(new_pin_dir)
            .with_context(|| format!("replace stale generation {}", new_pin_dir.display()))?;
    }
    std::fs::rename(tmp, new_pin_dir)
        .with_context(|| format!("move clone to {}", new_pin_dir.display()))
}

fn exit_fail() -> u8 {
    1
}


fn confirm_msg(prompt: &str, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    println!("{prompt} [y/N]");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        bail!("aborted (no confirmation)");
    }
    match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(()),
        _ => bail!("aborted by user"),
    }
}

/// Down-window reconciliation step. Missing shell.json (deleted by hand?)
/// regenerates the all-off file against the new pin rather than failing —
/// matching what `bootstrap --redo` would produce.
fn reconcile_shell_json(
    paths: &Paths,
    old_ids: &[String],
    new_pin_dir: &std::path::Path,
    renames: &[(String, String)],
) -> Result<()> {
    let new_ids =
        shelljson::first_party_non_bar_ids(&new_pin_dir.join("shell/plugins"))?;
    let (doc, report) = match std::fs::read_to_string(paths.shell_json()) {
        Ok(raw) => {
            let old_doc = serde_json::from_str(&raw)
                .with_context(|| format!("parse {}", paths.shell_json().display()))?;
            shelljson::reconcile(old_doc, old_ids, &new_ids, renames)?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "opb update: {} missing — regenerating all-off config",
                paths.shell_json().display()
            );
            (
                shelljson::generate(&new_ids),
                shelljson::ReconcileReport::default(),
            )
        }
        Err(e) => return Err(e).with_context(|| format!("read {}", paths.shell_json().display())),
    };
    atomic::write(
        &paths.shell_json(),
        shelljson::render(&doc).as_bytes(),
    )?;
    if report.is_noop() {
        println!("opb update: shell.json reconciled (no changes needed)");
    } else {
        println!("{}", render_report(&report));
    }
    Ok(())
}

fn render_report(report: &shelljson::ReconcileReport) -> String {
    use std::fmt::Write;
    let mut s = String::from("opb update: shell.json reconciled\n");
    for (from, to) in &report.renamed {
        let _ = writeln!(s, "  renamed: {from} -> {to}");
    }
    for id in &report.pruned {
        let _ = writeln!(s, "  pruned (no longer shipped): {id}");
    }
    if !report.appeared.is_empty() {
        let posture = if report.appeared_kept_off {
            "kept off (all-off selection preserved)"
        } else {
            "enabled by upstream default"
        };
        let _ = writeln!(s, "  new components ({posture}): {}", report.appeared.join(", "));
    }
    s
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

    #[test]
    fn install_clone_replaces_stale_generation() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("clone-tmp");
        let target = dir.path().join("omarchy@abc");

        // Fresh path: plain move.
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("fresh"), "x").unwrap();
        install_clone(&tmp, &target).unwrap();
        assert_eq!(std::fs::read_to_string(target.join("fresh")).unwrap(), "x");
        assert!(!tmp.exists());

        // Roll-forward onto a kept generation: stale content must go.
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("fresh"), "y").unwrap();
        std::fs::write(target.join("stale"), "old").unwrap();
        install_clone(&tmp, &target).unwrap();
        assert_eq!(std::fs::read_to_string(target.join("fresh")).unwrap(), "y");
        assert!(!target.join("stale").exists());
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

    #[test]
    fn parse_rename_accepts_old_new_pairs() {
        assert_eq!(
            parse_rename("omarchy.model-usage=omarchy.agents").unwrap(),
            ("omarchy.model-usage".to_owned(), "omarchy.agents".to_owned())
        );
        let (_, to) = parse_rename("a=b=c").unwrap();
        assert_eq!(to, "b=c");
    }

    #[test]
    fn parse_rename_rejects_malformed() {
        assert!(parse_rename("no-separator").is_err());
        assert!(parse_rename("=new").is_err());
        assert!(parse_rename("old=").is_err());
    }

    #[test]
    fn render_report_covers_all_sections() {
        let report = shelljson::ReconcileReport {
            renamed: vec![("omarchy.a".into(), "omarchy.b".into())],
            pruned: vec!["omarchy.gone".into()],
            appeared: vec!["omarchy.new1".into(), "omarchy.new2".into()],
            appeared_kept_off: true,
        };
        let s = render_report(&report);
        assert!(s.contains("renamed: omarchy.a -> omarchy.b"));
        assert!(s.contains("pruned (no longer shipped): omarchy.gone"));
        assert!(s.contains("kept off (all-off selection preserved)"));
        assert!(s.contains("omarchy.new1, omarchy.new2"));
    }

    #[test]
    fn render_report_states_upstream_default_posture() {
        let report = shelljson::ReconcileReport {
            appeared: vec!["omarchy.new1".into()],
            appeared_kept_off: false,
            ..Default::default()
        };
        assert!(render_report(&report).contains("enabled by upstream default"));
    }

    // --- generations & rollback ---

use crate::atomic;
    use std::fs;
    use std::path::PathBuf;

    /// Fake pin dir with a minimal plugins tree; returns its commit name.
    fn fake_generation(paths: &Paths, commit: &str, plugin_ids: &[&str]) -> PathBuf {
        let dir = paths.pin_dir(commit);
        fs::create_dir_all(dir.join("shell/plugins/_x")).unwrap();
        fs::write(dir.join("version"), "4.0.0.alpha\n").unwrap();
        for id in plugin_ids {
            let m = serde_json::json!({ "id": id, "kinds": ["service"] });
            fs::write(
                dir.join(format!("shell/plugins/_x/{id}.manifest.json")),
                serde_json::to_vec(&m).unwrap(),
            )
            .unwrap();
        }
        dir
    }

    fn activate(paths: &Paths, reference: &str, commit: &str, prev: Option<(&str, &str)>) {
        let mut lock = PinLock::stable(reference, commit);
        if let Some((r, c)) = prev {
            lock = lock.with_previous(&PinLock::stable(r, c));
        }
        lock.save(paths).unwrap();
        atomic::symlink_flip(&paths.pin_dir(commit), &paths.current_dir()).unwrap();
    }

    #[test]
    fn prune_keeps_active_and_previous_only() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        fake_generation(&paths, "aaa", &[]);
        fake_generation(&paths, "bbb", &[]);
        fake_generation(&paths, "ccc", &[]);
        activate(&paths, "v4.1.0", "bbb", Some(("v4.0.0", "aaa")));

        super::prune_generations(&paths).unwrap();

        assert!(paths.pin_dir("aaa").is_dir());
        assert!(paths.pin_dir("bbb").is_dir());
        assert!(!paths.pin_dir("ccc").exists(), "unreferenced generation must go");
    }

    #[test]
    fn rollback_flips_link_lock_and_shell_json() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        // aaa ships clock+lock, bbb (current) ships clock only and the shell
        // json was reconciled to bbb's set.
        fake_generation(&paths, "aaa", &["omarchy.clock", "omarchy.lock"]);
        fake_generation(&paths, "bbb", &["omarchy.clock"]);
        activate(&paths, "v4.1.0", "bbb", Some(("v4.0.0", "aaa")));
        atomic::write(
            &paths.shell_json(),
            shelljson::render(&shelljson::generate(&["omarchy.clock".to_owned()]))
                .as_bytes(),
        )
        .unwrap();

        super::rollback(
            &paths,
            &super::RollbackOptions { renames: vec![], yes: true },
        )
        .unwrap();

        // Link flipped back; lock now describes aaa with bbb as previous
        // (a second rollback undoes this one).
        assert_eq!(
            std::fs::read_link(paths.current_dir()).unwrap(),
            paths.pin_dir("aaa")
        );
        let lock = PinLock::load(&paths).unwrap().unwrap();
        assert_eq!(lock.commit, "aaa");
        assert_eq!(lock.reference, "v4.0.0");
        let prev = lock.previous.unwrap();
        assert_eq!((prev.reference.as_str(), prev.commit.as_str()), ("v4.1.0", "bbb"));

        // shell.json reconciled against aaa's set: lock re-disabled.
        let raw = fs::read_to_string(paths.shell_json()).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let disabled: Vec<_> =
            cfg["disabledPlugins"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert!(disabled.contains(&"omarchy.lock"));
    }

    #[test]
    fn rollback_without_previous_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        fake_generation(&paths, "aaa", &[]);
        activate(&paths, "v4.0.0", "aaa", None);

        let err = format!(
            "{:#}",
            super::rollback(&paths, &super::RollbackOptions { renames: vec![], yes: true })
                .unwrap_err()
        );
        assert!(err.contains("no previous generation"), "got: {err}");
    }

    #[test]
    fn rollback_when_previous_dir_vanished_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_path_buf());
        fake_generation(&paths, "bbb", &[]);
        // previous recorded but its dir was deleted by hand.
        activate(&paths, "v4.1.0", "bbb", Some(("v4.0.0", "aaa")));

        assert!(
            super::rollback(&paths, &super::RollbackOptions { renames: vec![], yes: true })
                .is_err()
        );
    }
}
