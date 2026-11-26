// SPDX-License-Identifier: MIT OR Apache-2.0
//! CLI wiring for the commands implemented in the Phase 2–8 crates: build, cache, profile,
//! docgen, lsp, ci, bisect, import analysis, doctor extensions, and server lifecycle.

use super::{load_config, project_root, OK};
use crate::args::Args;
use crate::{Ctx, Mode};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tpt_lean_adapter::{self as adapter, Toolchain};
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::{Error, ErrorCode, Result};

/// Lean-dependent commands need a toolchain; report a clear error when it is missing.
fn toolchain_or_error() -> Result<Toolchain> {
    let tc = adapter::discover_toolchain();
    if tc.lean.is_none() || tc.lake.is_none() {
        return Err(Error::new(
            ErrorCode::LeanNotFound,
            "lean and lake are required (install with elan)",
        ));
    }
    Ok(tc)
}

fn timeout_arg(args: &Args, default: u64) -> Result<u64> {
    match args.value("--timeout")? {
        Some(t) => t
            .parse::<u64>()
            .map_err(|_| Error::invalid("--timeout must be seconds")),
        None => Ok(default),
    }
}

// ---------------------------------------------------------------- build

/// `tpt-lean build [target] [--json] [--watch] [--explain-slow] [--plan] [--timeout S]`.
pub fn build_cmd(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &[
            "--project",
            "--timeout",
            "--json",
            "--watch",
            "--explain-slow",
            "--report",
            "--plan",
            "--poll-ms",
        ],
        &["--project", "--timeout", "--poll-ms"],
    )?;
    run_build(ctx, args, args.flag("--report"))
}

/// The build itself, shared by `build` and `profile --build`. With `report` set, the build report
/// (cache outcome, time per step, slowest files) follows the summary.
fn run_build(ctx: &Ctx, args: &Args, report: bool) -> Result<i32> {
    let root = project_root(args)?;
    let cfg = load_config(args, &root)?;
    if !cfg.security.allow_build {
        return Err(Error::new(
            ErrorCode::InvalidArgument,
            "building is disabled by security.allow_build",
        ));
    }
    if args.flag("--plan") {
        // The plan reads the sources only; it does not start Lake.
        let scan = tpt_lean_build::scan_project(&root)?;
        let plan = tpt_lean_build::plan(&scan)?;
        let mut human = vec![format!(
            "{} module(s) in {} wave(s); critical path {}, widest wave {}",
            plan.modules,
            plan.waves.len(),
            plan.critical_path,
            plan.widest_wave
        )];
        for (i, wave) in plan.waves.iter().enumerate() {
            const SHOWN: usize = 8;
            let mut names: Vec<&str> = wave.iter().take(SHOWN).map(String::as_str).collect();
            if wave.len() > SHOWN {
                names.push("...");
            }
            human.push(format!("wave {}: {}", i + 1, names.join(", ")));
        }
        ctx.emit(
            "build.plan",
            &human.join(
                "
",
            ),
            &plan,
        )?;
        return Ok(OK);
    }
    let target = args
        .positionals(&["--project", "--timeout", "--poll-ms"])
        .into_iter()
        .next();
    let timeout = timeout_arg(args, cfg.security.command_timeout_seconds)?;
    toolchain_or_error()?;
    let json_events = ctx.mode != Mode::Human || args.flag("--json");
    let mut events: Vec<tpt_lean_build::BuildEvent> = Vec::new();
    let mut sink = |e: &tpt_lean_build::BuildEvent| {
        if json_events {
            if let Ok(line) = serde_json::to_string(e) {
                println!("{line}");
            }
        }
        events.push(e.clone());
    };

    if args.flag("--watch") {
        let poll_ms: u64 = match args.value("--poll-ms")? {
            Some(p) => p
                .parse()
                .map_err(|_| Error::invalid("--poll-ms must be a number"))?,
            None => 1000,
        };
        ctx.note("watching for changes (Ctrl+C to stop)");
        tpt_lean_build::watch(
            &root,
            target.as_deref(),
            timeout,
            poll_ms,
            &CancelToken::new(),
            &mut sink,
        )?;
        return Ok(OK);
    }

    let outcome = tpt_lean_build::build(&root, target.as_deref(), timeout, &mut sink)?;
    let slow = if args.flag("--explain-slow") {
        tpt_lean_build::explain_slow(&events, 10)
    } else {
        Vec::new()
    };
    if !json_events {
        let human = format!(
            "build {}: {} file(s) built, {} failed, {} ms (cache key {} ms{})",
            if outcome.success { "succeeded" } else { "failed" },
            outcome.files_built,
            outcome.files_failed,
            outcome.duration_ms,
            outcome.key_ms,
            if outcome.cache_hit { ", cache hit" } else { "" }
        );
        let data = json!({
            "success": outcome.success,
            "timed_out": outcome.timed_out,
            "cache_hit": outcome.cache_hit,
            "files_built": outcome.files_built,
            "files_failed": outcome.files_failed,
            "duration_ms": outcome.duration_ms,
            "key_ms": outcome.key_ms,
            "digest": outcome.digest,
        });
        ctx.emit("build", &human, &data)?;
        for s in &slow {
            println!("slow: {s:?}");
        }
    } else if args.flag("--explain-slow") {
        ctx.emit("build.explain_slow", "", &slow)?;
    }
    if report {
        print_build_report(ctx, &build_report(&events, &outcome))?;
    }
    Ok(OK)
}

// ---------------------------------------------------------------- cache

fn cache_dir_or_error() -> Result<PathBuf> {
    tpt_lean_core::dirs::global_cache_dir()
        .ok_or_else(|| Error::new(ErrorCode::Io, "no global cache directory could be determined"))
}

/// `tpt-lean cache status`.
/// `tpt-lean cache export DIR`: copy the global cache's build records into DIR, for example as a CI
/// artifact to restore on another machine.
pub fn cache_export(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&[], &[])?;
    let dir = tpt_lean_build::cache::records_dir()
        .ok_or_else(|| Error::new(ErrorCode::Io, "no global cache directory could be determined"))?;
    let dest = args
        .positionals(&[])
        .into_iter()
        .next()
        .ok_or_else(|| Error::invalid("usage: tpt-lean cache export DIR"))?;
    let local = tpt_lean_build::DiskCache::new(dir);
    let copied = tpt_lean_build::cache::export_records(&local, Path::new(&dest))?;
    ctx.emit(
        "cache.export",
        &format!("exported {copied} record(s) to {dest}"),
        &json!({ "directory": dest, "records": copied }),
    )?;
    Ok(OK)
}

/// `tpt-lean cache import DIR`: copy valid build records from DIR into the global cache. Invalid
/// files are skipped.
pub fn cache_import(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&[], &[])?;
    let dir = tpt_lean_build::cache::records_dir()
        .ok_or_else(|| Error::new(ErrorCode::Io, "no global cache directory could be determined"))?;
    let src = args
        .positionals(&[])
        .into_iter()
        .next()
        .ok_or_else(|| Error::invalid("usage: tpt-lean cache import DIR"))?;
    let local = tpt_lean_build::DiskCache::new(dir);
    let copied = tpt_lean_build::cache::import_records(Path::new(&src), &local)?;
    ctx.emit(
        "cache.import",
        &format!("imported {copied} record(s) from {src}"),
        &json!({ "directory": src, "records": copied }),
    )?;
    Ok(OK)
}

pub fn cache_status(ctx: &Ctx, _args: &Args) -> Result<i32> {
    let dir = cache_dir_or_error()?;
    let status = tpt_lean_build::status(&dir);
    let shared: Vec<String> = tpt_lean_build::cache::shared_layers()
        .iter()
        .map(|d| d.root().display().to_string())
        .collect();
    let human = if status.exists {
        format!(
            "global cache: {} ({} files, {} bytes)",
            status.directory, status.files, status.bytes
        )
    } else {
        format!("global cache: {} (not created yet)", status.directory)
    };
    let human = if shared.is_empty() {
        human
    } else {
        format!("{human}\nshared layers: {}", shared.join(", "))
    };
    ctx.emit(
        "cache.status",
        &human,
        &json!({
            "location": status.directory,
            "exists": status.exists,
            "files": status.files,
            "bytes": status.bytes,
            "shared": shared,
        }),
    )?;
    Ok(OK)
}

/// `tpt-lean cache prune [--days N] [--dry-run]`.
pub fn cache_prune(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--days", "--dry-run"], &["--days"])?;
    let days: u64 = match args.value("--days")? {
        Some(d) => d.parse().map_err(|_| Error::invalid("--days must be a number"))?,
        None => 30,
    };
    let dir = cache_dir_or_error()?;
    let report =
        tpt_lean_build::prune_older_than(&dir, Duration::from_secs(days * 86_400), args.flag("--dry-run"))?;
    let verb = if report.dry_run { "would remove" } else { "removed" };
    let human = format!(
        "{verb} {} file(s), {} bytes older than {days} day(s)",
        report.removed_files, report.freed_bytes
    );
    ctx.emit(
        "cache.prune",
        &human,
        &json!({
            "directory": report.directory,
            "dry_run": report.dry_run,
            "removed_files": report.removed_files,
            "freed_bytes": report.freed_bytes,
        }),
    )?;
    Ok(OK)
}

// ---------------------------------------------------------------- explain

/// `tpt-lean explain --last-error`: show the diagnostics saved by the last run.
pub fn explain_last(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--last-error", "--project"], &["--project"])?;
    let root = project_root(args)?;
    match adapter::load_last_diagnostics(&root)? {
        None => Err(Error::new(
            ErrorCode::IndexError,
            "no saved diagnostics yet; run `tpt-lean check` first",
        )),
        Some(last) => {
            let human: Vec<String> = last
                .diagnostics
                .iter()
                .map(|d| {
                    format!(
                        "{}:{}:{}: {:?}: {}",
                        d.file, d.line, d.column, d.severity, d.message
                    )
                })
                .collect();
            let text = if human.is_empty() {
                "no diagnostics in the last run".to_string()
            } else {
                human.join("\n")
            };
            ctx.emit("explain.last_error", &text, &last)?;
            Ok(OK)
        }
    }
}

/// Lemma candidates for a goal, from the project index (used by `explain`).
pub fn lemma_candidates(root: &Path, goal: &str, limit: usize) -> Vec<serde_json::Value> {
    tpt_lean_adapter::explain::lemma_candidates(root, goal, limit)
}

// ---------------------------------------------------------------- profile

/// `tpt-lean profile FILE [--slowest N] [--json]` and `tpt-lean profile --compare A B`.
pub fn profile_cmd(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &[
            "--project",
            "--timeout",
            "--slowest",
            "--json",
            "--compare",
            "--flamegraph",
            "--imports",
            "--graph",
            "--graph-depth",
            "--build",
        ],
        &[
            "--project",
            "--timeout",
            "--slowest",
            "--flamegraph",
            "--graph",
            "--graph-depth",
        ],
    )?;
    if args.flag("--build") {
        // Build-step timing and the cache hit/miss outcome come from the build's own events.
        return run_build(ctx, args, true);
    }
    if args.flag("--compare") {
        let paths = args.positionals(&["--project", "--timeout", "--slowest", "--flamegraph"]);
        let [a, b] = paths.as_slice() else {
            return Err(Error::invalid("usage: tpt-lean profile --compare A.json B.json"));
        };
        let ra = tpt_lean_profile::read_report(Path::new(a))?;
        let rb = tpt_lean_profile::read_report(Path::new(b))?;
        let text = tpt_lean_profile::compare(&ra, &rb);
        ctx.emit("profile.compare", &text, &json!({ "text": text }))?;
        return Ok(OK);
    }
    let file = args
        .positionals(&["--project", "--timeout", "--slowest", "--flamegraph"])
        .into_iter()
        .next()
        .ok_or_else(|| {
            Error::invalid("usage: tpt-lean profile FILE [--slowest N] [--json] [--flamegraph OUT.folded]")
        })?;
    let root = project_root(args)?;
    let cfg = load_config(args, &root)?;
    let ws = tpt_lean_core::paths::WorkspaceRoot::new(&root)?;
    let path = ws.resolve(&file)?;
    let timeout = timeout_arg(args, cfg.security.command_timeout_seconds)?;
    toolchain_or_error()?;
    let (mut report, output) = tpt_lean_profile::profile_file_traced(&root, &path, timeout)?;
    let graph_path = args.value("--graph")?;
    let depth: usize = match args.value("--graph-depth")? {
        Some(v) => v
            .parse::<usize>()
            .ok()
            .filter(|d| (1..=4).contains(d))
            .ok_or_else(|| Error::invalid("--graph-depth must be 1 to 4"))?,
        None => 1,
    };
    if args.flag("--imports") || graph_path.is_some() {
        // Deeper graphs time project modules too; cap the number of Lean runs they cost.
        let max_runs = if depth > 1 { 60 } else { usize::MAX };
        report.import_timings = tpt_lean_profile::import_tree(&root, &path, timeout, depth, max_runs)?;
    }
    if let Some(out_path) = graph_path {
        let dot = tpt_lean_profile::import_graph_dot(&report);
        std::fs::write(&out_path, &dot).map_err(|e| Error::from(e).with_path(&out_path))?;
        let edges = report.import_timings.len();
        ctx.emit(
            "profile.graph",
            &format!("wrote an import graph with {edges} import(s) to {out_path}"),
            &json!({ "path": out_path, "imports": edges }),
        )?;
        return Ok(OK);
    }
    if let Some(out_path) = args.value("--flamegraph")? {
        let folded = tpt_lean_profile::flamegraph_folded(&output);
        std::fs::write(&out_path, &folded).map_err(|e| Error::from(e).with_path(&out_path))?;
        let stacks = folded.lines().count();
        let human = if stacks == 0 {
            format!("no trace nodes in the output; wrote an empty file to {out_path}")
        } else {
            format!("wrote {stacks} folded stack(s) to {out_path}")
        };
        ctx.emit(
            "profile.flamegraph",
            &human,
            &json!({ "path": out_path, "stacks": stacks }),
        )?;
        return Ok(OK);
    }
    let n: usize = match args.value("--slowest")? {
        Some(v) => v
            .parse()
            .map_err(|_| Error::invalid("--slowest must be a number"))?,
        None => 10,
    };
    if args.flag("--json") || ctx.mode != Mode::Human {
        let body = tpt_lean_profile::to_json(&report)?;
        println!("{body}");
    } else {
        print!("{}", tpt_lean_profile::render_report(&report, n));
    }
    Ok(OK)
}

// ---------------------------------------------------------------- docgen

fn open_path(path: &Path) -> Result<()> {
    // Typed invocation of the platform opener: no shell string is built.
    let (program, arg): (&str, PathBuf) = if cfg!(windows) {
        ("explorer", path.to_path_buf())
    } else if cfg!(target_os = "macos") {
        ("open", path.to_path_buf())
    } else {
        ("xdg-open", path.to_path_buf())
    };
    std::process::Command::new(program)
        .arg(arg)
        .spawn()
        .map(|_| ())
        .map_err(|e| Error::from(e).with_path(path))
}

/// `tpt-lean docgen [--out DIR] [--open] [--json] [--include-deps]`, or
/// `tpt-lean docgen --from-sidecar Module.Name [--exe PATH] [--timeout S] [--out DIR] [--open] [--json]`.
pub fn docgen_cmd(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &[
            "--project",
            "--out",
            "--open",
            "--json",
            "--include-deps",
            "--from-sidecar",
            "--exe",
            "--timeout",
        ],
        &["--project", "--out", "--from-sidecar", "--exe", "--timeout"],
    )?;
    let root = project_root(args)?;
    let include_deps = args.flag("--include-deps");
    let sidecar_module = args.value("--from-sidecar")?;
    if sidecar_module.is_some() && include_deps {
        return Err(Error::invalid(
            "--include-deps cannot be combined with --from-sidecar (the sidecar documents one module)",
        ));
    }
    // The sidecar report is fetched first so that a missing sidecar or a failing module is
    // reported before any output is written.
    let sidecar_report = match sidecar_module {
        Some(module) => {
            let cfg = super::load_config(args, &root)?;
            let run = super::sidecar_run(args, &root, &cfg)?;
            let mut policy = run.policy.clone();
            if let Some(lean_path) = &run.lean_path {
                policy.extra_env.push(("LEAN_PATH".to_string(), lean_path.clone()));
            }
            Some(adapter::sidecar::run_sidecar(
                &run.exe,
                &run.sysroot,
                &module,
                Some(&root),
                &policy,
                &run.cancel,
            )?)
        }
        None => None,
    };
    if args.flag("--json") {
        let body = match &sidecar_report {
            Some(report) => tpt_lean_docgen::export_json_from_sidecar(&root, report)?,
            None => tpt_lean_docgen::export_json(&root, include_deps)?,
        };
        println!("{body}");
        return Ok(OK);
    }
    let out = match args.value("--out")? {
        Some(p) => PathBuf::from(p),
        None => root.join(".tpt-lean").join("docs"),
    };
    let summary = match &sidecar_report {
        Some(report) => tpt_lean_docgen::generate_from_sidecar(&root, &out, report)?,
        None => tpt_lean_docgen::generate(&root, &out, include_deps)?,
    };
    let human = format!(
        "documented {} module(s), {} declaration(s) in {}",
        summary.modules,
        summary.declarations,
        summary.out_dir.display()
    );
    ctx.emit("docgen", &human, &summary)?;
    if args.flag("--open") {
        open_path(&summary.out_dir.join("index.html"))?;
    }
    Ok(OK)
}

// ---------------------------------------------------------------- lsp

/// `tpt-lean lsp`: run the LSP proxy over stdin/stdout.
pub fn lsp_cmd(_ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--project"], &["--project"])?;
    let root = project_root(args)?;
    tpt_lean_lsp::run_stdio(&root)?;
    Ok(OK)
}

// ---------------------------------------------------------------- ci

/// `tpt-lean ci [--json] [--sarif] [--timeout S]`.
pub fn ci_cmd(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &["--project", "--timeout", "--json", "--sarif"],
        &["--project", "--timeout"],
    )?;
    let root = project_root(args)?;
    let cfg = load_config(args, &root)?;
    let timeout = timeout_arg(args, cfg.security.command_timeout_seconds)?;
    let tc = toolchain_or_error()?;
    let opts = tpt_lean_tools::ci::CiOptions {
        timeout_seconds: timeout,
    };
    let report = tpt_lean_tools::ci::run_ci(&root, &tc, &opts)?;
    if args.flag("--sarif") {
        println!("{}", tpt_lean_tools::ci::to_sarif(&report));
        return Ok(if report.ok { OK } else { 1 });
    }
    if args.flag("--json") || ctx.mode != Mode::Human {
        println!(
            "{}",
            serde_json::to_string(&report).map_err(|e| Error::new(ErrorCode::Internal, e.to_string()))?
        );
    } else {
        println!(
            "ci: {} in {} ms, {} diagnostic(s)",
            if report.ok { "ok" } else { "failed" },
            report.total_ms,
            report.diagnostics.len()
        );
    }
    Ok(if report.ok { OK } else { 1 })
}

// ---------------------------------------------------------------- bisect

/// `tpt-lean bisect [--axis commits|imports|flags] ...`: find the first failing candidate with a
/// `lake build` check per probe.
///
/// - `commits` (default): `--good REV --bad REV`; one temporary worktree per probe.
/// - `imports`: `--file FILE`; probes the file's imports in order, keeping a growing prefix.
/// - `flags`: `--file FILE --flags "a=1,b,c=false"`; probes Lean options in order.
///
/// The `imports` and `flags` axes rewrite FILE for each probe and restore it afterwards.
pub fn bisect_cmd(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &["--project", "--axis", "--good", "--bad", "--file", "--flags", "--timeout"],
        &["--project", "--axis", "--good", "--bad", "--file", "--flags", "--timeout"],
    )?;
    let axis = tpt_lean_tools::bisect::Axis::parse(args.value("--axis")?.as_deref().unwrap_or("commits"))?;
    let root = project_root(args)?;
    let cfg = load_config(args, &root)?;
    let timeout = timeout_arg(args, cfg.security.command_timeout_seconds)?;
    if axis == tpt_lean_tools::bisect::Axis::Commits {
        if args.value("--file")?.is_some() || args.value("--flags")?.is_some() {
            return Err(Error::invalid("--file and --flags apply to --axis imports or flags"));
        }
        let good = args
            .value("--good")?
            .ok_or_else(|| Error::invalid("--good REV is required"))?;
        let bad = args
            .value("--bad")?
            .ok_or_else(|| Error::invalid("--bad REV is required"))?;
        let tc = toolchain_or_error()?;
        let check = tpt_lean_tools::bisect::lake_build_check(tc, ".", timeout)?;
        let report = tpt_lean_tools::bisect::bisect_commits(&root, &good, &bad, check)?;
        let human = match &report.first_bad {
            Some(c) => format!(
                "first failing commit: {c} ({} commit(s) between)",
                report.commits_between
            ),
            None => format!("no failing commit between {good} and {bad}"),
        };
        ctx.emit("bisect", &human, &report)?;
        return Ok(OK);
    }
    if args.value("--good")?.is_some() || args.value("--bad")?.is_some() {
        return Err(Error::invalid("--good and --bad apply to --axis commits"));
    }
    let file = args
        .value("--file")?
        .ok_or_else(|| Error::invalid("--file FILE is required for this axis"))?;
    let (candidates, label) = match axis {
        tpt_lean_tools::bisect::Axis::Imports => {
            if args.value("--flags")?.is_some() {
                return Err(Error::invalid("--flags applies to --axis flags"));
            }
            let imports = tpt_lean_tools::bisect::file_import_candidates(&root, &file)?;
            (imports, "import")
        }
        _ => {
            let raw = args
                .value("--flags")?
                .ok_or_else(|| Error::invalid("--flags \"a=1,b,c\" is required for --axis flags"))?;
            let flags: Vec<String> = raw
                .split(',')
                .map(str::trim)
                .filter(|f| !f.is_empty())
                .map(String::from)
                .collect();
            for f in &flags {
                tpt_lean_tools::bisect::parse_flag(f)?;
            }
            (flags, "flag")
        }
    };
    let tc = toolchain_or_error()?;
    let check = tpt_lean_tools::bisect::lake_build_variant_check(tc, &root, &file, axis, timeout)?;
    let report = tpt_lean_tools::bisect::bisect_items(axis, &file, &candidates, check)?;
    let human = match &report.first_bad {
        Some(item) => format!(
            "first failing {label}: {item} ({} candidate(s), {} probe(s)); {file} was restored",
            candidates.len(),
            report.probes.len()
        ),
        None => format!("no failing {label} among {} candidate(s) in {file}", candidates.len()),
    };
    ctx.emit("bisect", &human, &report)?;
    Ok(OK)
}

// ---------------------------------------------------------------- imports

/// Import analysis flags: `--unused`, `--suggest QUERY`, `--minimize FILE`, `--cost`.
/// Returns `None` when no analysis flag is present (plain listing applies).
pub fn imports_analysis(ctx: &Ctx, args: &Args) -> Option<Result<i32>> {
    let analysis = args.flag("--unused")
        || args.flag("--cost")
        || args.value("--suggest").ok().flatten().is_some()
        || args.flag("--minimize");
    if !analysis {
        return None;
    }
    Some(imports_analysis_run(ctx, args))
}

/// One import of a file, timed by Lean, with the module's structural weight for context.
#[derive(serde::Serialize)]
struct TimedImportCost {
    module: String,
    line: u32,
    load_ms: f64,
    error: Option<String>,
    /// Project modules reachable from this import (see `imports --cost`), when it is a project file.
    transitive_project_imports: Option<usize>,
}

fn imports_analysis_run(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &[
            "--project",
            "--unused",
            "--suggest",
            "--minimize",
            "--cost",
            "--timed",
            "--timeout",
            "--limit",
        ],
        &["--project", "--suggest", "--minimize", "--limit", "--timeout"],
    )?;
    let root = project_root(args)?;
    if let Some(query) = args.value("--suggest")? {
        let limit: usize = match args.value("--limit")? {
            Some(v) => v
                .parse()
                .map_err(|_| Error::invalid("--limit must be a number"))?,
            None => 10,
        };
        let items = tpt_lean_tools::imports::suggest_imports(&root, &query, limit)?;
        let human: Vec<String> = items
            .iter()
            .map(|s| format!("import {}  ({} match(es) in {})", s.module, s.matches, s.file))
            .collect();
        ctx.emit_items("imports.suggest", &human, &items)?;
        return Ok(OK);
    }
    if args.flag("--cost") && args.flag("--timed") {
        // Per-import compile-time estimate for one file: the same timings as `profile --imports`.
        let file = args
            .positionals(&["--project", "--timeout"])
            .into_iter()
            .next()
            .ok_or_else(|| {
                Error::invalid("usage: tpt-lean imports --cost --timed FILE [--timeout S]")
            })?;
        let cfg = load_config(args, &root)?;
        let timeout = timeout_arg(args, cfg.security.command_timeout_seconds)?;
        let ws = tpt_lean_core::paths::WorkspaceRoot::new(&root)?;
        let path = ws.resolve(&file)?;
        let structural = tpt_lean_tools::imports::import_costs(&root)?;
        let rows: Vec<TimedImportCost> = tpt_lean_profile::import_timings(&root, &path, timeout)?
            .into_iter()
            .map(|t| TimedImportCost {
                transitive_project_imports: structural
                    .iter()
                    .find(|c| c.module == t.module)
                    .map(|c| c.transitive_project_imports),
                module: t.module,
                line: t.line,
                load_ms: t.load_ms,
                error: t.error,
            })
            .collect();
        let human: Vec<String> = rows
            .iter()
            .map(|r| match &r.error {
                Some(e) => format!("line {}: {} not loaded: {e}", r.line, r.module),
                None => format!("line {}: {} ~{:.0} ms", r.line, r.module, r.load_ms),
            })
            .collect();
        ctx.emit_items("imports.cost", &human, &rows)?;
        return Ok(OK);
    }
    if args.flag("--cost") {
        let costs = tpt_lean_tools::imports::import_costs(&root)?;
        let human: Vec<String> = costs
            .iter()
            .map(|c| format!("{}  direct={} ", c.module, c.direct_imports))
            .collect();
        ctx.emit_items("imports.cost", &human, &costs)?;
        return Ok(OK);
    }
    if args.flag("--unused") {
        let ws = tpt_lean_core::paths::WorkspaceRoot::new(&root)?;
        let file = args
            .positionals(&["--project", "--suggest", "--minimize", "--limit"])
            .into_iter()
            .next()
            .ok_or_else(|| Error::invalid("usage: tpt-lean imports --unused FILE"))?;
        let path = ws.resolve(&file)?;
        let findings = tpt_lean_tools::imports::unused_imports(&root, &path)?;
        let human: Vec<String> = findings.iter().map(|f| format!("{f:?}")).collect();
        ctx.emit_items("imports.unused", &human, &findings)?;
        return Ok(OK);
    }
    // --minimize FILE: prints a plan (diff) and never writes the file.
    let file = args
        .value("--minimize")?
        .ok_or_else(|| Error::invalid("usage: tpt-lean imports --minimize FILE"))?;
    let ws = tpt_lean_core::paths::WorkspaceRoot::new(&root)?;
    let path = ws.resolve(&file)?;
    let plan = tpt_lean_tools::imports::minimize_imports(&root, &path)?;
    let human = if plan.diff.is_empty() {
        "nothing to remove".to_string()
    } else {
        plan.diff.clone()
    };
    ctx.emit("imports.minimize", &human, &plan)?;
    Ok(OK)
}

// ---------------------------------------------------------------- server

/// `tpt-lean server stop|status`.
pub fn server_lifecycle(ctx: &Ctx, args: &Args, action: &str) -> Result<i32> {
    args.reject_unknown(&["--project"], &["--project"])?;
    let root = project_root(args)?;
    match action {
        "status" => {
            let report = tpt_lean_server::status(&root)?;
            ctx.emit("server.status", &report.to_json().to_string(), &report.to_json())?;
        }
        _ => {
            let report = tpt_lean_server::stop(&root)?;
            ctx.emit("server.stop", &report.to_json().to_string(), &report.to_json())?;
        }
    }
    Ok(OK)
}

/// `tpt-lean mcp http [--port N]`: MCP over local HTTP/SSE on 127.0.0.1.
pub fn mcp_http(_ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--project", "--port"], &["--project", "--port"])?;
    let root = project_root(args)?;
    let port: u16 = match args.value("--port")? {
        Some(p) => p.parse().map_err(|_| Error::invalid("--port must be 0-65535"))?,
        None => 7474,
    };
    let server = tpt_lean_mcp::Server::new(&root)?;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    tpt_lean_mcp::serve_http(&server, addr).map_err(Error::from)?;
    Ok(OK)
}

// ---------------------------------------------------------------- watch

/// One checked file, as printed by `watch` and streamed with `--json`.
#[derive(serde::Serialize)]
struct WatchRecord {
    file: String,
    errors: usize,
    warnings: usize,
    sorry: usize,
    error: Option<String>,
    diagnostics: Vec<adapter::Diagnostic>,
}

/// A file's identity for change detection: its length and modification time.
type Stamp = (u64, Option<std::time::SystemTime>);

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

/// `watch [--once] [--interval-ms N]`: check every `.lean` file on start, then re-check each file
/// when its length or modification time changes. Prints one summary per checked file. With
/// `--once` it checks everything once and exits 1 if any file has an error.
pub fn watch_cmd(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &["--project", "--timeout", "--interval-ms", "--once"],
        &["--project", "--timeout", "--interval-ms"],
    )?;
    let root = project_root(args)?;
    let cfg = load_config(args, &root)?;
    let timeout = timeout_arg(args, cfg.security.command_timeout_seconds)?;
    let interval = match args.value("--interval-ms")? {
        Some(v) => v
            .parse::<u64>()
            .map_err(|_| Error::invalid("--interval-ms must be a number of milliseconds"))?
            .max(100),
        None => 1000,
    };
    let once = args.flag("--once");
    let tc = toolchain_or_error()?;
    let policy = adapter::lean_policy(timeout);
    let cancel = CancelToken::new();

    let mut state: std::collections::BTreeMap<String, Stamp> = std::collections::BTreeMap::new();
    let mut any_error = false;
    if ctx.mode == Mode::Human && !once {
        ctx.note("watching for changes (Ctrl+C to stop)");
    }
    loop {
        // lakefile.lean is read by Lake, not by plain `lean`, so it is not checked here.
        let files: Vec<(String, PathBuf)> = tpt_lean_index::collect_lean_files(&root)?
            .into_iter()
            .filter(|(rel, _)| rel != "lakefile.lean")
            .collect();
        // Forget files that were deleted, so that a recreated file is checked again.
        let present: std::collections::BTreeSet<&str> = files.iter().map(|(r, _)| r.as_str()).collect();
        state.retain(|rel, _| present.contains(rel.as_str()));
        for (rel, path) in &files {
            let Some(now) = stamp(path) else { continue };
            if state.get(rel) == Some(&now) {
                continue;
            }
            state.insert(rel.clone(), now);
            let record = check_file(&tc, &policy, &cancel, &root, rel, path);
            if record.errors > 0 || record.error.is_some() {
                any_error = true;
            }
            print_watch_record(ctx, &record)?;
        }
        if once {
            return Ok(if any_error { 1 } else { OK });
        }
        std::thread::sleep(Duration::from_millis(interval));
    }
}

/// Run `lean` on one file and summarise its diagnostics and `sorry` count.
fn check_file(
    tc: &Toolchain,
    policy: &tpt_lean_core::policy::ProcessPolicy,
    cancel: &CancelToken,
    root: &Path,
    rel: &str,
    path: &Path,
) -> WatchRecord {
    let command = adapter::Command::LeanCheck {
        file: path.to_path_buf(),
    };
    let sorry = std::fs::read_to_string(path)
        .map(|src| tpt_lean_tools::sorry::hits_in_source(rel, &src).len())
        .unwrap_or(0);
    match adapter::execute(tc, &command, Some(root), policy, cancel) {
        Ok(result) => {
            let errors = result
                .diagnostics
                .iter()
                .filter(|d| d.severity == adapter::Severity::Error)
                .count();
            let warnings = result
                .diagnostics
                .iter()
                .filter(|d| d.severity == adapter::Severity::Warning)
                .count();
            WatchRecord {
                file: rel.to_string(),
                errors,
                warnings,
                sorry,
                error: None,
                diagnostics: result.diagnostics,
            }
        }
        Err(e) => WatchRecord {
            file: rel.to_string(),
            errors: 0,
            warnings: 0,
            sorry,
            error: Some(e.to_string()),
            diagnostics: Vec::new(),
        },
    }
}

fn print_watch_record(ctx: &Ctx, record: &WatchRecord) -> Result<()> {
    let mut human = vec![format!(
        "{}: {} error(s), {} warning(s), {} sorry",
        record.file, record.errors, record.warnings, record.sorry
    )];
    if let Some(e) = &record.error {
        human.push(format!("  could not check: {e}"));
    }
    for d in record
        .diagnostics
        .iter()
        .filter(|d| d.severity == adapter::Severity::Error)
        .take(3)
    {
        let first = d.message.lines().next().unwrap_or("");
        human.push(format!("  {}:{}: {first}", d.line, d.column));
    }
    ctx.emit_items("watch", &human, std::slice::from_ref(record))
}

// ---------------------------------------------------------------- build report

/// One file in the build report, with the time Lake reported for it.
#[derive(serde::Serialize)]
struct FileTiming {
    file: String,
    duration_ms: Option<u64>,
    failed: bool,
}

/// `build --report`: where the time of one build went, and whether a cache layer served it.
#[derive(serde::Serialize)]
struct BuildReport {
    success: bool,
    /// The cache layer that served the build (`memory`, `disk` or `shared`), when it hit.
    cache_hit: bool,
    cache_layer: Option<String>,
    /// Time to compute the cache key, from `tpt-lean` itself.
    key_ms: u64,
    total_ms: u64,
    /// Total minus the cache key time: the part spent in Lake and the compiler.
    lake_ms: u64,
    files_built: usize,
    files_failed: usize,
    /// The slowest files, longest first. At most ten.
    slowest: Vec<FileTiming>,
}

fn build_report(
    events: &[tpt_lean_build::BuildEvent],
    outcome: &tpt_lean_build::BuildOutcome,
) -> BuildReport {
    use tpt_lean_build::BuildEvent;
    let cache_layer = events.iter().find_map(|e| match e {
        BuildEvent::CacheHit { layer, .. } => serde_json::to_value(layer)
            .ok()
            .and_then(|v| v.as_str().map(String::from)),
        _ => None,
    });
    let mut files: Vec<FileTiming> = events
        .iter()
        .filter_map(|e| match e {
            BuildEvent::FileFinished {
                file, duration_ms, ..
            } => Some(FileTiming {
                file: file.clone(),
                duration_ms: *duration_ms,
                failed: false,
            }),
            BuildEvent::FileFailed {
                file, duration_ms, ..
            } => Some(FileTiming {
                file: file.clone(),
                duration_ms: *duration_ms,
                failed: true,
            }),
            _ => None,
        })
        .collect();
    // Files without a reported duration sort last.
    files.sort_by_key(|f| std::cmp::Reverse(f.duration_ms));
    files.truncate(10);
    BuildReport {
        success: outcome.success,
        cache_hit: outcome.cache_hit,
        cache_layer,
        key_ms: outcome.key_ms,
        total_ms: outcome.duration_ms,
        lake_ms: outcome.duration_ms.saturating_sub(outcome.key_ms),
        files_built: outcome.files_built,
        files_failed: outcome.files_failed,
        slowest: files,
    }
}

/// Print the report for a person, or emit it in the JSON envelope.
fn print_build_report(ctx: &Ctx, report: &BuildReport) -> Result<()> {
    let cache = match (&report.cache_hit, &report.cache_layer) {
        (true, Some(layer)) => format!("hit ({layer} layer)"),
        (true, None) => "hit".to_string(),
        (false, _) => "miss".to_string(),
    };
    let mut human = vec![
        format!("cache: {cache}"),
        format!(
            "time: {} ms total = {} ms cache key + {} ms Lake",
            report.total_ms, report.key_ms, report.lake_ms
        ),
    ];
    if !report.slowest.is_empty() {
        human.push("slowest files:".to_string());
        for f in &report.slowest {
            let time = f
                .duration_ms
                .map(|d| format!("{d} ms"))
                .unwrap_or_else(|| "no time reported".to_string());
            let status = if f.failed { " (failed)" } else { "" };
            human.push(format!("  {time:>16}  {}{status}", f.file));
        }
    }
    ctx.emit_items("build.report", &human, std::slice::from_ref(report))
}
