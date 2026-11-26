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
            "--plan",
            "--poll-ms",
        ],
        &["--project", "--timeout", "--poll-ms"],
    )?;
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
        ctx.emit("build.plan", &human.join("
"), &plan)?;
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
    let dir = tpt_lean_build::cache::records_dir().ok_or_else(|| Error::new(ErrorCode::Io, "no global cache directory could be determined"))?;
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
    let dir = tpt_lean_build::cache::records_dir().ok_or_else(|| Error::new(ErrorCode::Io, "no global cache directory could be determined"))?;
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
        format!("{human}
shared layers: {}", shared.join(", "))
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

/// `tpt-lean docgen [--out DIR] [--open] [--json] [--include-deps]`.
pub fn docgen_cmd(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &["--project", "--out", "--open", "--json", "--include-deps"],
        &["--project", "--out"],
    )?;
    let root = project_root(args)?;
    let include_deps = args.flag("--include-deps");
    if args.flag("--json") {
        let body = tpt_lean_docgen::export_json(&root, include_deps)?;
        println!("{body}");
        return Ok(OK);
    }
    let out = match args.value("--out")? {
        Some(p) => PathBuf::from(p),
        None => root.join(".tpt-lean").join("docs"),
    };
    let summary = tpt_lean_docgen::generate(&root, &out, include_deps)?;
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

/// `tpt-lean bisect --good REV --bad REV [--timeout S]`: find the first failing commit with a
/// `lake build` check in a temporary worktree per probe.
pub fn bisect_cmd(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &["--project", "--good", "--bad", "--timeout"],
        &["--project", "--good", "--bad", "--timeout"],
    )?;
    let root = project_root(args)?;
    let cfg = load_config(args, &root)?;
    let good = args
        .value("--good")?
        .ok_or_else(|| Error::invalid("--good REV is required"))?;
    let bad = args
        .value("--bad")?
        .ok_or_else(|| Error::invalid("--bad REV is required"))?;
    let timeout = timeout_arg(args, cfg.security.command_timeout_seconds)?;
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

fn imports_analysis_run(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &[
            "--project",
            "--unused",
            "--suggest",
            "--minimize",
            "--cost",
            "--limit",
        ],
        &["--project", "--suggest", "--minimize", "--limit"],
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
