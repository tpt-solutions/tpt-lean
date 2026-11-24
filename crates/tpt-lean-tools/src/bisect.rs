// SPDX-License-Identifier: MIT OR Apache-2.0
//! Git commit bisect.
//!
//! Scope: only project commits. The commit list comes from `git rev-list`. Each probe checks
//! out one commit into a temporary `git worktree` (detached, outside the user's working tree),
//! runs a caller-supplied check there, and removes the worktree again.
//!
//! Not done: dependency versions, Mathlib updates, compiler flags and import changes are listed
//! in [`NOT_IMPLEMENTED`] and are not searched. The search assumes the failure is monotone along
//! the linearized history (`git rev-list --reverse`, which includes merge commits). The good and
//! bad endpoints are assumed, not probed.

use serde::Serialize;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tpt_lean_adapter::{Command, Severity, Toolchain};
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::{Error, ErrorCode, Result};

/// Searches that this module does not perform yet.
pub const NOT_IMPLEMENTED: &[&str] = &[
    "dependency versions",
    "Mathlib updates",
    "compiler flags",
    "import changes",
];

const GIT_TIMEOUT_SECS: u64 = 120;

fn validate_rev(rev: &str) -> Result<&str> {
    let ok = !rev.is_empty()
        && rev.len() <= 200
        && !rev.starts_with('-')
        && !rev.contains("..")
        && rev
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-' | '~' | '^'));
    if ok {
        Ok(rev)
    } else {
        Err(Error::invalid(format!("invalid git revision: {rev:?}")))
    }
}

/// Run `git` with typed arguments (no shell) and return its stdout.
fn git(repo: &Path, args: Vec<String>) -> Result<String> {
    let policy = tpt_lean_adapter::runner::policy_with_timeout(GIT_TIMEOUT_SECS);
    let out = tpt_lean_adapter::run(Path::new("git"), &args, Some(repo), &policy, &CancelToken::new())?;
    if out.timed_out {
        return Err(Error::new(
            ErrorCode::Timeout,
            format!("git {} timed out", args[0]),
        ));
    }
    if !out.success() {
        return Err(Error::new(
            ErrorCode::InvalidArgument,
            format!("git {} failed: {}", args[0], out.stderr.trim()),
        )
        .with_path(repo));
    }
    Ok(out.stdout)
}

/// Commits reachable from `bad` but not from `good`, oldest first.
pub fn commits_between(repo: &Path, good: &str, bad: &str) -> Result<Vec<String>> {
    let range = format!("{}..{}", validate_rev(good)?, validate_rev(bad)?);
    let out = git(repo, vec!["rev-list".into(), "--reverse".into(), range])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

/// One probed commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Probe {
    pub commit: String,
    /// True when the check failed at this commit.
    pub bad: bool,
}

/// Binary search over `commits` (oldest first) for the first commit where `is_bad` is true.
///
/// Returns the index of the first bad commit (None when every commit passes) and the probes
/// made. Assumes that once a commit is bad, every later commit is bad as well.
pub fn search<F>(commits: &[String], mut is_bad: F) -> Result<(Option<usize>, Vec<Probe>)>
where
    F: FnMut(&str) -> Result<bool>,
{
    let mut probes = Vec::new();
    let (mut lo, mut hi) = (0usize, commits.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let bad = is_bad(&commits[mid])?;
        probes.push(Probe {
            commit: commits[mid].clone(),
            bad,
        });
        if bad {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let first = (lo < commits.len()).then_some(lo);
    Ok((first, probes))
}

/// Result of a commit bisect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BisectReport {
    pub good: String,
    pub bad: String,
    pub commits_between: usize,
    pub probes: Vec<Probe>,
    /// First failing commit, oldest-first order. None when no commit in the range fails.
    pub first_bad: Option<String>,
    pub scope: &'static str,
    pub not_done: Vec<&'static str>,
}

/// A detached worktree that is removed on drop, even when a check fails or panics.
struct Worktree {
    repo: PathBuf,
    root: PathBuf,
    path: PathBuf,
}

impl Worktree {
    fn add(repo: &Path, commit: &str) -> Result<Self> {
        if commit.len() < 7 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::invalid(format!("not a commit id: {commit:?}")));
        }
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!("tpt-lean-bisect-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).map_err(|e| Error::from(e).with_path(&root))?;
        let guard = Self {
            repo: repo.to_path_buf(),
            path: root.join("wt"),
            root,
        };
        git(
            repo,
            vec![
                "worktree".into(),
                "add".into(),
                "--detach".into(),
                guard.path.display().to_string(),
                commit.into(),
            ],
        )?;
        Ok(guard)
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        let path = self.path.display().to_string();
        let _ = git(
            &self.repo,
            vec!["worktree".into(), "remove".into(), "--force".into(), path],
        );
        let _ = git(&self.repo, vec!["worktree".into(), "prune".into()]);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Bisect the commits between `good` and `bad` in the git repository at `repo`.
///
/// `check` receives a checkout of one commit and returns `Ok(true)` when it passes there.
/// Worktrees are created under the system temp directory and removed after each probe.
pub fn bisect_commits<F>(repo: &Path, good: &str, bad: &str, mut check: F) -> Result<BisectReport>
where
    F: FnMut(&Path) -> Result<bool>,
{
    let commits = commits_between(repo, good, bad)?;
    let (first, probes) = search(&commits, |commit| {
        let worktree = Worktree::add(repo, commit)?;
        let passed = check(&worktree.path)?;
        Ok(!passed)
    })?;
    Ok(BisectReport {
        good: good.to_string(),
        bad: bad.to_string(),
        commits_between: commits.len(),
        probes,
        first_bad: first.map(|i| commits[i].clone()),
        scope: "git commits between good and bad (oldest first, from git rev-list)",
        not_done: NOT_IMPLEMENTED.to_vec(),
    })
}

/// A typed check for [`bisect_commits`]: run `lake build` in `subdir` of the checkout.
///
/// Passes when the build exits 0 and prints no error diagnostics. Fails with `lean_not_found`
/// when `toolchain.lake` is missing. `subdir` must be a relative path without `..`.
pub fn lake_build_check(
    toolchain: Toolchain,
    subdir: &str,
    timeout_seconds: u64,
) -> Result<impl Fn(&Path) -> Result<bool>> {
    let rel = Path::new(subdir);
    let safe = rel
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if !safe {
        return Err(Error::invalid(format!(
            "subdir must be a relative path without '..': {subdir:?}"
        )));
    }
    if toolchain.lake.is_none() {
        return Err(Error::new(
            ErrorCode::LeanNotFound,
            "lake not found on PATH or in ELAN_HOME/bin",
        ));
    }
    let policy = tpt_lean_adapter::lean_policy(timeout_seconds);
    let subdir = rel.to_path_buf();
    Ok(move |checkout: &Path| {
        let dir = checkout.join(&subdir);
        let result = tpt_lean_adapter::execute(
            &toolchain,
            &Command::LakeBuild { target: None },
            Some(&dir),
            &policy,
            &CancelToken::new(),
        )?;
        Ok(result.output.success() && !result.diagnostics.iter().any(|d| d.severity == Severity::Error))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command as StdCommand;

    fn commits(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("c{i}")).collect()
    }

    #[test]
    fn search_finds_first_bad_with_log_probes() {
        for n in 1..40 {
            for first in 0..n {
                let list = commits(n);
                let (found, probes) = search(&list, |c| {
                    let idx: usize = c[1..].parse().unwrap();
                    Ok(idx >= first)
                })
                .unwrap();
                assert_eq!(found, Some(first), "n={n} first={first}");
                assert!(probes.len() <= 7, "too many probes: {}", probes.len());
            }
        }
    }

    #[test]
    fn search_handles_no_bad_and_empty() {
        let (found, _) = search(&commits(5), |_| Ok(false)).unwrap();
        assert_eq!(found, None);
        let (found, probes) = search(&[], |_| Ok(true)).unwrap();
        assert_eq!(found, None);
        assert!(probes.is_empty());
    }

    #[test]
    fn rejects_option_like_and_range_revisions() {
        assert!(validate_rev("-rf").is_err());
        assert!(validate_rev("a..b").is_err());
        assert!(validate_rev("a b").is_err());
        assert!(validate_rev("HEAD~1").is_ok());
        assert!(validate_rev("v1.2/3").is_ok());
    }

    #[test]
    fn lake_check_rejects_escaping_subdir() {
        let tc = Toolchain {
            lean: None,
            lake: None,
        };
        assert!(lake_build_check(tc.clone(), "../x", 5).is_err());
        assert!(lake_build_check(tc.clone(), "/abs", 5).is_err());
        // Valid subdir but no lake: a clear error instead of a spawn attempt.
        let err = lake_build_check(tc, ".", 5).err().unwrap();
        assert_eq!(err.code(), ErrorCode::LeanNotFound);
    }

    fn git_test(repo: &Path, args: &[&str]) -> String {
        let out = StdCommand::new("git")
            .arg("-C")
            .arg(repo)
            .args([
                "-c",
                "user.name=tpt test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn git_bisect_finds_breaking_commit_and_cleans_worktrees() {
        if StdCommand::new("git").arg("--version").output().is_err() {
            eprintln!("skipping: git not available");
            return;
        }
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let repo = std::env::temp_dir().join(format!("tpt-tools-bisect-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&repo).unwrap();
        git_test(&repo, &["init", "-q"]);

        let mut shas = Vec::new();
        for (i, state) in ["ok", "ok", "broken", "broken", "broken"].iter().enumerate() {
            fs::write(repo.join("flag.txt"), format!("{state}\n# revision {i}\n")).unwrap();
            git_test(&repo, &["add", "flag.txt"]);
            git_test(&repo, &["commit", "-q", "-m", state]);
            shas.push(git_test(&repo, &["rev-parse", "HEAD"]));
        }

        let report = bisect_commits(&repo, &shas[0], &shas[4], |checkout| {
            let text = fs::read_to_string(checkout.join("flag.txt")).map_err(Error::from)?;
            Ok(text.lines().next() == Some("ok"))
        })
        .unwrap();

        assert_eq!(report.commits_between, 4);
        assert_eq!(report.first_bad.as_deref(), Some(shas[2].as_str()));
        assert_eq!(report.not_done, NOT_IMPLEMENTED.to_vec());
        let worktrees = git_test(&repo, &["worktree", "list", "--porcelain"]);
        assert_eq!(worktrees.matches("worktree ").count(), 1, "{worktrees}");

        let _ = fs::remove_dir_all(&repo);
    }
}
