// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bisect over three axes: project git commits, the imports of one file, and Lean options.
//!
//! * `commits` (default): the commit list comes from `git rev-list`. Each probe checks out one
//!   commit into a temporary `git worktree` (detached, outside the user's working tree), runs a
//!   caller-supplied check there, and removes the worktree again.
//! * `imports`: the candidates are the import lines of one project file, in file order. Probe `k`
//!   keeps the first `k` imports and removes the others. The first probe that fails names the
//!   import that breaks the build.
//! * `flags`: the candidates are Lean options (`name=value`, or `name` for `true`) in the order
//!   given. Probe `k` writes the first `k` as `set_option` lines after the imports.
//!
//! The search assumes the failure is monotone along the candidate order (a later probe fails
//! whenever an earlier one does). The good endpoint (no candidate applied) is assumed, not
//! probed, and the bad endpoint is not probed either.
//!
//! Not done: dependency versions and Mathlib updates are listed in [`NOT_IMPLEMENTED`].
//!
//! The `imports` and `flags` checks ([`lake_build_variant_check`]) rewrite the target file for
//! each probe and restore the original text afterwards, also on error. They are the only part of
//! this module that touches the project tree.

use serde::Serialize;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tpt_lean_adapter::{Command, Severity, Toolchain};
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::{Error, ErrorCode, Result};
use tpt_lean_syntax::Import;

/// Searches that this module does not perform yet.
pub const NOT_IMPLEMENTED: &[&str] = &["dependency versions", "Mathlib updates"];

const GIT_TIMEOUT_SECS: u64 = 120;

/// What a bisect searches over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Commits,
    Imports,
    Flags,
}

impl Axis {
    /// Parse a `--axis` value. Unknown values are an invalid argument.
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "commits" => Ok(Self::Commits),
            "imports" => Ok(Self::Imports),
            "flags" => Ok(Self::Flags),
            other => Err(Error::invalid(format!(
                "unknown --axis {other:?}; expected commits, imports or flags"
            ))),
        }
    }

    /// The name used in reports and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Self::Commits => "commits",
            Self::Imports => "imports",
            Self::Flags => "flags",
        }
    }
}

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
    let (first, raw) = search_positions(commits.len(), |i| is_bad(&commits[i]))?;
    let probes = raw
        .into_iter()
        .map(|(i, bad)| Probe {
            commit: commits[i].clone(),
            bad,
        })
        .collect();
    Ok((first, probes))
}

/// Binary search over positions `0..len` for the first position where `is_bad` is true.
/// Returns the first bad position (None when there is none) and `(position, bad)` per probe.
fn search_positions<F>(len: usize, mut is_bad: F) -> Result<(Option<usize>, Vec<(usize, bool)>)>
where
    F: FnMut(usize) -> Result<bool>,
{
    let mut probes = Vec::new();
    let (mut lo, mut hi) = (0usize, len);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let bad = is_bad(mid)?;
        probes.push((mid, bad));
        if bad {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let first = (lo < len).then_some(lo);
    Ok((first, probes))
}

/// Result of a commit bisect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BisectReport {
    pub axis: &'static str,
    pub good: String,
    pub bad: String,
    pub commits_between: usize,
    pub probes: Vec<Probe>,
    /// First failing commit, oldest-first order. None when no commit in the range fails.
    pub first_bad: Option<String>,
    pub scope: &'static str,
    pub not_done: Vec<&'static str>,
}

/// One probe of an `imports` or `flags` bisect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemProbe {
    /// How many candidates were applied in this probe (the first `enabled` of the list).
    pub enabled: usize,
    /// The candidate applied last in this probe.
    pub item: String,
    /// True when the check failed with this prefix applied.
    pub bad: bool,
}

/// Result of an `imports` or `flags` bisect. `first_bad` names the candidate that makes the check
/// fail: the last one added to the smallest failing prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemBisectReport {
    pub axis: &'static str,
    /// Project-relative file whose imports or flags were varied.
    pub file: String,
    pub candidates: Vec<String>,
    pub probes: Vec<ItemProbe>,
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
        axis: Axis::Commits.name(),
        good: good.to_string(),
        bad: bad.to_string(),
        commits_between: commits.len(),
        probes,
        first_bad: first.map(|i| commits[i].clone()),
        scope: "git commits between good and bad (oldest first, from git rev-list)",
        not_done: NOT_IMPLEMENTED.to_vec(),
    })
}

fn item_scope(axis: Axis) -> &'static str {
    match axis {
        Axis::Imports => {
            "imports of the file in file order. A probe keeps the first k imports and removes the rest; \
             the state with no imports is assumed to pass"
        }
        Axis::Flags => {
            "Lean options in the order given. A probe sets the first k as set_option lines after the \
             imports; the state with none is assumed to pass"
        }
        Axis::Commits => "git commits between good and bad (oldest first, from git rev-list)",
    }
}

/// Bisect `candidates` (applied in order) for the smallest prefix whose check fails.
///
/// `check` receives the applied prefix and returns `Ok(true)` when it passes. The first failing
/// candidate is the last one of the smallest failing prefix. Assumes monotone failure.
pub fn bisect_items<F>(axis: Axis, file: &str, candidates: &[String], mut check: F) -> Result<ItemBisectReport>
where
    F: FnMut(&[String]) -> Result<bool>,
{
    let (first, raw) = search_positions(candidates.len(), |i| Ok(!check(&candidates[..=i])?))?;
    Ok(ItemBisectReport {
        axis: axis.name(),
        file: file.to_string(),
        candidates: candidates.to_vec(),
        probes: raw
            .into_iter()
            .map(|(i, bad)| ItemProbe {
                enabled: i + 1,
                item: candidates[i].clone(),
                bad,
            })
            .collect(),
        first_bad: first.map(|i| candidates[i].clone()),
        scope: item_scope(axis),
        not_done: NOT_IMPLEMENTED.to_vec(),
    })
}

/// Validate a project-relative `.lean` path (no absolute paths, no `..`) and return the full path.
fn project_lean_file(root: &Path, rel_file: &str) -> Result<PathBuf> {
    let rel = Path::new(rel_file);
    let safe = rel
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if !safe || !rel_file.ends_with(".lean") || rel_file.len() < 6 {
        return Err(Error::invalid(format!(
            "file must be a relative path to a .lean file without '..': {rel_file:?}"
        )));
    }
    Ok(root.join(rel))
}

/// The module names of the imports of a project file, in file order (the `imports` candidates).
pub fn file_import_candidates(root: &Path, rel_file: &str) -> Result<Vec<String>> {
    let path = project_lean_file(root, rel_file)?;
    let text = fs::read_to_string(&path).map_err(|e| Error::from(e).with_path(&path))?;
    Ok(tpt_lean_syntax::parse(&text)
        .imports
        .into_iter()
        .map(|i| i.module)
        .collect())
}

/// Parse a `name=value` option (or `name`, meaning `true`). Names and values are restricted to
/// characters that are safe to write into Lean source.
pub fn parse_flag(raw: &str) -> Result<(String, String)> {
    let raw = raw.trim();
    let (name, value) = match raw.split_once('=') {
        Some((n, v)) => (n.trim(), v.trim()),
        None => (raw, "true"),
    };
    let name_ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_');
    let value_ok = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if name_ok && value_ok {
        Ok((name.to_string(), value.to_string()))
    } else {
        Err(Error::invalid(format!(
            "flag must be name or name=value with letters, digits, '.', '_' (and '-' in values): {raw:?}"
        )))
    }
}

/// Source text with every import not in `keep` removed. Removed imports leave blank text behind,
/// which Lean ignores.
pub fn render_imports(source: &str, imports: &[Import], keep: &[String]) -> String {
    let mut spans: Vec<(usize, usize)> = imports
        .iter()
        .filter(|i| !keep.contains(&i.module))
        .map(|i| (i.range.start, i.range.end))
        .collect();
    spans.sort_unstable();
    let mut out = source.to_string();
    for (start, end) in spans.into_iter().rev() {
        out.replace_range(start..end, "");
    }
    out
}

/// Source text with `set_option` lines for `flags` inserted after the last import line.
pub fn render_flags(source: &str, imports: &[Import], flags: &[(String, String)]) -> String {
    let insert_at = match imports.iter().map(|i| i.range.end).max() {
        Some(end) => source[end..]
            .find('\n')
            .map(|p| end + p + 1)
            .unwrap_or(source.len()),
        None => 0,
    };
    let mut block = String::new();
    if insert_at > 0 && !source[..insert_at].ends_with('\n') {
        block.push('\n');
    }
    for (name, value) in flags {
        block.push_str(&format!("set_option {name} {value}\n"));
    }
    format!("{}{}{}", &source[..insert_at], block, &source[insert_at..])
}

/// Restores a file's original text when dropped, so a failed or interrupted probe leaves the
/// project file as it was.
struct RestoreFile {
    path: PathBuf,
    original: String,
}

impl Drop for RestoreFile {
    fn drop(&mut self) {
        if let Err(e) = fs::write(&self.path, &self.original) {
            eprintln!(
                "warning: could not restore {}: {e}; restore it from version control",
                self.path.display()
            );
        }
    }
}

/// A typed check for the `imports` and `flags` axes. For each probe it writes the variant of
/// `rel_file` (see [`render_imports`] and [`render_flags`]), runs `lake build <module>` in the
/// project root, and restores the original file. The check passes on exit 0 with no error
/// diagnostics, like [`lake_build_check`].
///
/// Fails with `lean_not_found` when `toolchain.lake` is missing, and rejects `commits`.
pub fn lake_build_variant_check(
    toolchain: Toolchain,
    root: &Path,
    rel_file: &str,
    axis: Axis,
    timeout_seconds: u64,
) -> Result<impl FnMut(&[String]) -> Result<bool>> {
    if axis == Axis::Commits {
        return Err(Error::invalid("the commits axis uses lake_build_check"));
    }
    let path = project_lean_file(root, rel_file)?;
    let module = crate::imports::module_name(rel_file)
        .ok_or_else(|| Error::invalid(format!("not a module path: {rel_file:?}")))?;
    if toolchain.lake.is_none() {
        return Err(Error::new(
            ErrorCode::LeanNotFound,
            "lake not found on PATH or in ELAN_HOME/bin",
        ));
    }
    let source = fs::read_to_string(&path).map_err(|e| Error::from(e).with_path(&path))?;
    let imports = tpt_lean_syntax::parse(&source).imports;
    let policy = tpt_lean_adapter::lean_policy(timeout_seconds);
    let root = root.to_path_buf();
    Ok(move |applied: &[String]| {
        let text = match axis {
            Axis::Flags => {
                let flags = applied
                    .iter()
                    .map(|f| parse_flag(f))
                    .collect::<Result<Vec<_>>>()?;
                render_flags(&source, &imports, &flags)
            }
            _ => render_imports(&source, &imports, applied),
        };
        let _restore = RestoreFile {
            path: path.clone(),
            original: source.clone(),
        };
        fs::write(&path, text).map_err(|e| Error::from(e).with_path(&path))?;
        let result = tpt_lean_adapter::execute(
            &toolchain,
            &Command::LakeBuild {
                target: Some(module.clone()),
            },
            Some(&root),
            &policy,
            &CancelToken::new(),
        )?;
        Ok(result.output.success() && !result.diagnostics.iter().any(|d| d.severity == Severity::Error))
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

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("M{i}")).collect()
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
    fn axis_parses_known_values_and_rejects_others() {
        assert_eq!(Axis::parse("commits").unwrap(), Axis::Commits);
        assert_eq!(Axis::parse("imports").unwrap(), Axis::Imports);
        assert_eq!(Axis::parse("flags").unwrap(), Axis::Flags);
        let err = Axis::parse("mathlib").err().unwrap();
        assert_eq!(err.code(), ErrorCode::InvalidArgument);
    }

    #[test]
    fn bisect_items_finds_smallest_failing_prefix() {
        for n in 1..25 {
            for first in 0..n {
                let list = names(n);
                // The check fails once the applied prefix is longer than `first`.
                let report = bisect_items(Axis::Imports, "Main.lean", &list, |applied| {
                    Ok(applied.len() <= first)
                })
                .unwrap();
                assert_eq!(report.first_bad.as_deref(), Some(list[first].as_str()), "n={n} first={first}");
                assert_eq!(report.candidates, list);
                assert_eq!(report.axis, "imports");
                assert!(report.probes.iter().all(|p| p.enabled == list.iter().position(|x| *x == p.item).unwrap() + 1));
                assert_eq!(report.not_done, NOT_IMPLEMENTED.to_vec());
            }
        }
    }

    #[test]
    fn bisect_items_reports_no_failure_when_every_prefix_passes() {
        let list = names(6);
        let report = bisect_items(Axis::Flags, "Main.lean", &list, |_| Ok(true)).unwrap();
        assert_eq!(report.first_bad, None);
        assert!(report.probes.iter().all(|p| !p.bad));
        assert_eq!(report.axis, "flags");
    }

    #[test]
    fn bisect_items_propagates_check_errors() {
        let list = names(3);
        let err = bisect_items(Axis::Imports, "Main.lean", &list, |_| {
            Err(Error::new(ErrorCode::Timeout, "slow"))
        })
        .err()
        .unwrap();
        assert_eq!(err.code(), ErrorCode::Timeout);
    }

    #[test]
    fn render_imports_keeps_only_the_requested_prefix() {
        let src = "import A\nimport B.C\n\ntheorem t : True := trivial\n";
        let imports = tpt_lean_syntax::parse(src).imports;
        let out = render_imports(src, &imports, &["A".to_string()]);
        assert!(out.contains("import A"), "{out:?}");
        assert!(!out.contains("import B.C"), "{out:?}");
        assert!(out.contains("theorem t"));
        let none = render_imports(src, &imports, &[]);
        assert!(!none.contains("import"));
        assert_eq!(render_imports(src, &imports, &["A".into(), "B.C".into()]), src);
    }

    #[test]
    fn render_flags_inserts_after_imports() {
        let src = "import A\n\ntheorem t : True := trivial\n";
        let imports = tpt_lean_syntax::parse(src).imports;
        let flags = vec![("maxHeartbeats".to_string(), "400000".to_string())];
        assert_eq!(
            render_flags(src, &imports, &flags),
            "import A\nset_option maxHeartbeats 400000\n\ntheorem t : True := trivial\n"
        );
    }

    #[test]
    fn render_flags_without_imports_goes_to_the_top_and_handles_no_final_newline() {
        let flags = vec![("pp.all".to_string(), "true".to_string())];
        assert_eq!(
            render_flags("theorem t : True := trivial", &[], &flags),
            "set_option pp.all true\ntheorem t : True := trivial"
        );
        let src = "import A";
        let imports = tpt_lean_syntax::parse(src).imports;
        assert_eq!(render_flags(src, &imports, &flags), "import A\nset_option pp.all true\n");
    }

    #[test]
    fn parse_flag_accepts_name_value_and_bare_name() {
        assert_eq!(
            parse_flag("maxHeartbeats=400000").unwrap(),
            ("maxHeartbeats".to_string(), "400000".to_string())
        );
        assert_eq!(parse_flag("pp.all").unwrap(), ("pp.all".to_string(), "true".to_string()));
        assert_eq!(
            parse_flag(" linter.unusedVariables = false ").unwrap(),
            ("linter.unusedVariables".to_string(), "false".to_string())
        );
    }

    #[test]
    fn parse_flag_rejects_text_that_could_inject_lean_source() {
        for bad in ["", "=1", "a b", "x;y", "a=b c", "a=\n", "a=1\ntheorem"] {
            assert!(parse_flag(bad).is_err(), "{bad:?} should be rejected");
        }
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
    fn project_file_must_be_relative_lean_file() {
        let root = Path::new("/proj");
        assert!(project_lean_file(root, "Main.lean").is_ok());
        assert!(project_lean_file(root, "Sub/Other.lean").is_ok());
        assert!(project_lean_file(root, "../Main.lean").is_err());
        assert!(project_lean_file(root, "/abs/Main.lean").is_err());
        assert!(project_lean_file(root, "Main.txt").is_err());
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

    #[test]
    fn variant_check_needs_lake_and_rejects_commits_axis() {
        let tc = Toolchain {
            lean: None,
            lake: None,
        };
        let err = lake_build_variant_check(tc.clone(), Path::new("."), "Main.lean", Axis::Imports, 5)
            .err()
            .unwrap();
        assert_eq!(err.code(), ErrorCode::LeanNotFound);
        let err = lake_build_variant_check(tc, Path::new("."), "Main.lean", Axis::Commits, 5)
            .err()
            .unwrap();
        assert_eq!(err.code(), ErrorCode::InvalidArgument);
    }

    #[test]
    fn file_import_candidates_lists_imports_in_order() {
        let dir = std::env::temp_dir().join(format!("tpt-tools-bisect-imports-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Main.lean"), "import Mathlib.Tactic\nimport A.B\n\ntheorem t : True := trivial\n").unwrap();
        let got = file_import_candidates(&dir, "Main.lean").unwrap();
        assert_eq!(got, vec!["Mathlib.Tactic".to_string(), "A.B".to_string()]);
        let _ = fs::remove_dir_all(&dir);
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

        assert_eq!(report.axis, "commits");
        assert_eq!(report.commits_between, 4);
        assert_eq!(report.first_bad.as_deref(), Some(shas[2].as_str()));
        assert_eq!(report.not_done, NOT_IMPLEMENTED.to_vec());
        let worktrees = git_test(&repo, &["worktree", "list", "--porcelain"]);
        assert_eq!(worktrees.matches("worktree ").count(), 1, "{worktrees}");

        let _ = fs::remove_dir_all(&repo);
    }
}
