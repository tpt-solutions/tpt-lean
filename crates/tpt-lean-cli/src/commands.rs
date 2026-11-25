// SPDX-License-Identifier: MIT OR Apache-2.0
//! Command implementations and dispatch.

mod ext;

use crate::args::Args;
use crate::{Ctx, Mode};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tpt_lean_adapter::explain::split_goal;
use tpt_lean_adapter::{self as adapter, Command as LeanCommand};
use tpt_lean_core::config::{self, Config};
use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_core::paths::WorkspaceRoot;
use tpt_lean_core::policy::CancelToken;
use tpt_lean_core::project;
use tpt_lean_core::{Error, ErrorCode, Result};
use tpt_lean_index::{default_scan_roots, Index};
use tpt_lean_search::SearchOptions;

/// Exit code for a successful command.
const OK: i32 = 0;
/// Exit code for `fmt --check` when files would change.
const CHECK_FAILED: i32 = 1;

/// Route the parsed arguments to a command. Returns the process exit code.
pub fn dispatch(ctx: &Ctx, args: &Args) -> Result<i32> {
    if args.help || args.command.is_empty() {
        return help(ctx);
    }
    let name: Vec<&str> = args.command.iter().map(|s| s.as_str()).collect();
    match name.as_slice() {
        ["init"] => init(ctx, args),
        ["doctor"] => doctor(ctx, args),
        ["config"] | ["config", "show"] => config_show(ctx, args),
        ["fmt"] => fmt(ctx, args),
        ["search"] => search(ctx, args),
        ["index", "scan"] | ["index", "refresh"] => index_refresh(ctx, args),
        ["index", "status"] => index_status(ctx, args),
        ["index", "clear"] => index_clear(ctx, args),
        ["index", "sidecar"] => index_sidecar(ctx, args),
        ["mcp", "stdio"] => mcp_stdio(ctx, args),
        ["mcp", "http"] => ext::mcp_http(ctx, args),
        ["server", "stop"] => ext::server_lifecycle(ctx, args, "stop"),
        ["server", "status"] => ext::server_lifecycle(ctx, args, "status"),
        ["docgen"] => ext::docgen_cmd(ctx, args),
        ["profile"] => ext::profile_cmd(ctx, args),
        ["lsp"] => ext::lsp_cmd(ctx, args),
        ["ci"] => ext::ci_cmd(ctx, args),
        ["bisect"] => ext::bisect_cmd(ctx, args),
        ["server", "start"] => server_start(ctx, args),
        ["build"] => ext::build_cmd(ctx, args),
        ["check"] => check(ctx, args),
        ["cache", "status"] => ext::cache_status(ctx, args),
        ["cache", "prune"] => ext::cache_prune(ctx, args),
        ["clean"] => clean(ctx, args),
        ["goals"] => goals(ctx, args),
        ["explain"] if args.flag("--last-error") => ext::explain_last(ctx, args),
        ["explain"] => explain(ctx, args),
        ["context-pack"] => context_pack(ctx, args),
        ["imports"] => match ext::imports_analysis(ctx, args) {
            Some(r) => r,
            None => imports(ctx, args),
        },
        ["help"] => help(ctx),
        [cmd, ..] => not_implemented(cmd),
        [] => help(ctx),
    }
}

fn not_implemented(cmd: &str) -> Result<i32> {
    Err(Error::not_implemented(format!("`tpt-lean {cmd}`")))
}

fn help(ctx: &Ctx) -> Result<i32> {
    let text = "\
tpt-lean: tooling around Lean 4 (Lean remains the proof checker)

Implemented:
  init <dir> [--mathlib]        create a starter Lean project
  doctor                        check the local Lean environment
  config show                   print the effective configuration
  fmt [--check|--diff] [paths]  conservative formatter
  search <query> [--namespace NS] [--near TERM]... [--limit N]
  index scan|refresh|status|clear
  build [target]                run `lake build` (typed options only)
  check [file]                  run `lake build` (no file) or `lean <file>`
  cache status                  show the global cache location and size
  mcp stdio                     MCP server over stdin/stdout (read-only tools)

Planned, not built yet (exit code 10):
  goals, explain, context-pack, imports, profile, docgen, lsp, server,
  bisect, ci, clean, cache prune

Global flags: --json, --ndjson, --project <dir>, --set section.key=value, -v, -q";
    if ctx.mode == Mode::Human {
        println!("{text}");
    } else {
        ctx.emit(
            "help",
            "",
            &serde_json::json!({ "commands": text.lines().count() }),
        )?;
    }
    Ok(OK)
}

/// Resolve the project root: `--project`, else the nearest marker above the working directory,
/// else the working directory itself.
fn project_root(args: &Args) -> Result<PathBuf> {
    if let Some(p) = &args.project {
        let p = PathBuf::from(p);
        return std::fs::canonicalize(&p)
            .map(|c| tpt_lean_core::paths::strip_verbatim(&c))
            .map_err(|e| Error::from(e).with_path(&p));
    }
    let cwd = std::env::current_dir()?;
    match project::discover(&cwd) {
        Ok(p) => Ok(p.root),
        Err(e) if e.code() == ErrorCode::ProjectNotFound => Ok(tpt_lean_core::paths::strip_verbatim(
            &std::fs::canonicalize(&cwd)?,
        )),
        Err(e) => Err(e),
    }
}

fn load_config(args: &Args, root: &Path) -> Result<Config> {
    let overrides: Vec<(&str, &str)> = args
        .overrides
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    Ok(config::load_config(root, &overrides)?.0)
}

#[derive(Serialize)]
struct InitResult {
    root: String,
    files: Vec<String>,
}

fn init(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--mathlib", "--template"], &["--template"])?;
    let name = args
        .positionals(&[])
        .into_iter()
        .next()
        .ok_or_else(|| Error::invalid("usage: tpt-lean init <dir> [--mathlib]"))?;
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') || name.is_empty() {
        return Err(Error::invalid(
            "project name must use letters, digits, '_' or '-'",
        ));
    }
    let dir = PathBuf::from(&name);
    if dir.exists() && std::fs::read_dir(&dir)?.next().is_some() {
        return Err(Error::invalid(format!("{name} exists and is not empty")).with_path(&dir));
    }
    std::fs::create_dir_all(&dir)?;
    let mathlib = args.flag("--mathlib");
    let template = args.value("--template")?.unwrap_or_else(|| "default".into());
    let main_body = match template.as_str() {
        "default" => format!(
            "def main : IO Unit :=
  IO.println \"Hello from {}\"
",
            name.replace('-', "_")
        ),
        "theorem" => "theorem add_comm_example (a b : Nat) : a + b = b + a := Nat.add_comm a b

def main : IO Unit :=
  IO.println \"theorem template\"
"
        .into(),
        "teaching" => "-- Exercise: prove the statement below.
-- theorem exercise_one : 1 + 1 = 2 := by sorry

def main : IO Unit :=
  IO.println \"teaching template\"
"
        .into(),
        "library" => "namespace Lib

def greet (who : String) : String := s!\"hello {who}\"

end Lib

def main : IO Unit :=
  IO.println (Lib.greet \"world\")
"
        .into(),
        other => {
            return Err(Error::invalid(format!(
                "unknown template {other}; use default, theorem, teaching, or library"
            )))
        }
    };
    let mut lakefile = String::from("import Lake\nopen Lake DSL\n\n");
    if mathlib {
        lakefile
            .push_str("require mathlib from git \"https://github.com/leanprover-community/mathlib4\"\n\n");
    }
    lakefile.push_str(&format!(
        "package «{name}» where\n\n@[default_target]\nlean_exe «{name}» where\n  root := `Main\n"
    ));
    let files: Vec<(&str, String)> = vec![
        ("lakefile.lean", lakefile),
        ("lean-toolchain", "leanprover/lean4:stable\n".into()),
        ("Main.lean", main_body),
        (
            "tpt-lean.toml",
            format!("[project]\nname = \"{name}\"\nlean_version = \"stable\"\nroot = \".\"\n"),
        ),
        (".gitignore", ".lake/\n.tpt-lean/\nbuild/\n".into()),
        (
            "README.md",
            format!("# {name}\n\nA Lean 4 project created with `tpt-lean init`.\n\n- Build: `tpt-lean build`\n- Check environment: `tpt-lean doctor`\n"),
        ),
    ];
    let mut written = Vec::new();
    for (file, body) in &files {
        std::fs::write(dir.join(file), body)?;
        written.push((*file).to_string());
    }
    if mathlib {
        ctx.note("note: pin lean-toolchain to the version your Mathlib revision requires before building");
    }
    let human = format!("created {name} ({} files)", written.len());
    ctx.emit(
        "init",
        &human,
        &InitResult {
            root: dir.display().to_string(),
            files: written,
        },
    )?;
    Ok(OK)
}

fn doctor(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&[], &[])?;
    let root = project_root(args)?;
    let config = load_config(args, &root).map(|_| ()).map_err(|e| e.to_string());
    let checks = tpt_lean_tools::project_doctor::project_checks(&root, config);
    let human: Vec<String> = checks
        .iter()
        .map(|c| {
            format!(
                "{:<11} {:<5} {}",
                format!("{}:", c.name),
                c.status.to_uppercase(),
                c.detail
            )
        })
        .collect();
    let failed = checks.iter().any(|c| c.status == "fail");
    ctx.emit_items("doctor", &human, &checks)?;
    Ok(if failed {
        ErrorCode::LeanNotFound.exit_code()
    } else {
        OK
    })
}

fn config_show(ctx: &Ctx, args: &Args) -> Result<i32> {
    let root = project_root(args)?;
    let (cfg, sources) = config::load_config(
        &root,
        &args
            .overrides
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect::<Vec<_>>(),
    )?;
    let toml = config::to_toml(&cfg)?;
    if ctx.verbose {
        eprintln!(
            "sources: files={:?} env={:?} overrides={:?}",
            sources.files, sources.env, sources.overrides
        );
    }
    ctx.emit("config", toml.trim_end(), &cfg)?;
    Ok(OK)
}

#[derive(Serialize)]
struct FmtReport {
    file: String,
    changed: bool,
    error: Option<String>,
}

fn fmt(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--check", "--diff", "--verify", "--project"], &["--project"])?;
    let root = project_root(args)?;
    let ws = WorkspaceRoot::new(&root)?;
    let check = args.flag("--check");
    let show_diff = args.flag("--diff");
    let verify = args.flag("--verify");
    // Lean and the timeout are only needed for --verify; load them lazily.
    let verifier = if verify {
        let cfg = load_config(args, &root)?;
        Some((
            adapter::discover_toolchain(),
            adapter::lean_policy(cfg.security.command_timeout_seconds),
        ))
    } else {
        None
    };
    let positional = args.positionals(&["--project"]);
    let files: Vec<PathBuf> = if positional.is_empty() {
        tpt_lean_index::collect_lean_files(&root)?
            .into_iter()
            .map(|(_, p)| p)
            .collect()
    } else {
        positional
            .iter()
            .map(|p| ws.resolve(p))
            .collect::<Result<Vec<_>>>()?
    };

    let mut reports = Vec::new();
    let mut human = Vec::new();
    let mut any_changed = false;
    let mut any_error = false;
    for path in &files {
        let rel = path.strip_prefix(ws.path()).unwrap_or(path).display().to_string();
        let src = std::fs::read_to_string(path).map_err(|e| Error::from(e).with_path(path))?;
        match tpt_lean_fmt::format_text(&src) {
            Err(e) => {
                any_error = true;
                human.push(format!("{rel}: skipped ({e})"));
                reports.push(FmtReport {
                    file: rel,
                    changed: false,
                    error: Some(e.to_string()),
                });
            }
            Ok(out) if out.changed => {
                if let Some((toolchain, policy)) = &verifier {
                    let outcome = adapter::verify::verify_format(
                        toolchain,
                        &root,
                        &src,
                        &out.output,
                        policy,
                        &tpt_lean_core::policy::CancelToken::new(),
                    );
                    let refusal = match outcome {
                        Ok(v) if v.verified => None,
                        Ok(v) => Some(format!(
                            "formatting adds {} Lean error(s): {}",
                            v.new_errors.len(),
                            v.new_errors.join("; ")
                        )),
                        Err(e) => Some(format!("could not verify with lean: {e}")),
                    };
                    if let Some(reason) = refusal {
                        any_error = true;
                        human.push(format!("{rel}: not formatted ({reason})"));
                        reports.push(FmtReport {
                            file: rel,
                            changed: false,
                            error: Some(reason),
                        });
                        continue;
                    }
                }
                any_changed = true;
                if show_diff {
                    if let Some(h) = tpt_lean_fmt::diff(&src, &out.output) {
                        human.push(tpt_lean_fmt::render_hunk(&rel, &h).trim_end().to_string());
                    }
                } else if check {
                    human.push(format!("would reformat {rel}"));
                } else {
                    std::fs::write(path, &out.output).map_err(|e| Error::from(e).with_path(path))?;
                    human.push(format!("formatted {rel}"));
                }
                reports.push(FmtReport {
                    file: rel,
                    changed: true,
                    error: None,
                });
            }
            Ok(_) => reports.push(FmtReport {
                file: rel,
                changed: false,
                error: None,
            }),
        }
    }
    if human.is_empty() && !check {
        human.push(format!("{} file(s) already formatted", files.len()));
    }
    ctx.emit_items("fmt", &human, &reports)?;
    if any_error {
        return Ok(ErrorCode::SyntaxInvalid.exit_code());
    }
    Ok(if check && any_changed { CHECK_FAILED } else { OK })
}

#[derive(Serialize)]
struct SearchItem {
    name: String,
    kind: String,
    package: String,
    file: String,
    line: i64,
    score: f64,
    docstring: Option<String>,
}

/// The query for `search --goal-at`: the goal target at a position, as the language server
/// reports it. Fails when there is no goal there.
fn goal_query_at(args: &Args, root: &Path, spec: &str) -> Result<String> {
    let (file, line, column) = parse_position(spec)?;
    let cfg = load_config(args, root)?;
    let ws = WorkspaceRoot::new(root)?;
    let path = ws.resolve(&file)?;
    let text = std::fs::read_to_string(&path).map_err(|e| Error::from(e).with_path(&path))?;
    let snap = adapter::explain::snapshot_at(
        root,
        &path,
        &file,
        &text,
        line,
        column,
        std::time::Duration::from_secs(cfg.security.command_timeout_seconds),
    )?;
    let target = snap
        .goals
        .first()
        .and_then(|g| split_goal(g).1)
        .ok_or_else(|| Error::invalid(format!("no goal at {spec}")))?;
    let query = adapter::explain::goal_query(&target);
    if query.is_empty() {
        return Err(Error::invalid(format!(
            "the goal at {spec} has no searchable terms"
        )));
    }
    Ok(query)
}

fn search(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &["--limit", "--namespace", "--near", "--project", "--goal-at"],
        &["--limit", "--namespace", "--near", "--project", "--goal-at"],
    )?;
    let positional = args.positionals(&["--limit", "--namespace", "--near", "--project", "--goal-at"]);
    let root = project_root(args)?;
    let goal_at = args.value("--goal-at")?;
    let query = match goal_at {
        Some(spec) => {
            if !positional.is_empty() {
                return Err(Error::invalid("give either a query or --goal-at, not both"));
            }
            goal_query_at(args, &root, &spec)?
        }
        None => {
            if positional.is_empty() {
                return Err(Error::invalid(
                    "usage: tpt-lean search <query> | --goal-at FILE:LINE:COL [--namespace NS] [--near TERM]... [--limit N]",
                ));
            }
            positional.join(" ")
        }
    };
    let limit = match args.value("--limit")? {
        Some(v) => v
            .parse::<usize>()
            .map_err(|_| Error::invalid("--limit must be a number"))?,
        None => 10,
    };
    let dirs = ProjectDirs::new(&root);
    if !dirs.index_db().exists() {
        return Err(Error::new(
            ErrorCode::IndexError,
            "no index yet; run `tpt-lean index scan` first",
        )
        .with_path(dirs.index_db()));
    }
    let index = Index::open(&dirs.index_db())?;
    let opts = SearchOptions {
        limit,
        namespace: args.value("--namespace")?,
        near: args.values("--near"),
        imported_modules: Vec::new(),
    };
    let hits = tpt_lean_search::search(&index, &query, &opts)?;
    let items: Vec<SearchItem> = hits
        .into_iter()
        .map(|h| SearchItem {
            name: h.name,
            kind: h.kind,
            package: h.package,
            file: h.file,
            line: h.line,
            score: (h.score * 1000.0).round() / 1000.0,
            docstring: h.docstring,
        })
        .collect();
    let human: Vec<String> = if items.is_empty() {
        vec![format!("no matches for {query:?}")]
    } else {
        items
            .iter()
            .map(|i| {
                format!(
                    "{:.3}  {:<9} {}  ({}:{})",
                    i.score, i.kind, i.name, i.file, i.line
                )
            })
            .collect()
    };
    ctx.emit_items("search", &human, &items)?;
    Ok(OK)
}

#[derive(Serialize)]
struct RefreshReport {
    scanned: usize,
    updated: usize,
    unchanged: usize,
    removed: usize,
    db: String,
}

fn index_refresh(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--project"], &["--project"])?;
    let root = project_root(args)?;
    let cfg = load_config(args, &root)?;
    let dirs = ProjectDirs::new(&root);
    dirs.ensure()?;
    let mut index = Index::open(&dirs.index_db())?;
    let roots = default_scan_roots(&root, cfg.index.mathlib);
    ctx.note(&format!("scanning {} source root(s)", roots.len()));
    let stats = index.refresh(&roots)?;
    let human = format!(
        "scanned {} files: {} updated, {} unchanged, {} removed",
        stats.scanned, stats.updated, stats.unchanged, stats.removed
    );
    ctx.emit(
        "index.refresh",
        &human,
        &RefreshReport {
            scanned: stats.scanned,
            updated: stats.updated,
            unchanged: stats.unchanged,
            removed: stats.removed,
            db: dirs.index_db().display().to_string(),
        },
    )?;
    Ok(OK)
}

fn index_status(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--project"], &["--project"])?;
    let root = project_root(args)?;
    let dirs = ProjectDirs::new(&root);
    if !dirs.index_db().exists() {
        return Err(
            Error::new(ErrorCode::IndexError, "no index yet; run `tpt-lean index scan`")
                .with_path(dirs.index_db()),
        );
    }
    let status = Index::open(&dirs.index_db())?.status()?;
    let human = format!(
        "schema v{}  files {}  symbols {}  imports {}  packages [{}]  integrity {}",
        status.schema_version,
        status.files,
        status.symbols,
        status.imports,
        status.packages.join(", "),
        if status.integrity_ok { "ok" } else { "FAILED" }
    );
    ctx.emit(
        "index.status",
        &human,
        &serde_json::json!({
            "schema_version": status.schema_version,
            "files": status.files,
            "symbols": status.symbols,
            "imports": status.imports,
            "packages": status.packages,
            "integrity_ok": status.integrity_ok,
        }),
    )?;
    Ok(if status.integrity_ok {
        OK
    } else {
        ErrorCode::IndexError.exit_code()
    })
}

#[derive(Serialize)]
struct SidecarFailure {
    module: String,
    reason: String,
}

#[derive(Serialize)]
struct SidecarReport {
    stored: Vec<String>,
    failed: Vec<SidecarFailure>,
}

/// `index sidecar`: describe the project's built modules with the Lean sidecar and store the
/// declarations in the index. Modules that are already current are skipped.
fn index_sidecar(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &["--project", "--exe", "--timeout"],
        &["--project", "--exe", "--timeout"],
    )?;
    let root = project_root(args)?;
    let cfg = load_config(args, &root)?;
    let dirs = ProjectDirs::new(&root);
    let exe = match args.value("--exe")? {
        Some(path) => PathBuf::from(path),
        None => std::env::var_os("TPT_LEAN_SIDECAR")
            .map(PathBuf::from)
            .ok_or_else(|| {
                Error::invalid(
                    "sidecar not found: pass --exe or set TPT_LEAN_SIDECAR to the built tpt_sidecar binary (build lean/tpt-lean-sidecar with `lake build`)",
                )
            })?,
    };
    let timeout = match args.value("--timeout")? {
        Some(v) => v
            .parse::<u64>()
            .map_err(|_| Error::invalid("--timeout must be seconds"))?,
        None => cfg.security.command_timeout_seconds,
    };
    let toolchain = adapter::discover_toolchain();
    let mut index = Index::open(&dirs.index_db())?;
    let stats = adapter::sidecar::index_sidecar(
        &mut index,
        &root,
        &exe,
        &toolchain,
        &adapter::lean_policy(timeout),
        &CancelToken::new(),
    )?;
    let mut human = vec![format!(
        "stored declarations for {} module(s); {} failed",
        stats.stored.len(),
        stats.failed.len()
    )];
    human.extend(
        stats
            .failed
            .iter()
            .map(|(module, reason)| format!("  {module}: {}", reason.lines().next().unwrap_or(""))),
    );
    let report = SidecarReport {
        stored: stats.stored,
        failed: stats
            .failed
            .into_iter()
            .map(|(module, reason)| SidecarFailure { module, reason })
            .collect(),
    };
    let ok = report.failed.is_empty();
    ctx.emit("index.sidecar", &human.join("\n"), &report)?;
    Ok(if ok { OK } else { ErrorCode::IndexError.exit_code() })
}

fn index_clear(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--project"], &["--project"])?;
    let root = project_root(args)?;
    let dirs = ProjectDirs::new(&root);
    let mut index = Index::open(&dirs.index_db())?;
    index.clear()?;
    ctx.emit(
        "index.clear",
        "index cleared",
        &serde_json::json!({ "cleared": true }),
    )?;
    Ok(OK)
}

fn check(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--project", "--timeout"], &["--project", "--timeout"])?;
    let root = project_root(args)?;
    let cfg = load_config(args, &root)?;
    let timeout = match args.value("--timeout")? {
        Some(t) => t
            .parse::<u64>()
            .map_err(|_| Error::invalid("--timeout must be seconds"))?,
        None => cfg.security.command_timeout_seconds,
    };
    let command = match args.positionals(&["--project", "--timeout"]).into_iter().next() {
        None => LeanCommand::LakeBuild { target: None },
        Some(file) => {
            let ws = WorkspaceRoot::new(&root)?;
            LeanCommand::LeanCheck {
                file: ws.resolve(&file)?,
            }
        }
    };
    run_lean(ctx, &root, command, timeout, "check")
}

#[derive(Serialize)]
struct LeanRunReport {
    exit_code: Option<i32>,
    timed_out: bool,
    output_truncated: bool,
    duration_ms: u128,
    diagnostics: Vec<adapter::Diagnostic>,
}

fn run_lean(ctx: &Ctx, root: &Path, command: LeanCommand, timeout: u64, kind: &str) -> Result<i32> {
    let tc = adapter::discover_toolchain();
    let policy = adapter::lean_policy(timeout);
    let result = adapter::execute(&tc, &command, Some(root), &policy, &CancelToken::new())?;
    // Keep the last diagnostics so `explain --last-error` can show them later.
    let source = match &command {
        LeanCommand::LeanCheck { file } => std::fs::read_to_string(file).unwrap_or_default(),
        _ => String::new(),
    };
    let _ = adapter::save_last_diagnostics(root, &source, &result.diagnostics);
    let out = &result.output;
    let errors = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == adapter::Severity::Error)
        .count();
    let human = if out.timed_out {
        format!("{kind} timed out after {timeout}s")
    } else if out.success() {
        format!("{kind} succeeded ({} ms)", out.duration.as_millis())
    } else {
        let mut lines = vec![format!("{kind} failed with {errors} error(s)")];
        for d in &result.diagnostics {
            lines.push(format!(
                "{}:{}:{}: {:?}: {}",
                d.file, d.line, d.column, d.severity, d.message
            ));
        }
        lines.join("\n")
    };
    if ctx.mode == Mode::Human
        && !out.success()
        && !out.timed_out
        && result.diagnostics.is_empty()
        && !out.stderr.trim().is_empty()
    {
        eprintln!("{}", out.stderr.trim_end());
    }
    ctx.emit(
        kind,
        &human,
        &LeanRunReport {
            exit_code: out.exit_code,
            timed_out: out.timed_out,
            output_truncated: out.output_truncated,
            duration_ms: out.duration.as_millis(),
            diagnostics: result.diagnostics.clone(),
        },
    )?;
    if out.timed_out {
        return Ok(ErrorCode::Timeout.exit_code());
    }
    Ok(if out.success() { OK } else { 1 })
}

/// `tpt-lean mcp stdio`: serve MCP over stdin/stdout. Protocol traffic owns stdout, so no
/// human output is printed here.
fn mcp_stdio(_ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--project"], &["--project"])?;
    let root = project_root(args)?;
    let server = tpt_lean_mcp::Server::new(&root)?;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    tpt_lean_mcp::serve_stdio(&server, stdin.lock(), stdout.lock())?;
    Ok(OK)
}

/// `tpt-lean server start [--port N]`: serve the JSON-RPC API in the foreground.
/// Binds 127.0.0.1 unless `server.allow_remote` is set.
fn server_start(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--project", "--port"], &["--project", "--port"])?;
    let root = project_root(args)?;
    let port: u16 = match args.value("--port")? {
        Some(p) => p.parse().map_err(|_| Error::invalid("--port must be 0-65535"))?,
        None => 7373,
    };
    let server = tpt_lean_server::Server::new(&root)?;
    ctx.note(&format!(
        "serving JSON-RPC on {}:{port} (Ctrl+C to stop)",
        server.bind_host()
    ));
    tpt_lean_server::serve(server, port).map_err(Error::from)?;
    Ok(OK)
}

#[derive(Serialize)]
struct CleanReport {
    ran: bool,
    exit_code: Option<i32>,
}

/// `tpt-lean clean`: run `lake clean` in the project through the typed adapter.
fn clean(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--project", "--timeout"], &["--project", "--timeout"])?;
    let root = project_root(args)?;
    let cfg = load_config(args, &root)?;
    let tc = adapter::discover_toolchain();
    let policy = adapter::lean_policy(cfg.security.command_timeout_seconds);
    let result = adapter::execute(
        &tc,
        &LeanCommand::LakeClean,
        Some(&root),
        &policy,
        &CancelToken::new(),
    )?;
    let ok = result.output.success();
    let human = if ok {
        "clean succeeded".to_string()
    } else {
        "clean failed".to_string()
    };
    ctx.emit(
        "clean",
        &human,
        &CleanReport {
            ran: true,
            exit_code: result.output.exit_code,
        },
    )?;
    Ok(if ok { OK } else { 1 })
}

/// `tpt-lean cache prune [--days N] [--dry-run]`: delete global cache files older than N days.
/// Only files inside the tpt-lean global cache directory are considered.
#[derive(Serialize)]
struct ImportEntry {
    file: String,
    module: String,
    line: u32,
}

/// `tpt-lean imports [file]`: list `import` lines. Without a file, list every project file.
fn imports(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--project"], &["--project"])?;
    let root = project_root(args)?;
    let ws = WorkspaceRoot::new(&root)?;
    let targets: Vec<PathBuf> = match args.positionals(&["--project"]).first() {
        Some(f) => vec![ws.resolve(f)?],
        None => tpt_lean_index::collect_lean_files(&root)?
            .into_iter()
            .map(|(_, p)| p)
            .collect(),
    };
    let mut entries = Vec::new();
    for path in &targets {
        let src = std::fs::read_to_string(path).map_err(|e| Error::from(e).with_path(path))?;
        let rel = path.strip_prefix(ws.path()).unwrap_or(path).display().to_string();
        for imp in tpt_lean_syntax::parse(&src).imports {
            entries.push(ImportEntry {
                file: rel.clone(),
                module: imp.module,
                line: imp.line as u32,
            });
        }
    }
    let human: Vec<String> = if entries.is_empty() {
        vec!["no imports found".into()]
    } else {
        entries
            .iter()
            .map(|e| format!("{}:{}  import {}", e.file, e.line, e.module))
            .collect()
    };
    ctx.emit_items("imports", &human, &entries)?;
    Ok(OK)
}

#[derive(Serialize)]
struct GoalsReport {
    file: String,
    line: usize,
    column: usize,
    goals: Vec<String>,
}

/// Split `file:line:col` from the right, so Windows drive letters survive.
fn parse_position(spec: &str) -> Result<(String, usize, usize)> {
    let mut parts = spec.rsplitn(3, ':');
    let col = parts.next().and_then(|c| c.parse::<usize>().ok());
    let line = parts.next().and_then(|l| l.parse::<usize>().ok());
    let file = parts.next();
    match (file, line, col) {
        (Some(f), Some(l), Some(c)) if !f.is_empty() && l >= 1 && c >= 1 => Ok((f.to_string(), l, c)),
        _ => Err(Error::invalid(
            "expected FILE:LINE:COLUMN with 1-based line and column",
        )),
    }
}

/// `tpt-lean goals FILE:LINE:COL`: plain goals at a position, from the Lean language server.
/// Line and column are 1-based; the column counts characters.
fn goals(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--project", "--timeout"], &["--project", "--timeout"])?;
    let spec = args
        .positionals(&["--project", "--timeout"])
        .into_iter()
        .next()
        .ok_or_else(|| Error::invalid("usage: tpt-lean goals FILE:LINE:COL"))?;
    let (file, line, column) = parse_position(&spec)?;
    let root = project_root(args)?;
    let cfg = load_config(args, &root)?;
    let ws = WorkspaceRoot::new(&root)?;
    let path = ws.resolve(&file)?;
    let text = std::fs::read_to_string(&path).map_err(|e| Error::from(e).with_path(&path))?;
    let index = tpt_lean_syntax::position::LineIndex::new(&text);
    let offset = index
        .offset_of(line, column)
        .ok_or_else(|| Error::invalid(format!("{file}:{line}:{column} is outside the file")))?;
    let utf16 = index.position(offset).column_utf16 as u32;
    let timeout = match args.value("--timeout")? {
        Some(t) => t
            .parse::<u64>()
            .map_err(|_| Error::invalid("--timeout must be seconds"))?,
        None => cfg.security.command_timeout_seconds,
    };
    let tc = adapter::discover_toolchain();
    let (program, server_args) = adapter::lsp::server_command(&root, tc.lake.as_ref(), tc.lean.as_ref())?;
    let limit = std::time::Duration::from_secs(timeout);
    let mut client = adapter::lsp::LspClient::start(&program, &server_args, &root, limit)?;
    let uri = adapter::lsp::open_document(&mut client, &path, &text)?;
    let result = adapter::lsp::plain_goals(&mut client, &uri, (line - 1) as u32, utf16, limit);
    client.shutdown(std::time::Duration::from_secs(5));
    let goals = result?;
    let human = if goals.is_empty() {
        format!("no goals at {file}:{line}:{column}")
    } else {
        goals.join("\n\n")
    };
    ctx.emit(
        "goals",
        &human,
        &GoalsReport {
            file,
            line,
            column,
            goals,
        },
    )?;
    Ok(OK)
}

#[derive(Serialize)]
struct ExplainContext {
    goal: Option<String>,
    hypotheses: Vec<String>,
}

#[derive(Serialize)]
struct ExplainReport {
    lean: serde_json::Value,
    error: Option<adapter::explain::ExplainError>,
    context: ExplainContext,
    suggestions: Vec<serde_json::Value>,
}

/// `tpt-lean explain FILE:LINE:COL`: diagnostics at a position plus the goal there.
/// Lemma suggestions come from the project index when one exists; otherwise the list is empty.
fn explain(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--project", "--timeout"], &["--project", "--timeout"])?;
    let spec = args
        .positionals(&["--project", "--timeout"])
        .into_iter()
        .next()
        .ok_or_else(|| Error::invalid("usage: tpt-lean explain FILE:LINE:COL"))?;
    let (file, line, column) = parse_position(&spec)?;
    let root = project_root(args)?;
    let cfg = load_config(args, &root)?;
    let ws = WorkspaceRoot::new(&root)?;
    let path = ws.resolve(&file)?;
    let text = std::fs::read_to_string(&path).map_err(|e| Error::from(e).with_path(&path))?;
    let timeout = match args.value("--timeout")? {
        Some(t) => t
            .parse::<u64>()
            .map_err(|_| Error::invalid("--timeout must be seconds"))?,
        None => cfg.security.command_timeout_seconds,
    };
    let snap = adapter::explain::snapshot_at(
        &root,
        &path,
        &file,
        &text,
        line,
        column,
        std::time::Duration::from_secs(timeout),
    )?;
    let error = adapter::explain::error_at(&snap.diagnostics, &file, snap.line0, snap.utf16);

    let (hypotheses, target) = match snap.goals.first() {
        Some(g) => split_goal(g),
        None => (Vec::new(), None),
    };
    let mut human = Vec::new();
    match &error {
        Some(e) => human.push(format!(
            "error at {}:{}:{}: {}",
            e.file, e.line, e.column, e.message
        )),
        None => human.push(format!("no error at {file}:{line}:{column}")),
    }
    if let Some(g) = snap.goals.first() {
        human.push(String::new());
        human.push(g.clone());
    }
    let suggestions = target
        .as_deref()
        .map(|t| ext::lemma_candidates(&root, t, 5))
        .unwrap_or_default();
    if snap.compatibility != adapter::compat::Compatibility::Tested {
        human.push(format!(
            "warning: Lean {} is not a tested version; results may differ",
            snap.lean_version.as_deref().unwrap_or("(unknown)")
        ));
    }
    let report = ExplainReport {
        lean: adapter::explain::lean_json(&snap.lean_version, snap.compatibility),
        error,
        context: ExplainContext {
            goal: target,
            hypotheses,
        },
        suggestions,
    };
    ctx.emit("explain", &human.join("\n"), &report)?;
    Ok(OK)
}

/// `tpt-lean context-pack FILE:LINE:COL [--max-tokens N]`: a compact bundle for AI agents.
/// Lean supplies the goal and diagnostics; the index supplies lemma candidates. The bundle is
/// built by the adapter, which the JSON-RPC server also uses.
fn context_pack(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &["--project", "--timeout", "--max-tokens"],
        &["--project", "--timeout", "--max-tokens"],
    )?;
    let spec = args
        .positionals(&["--project", "--timeout", "--max-tokens"])
        .into_iter()
        .next()
        .ok_or_else(|| Error::invalid("usage: tpt-lean context-pack FILE:LINE:COL [--max-tokens N]"))?;
    let (file, line, column) = parse_position(&spec)?;
    let max_tokens: usize = match args.value("--max-tokens")? {
        Some(v) => v
            .parse()
            .map_err(|_| Error::invalid("--max-tokens must be a number"))?,
        None => 6000,
    };
    let root = project_root(args)?;
    let cfg = load_config(args, &root)?;
    let ws = WorkspaceRoot::new(&root)?;
    let path = ws.resolve(&file)?;
    let text = std::fs::read_to_string(&path).map_err(|e| Error::from(e).with_path(&path))?;
    let timeout = match args.value("--timeout")? {
        Some(t) => t
            .parse::<u64>()
            .map_err(|_| Error::invalid("--timeout must be seconds"))?,
        None => cfg.security.command_timeout_seconds,
    };
    let pack = adapter::context::build_pack(&adapter::context::PackRequest {
        root: &root,
        path: &path,
        label: &file,
        text: &text,
        line,
        column,
        max_tokens,
        timeout: std::time::Duration::from_secs(timeout),
        command_timeout_seconds: cfg.security.command_timeout_seconds,
    })?;

    let mut human = vec![format!(
        "context for {file}:{line}:{column} (~{} tokens)",
        pack.approx_tokens
    )];
    human.push(pack.snippet.clone());
    if let Some(g) = &pack.goal {
        human.push(format!("goal:\n{g}"));
    }
    for d in &pack.diagnostics {
        human.push(format!(
            "{}:{}:{}: {}: {}",
            file, d.line, d.column, d.severity, d.message
        ));
    }
    ctx.emit("context-pack", &human.join("\n\n"), &pack)?;
    Ok(OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_parses_windows_paths() {
        let (f, l, c) = parse_position(r"C:\proj\Main.lean:12:7").unwrap();
        assert_eq!((f.as_str(), l, c), (r"C:\proj\Main.lean", 12, 7));
        assert!(parse_position("Main.lean:0:1").is_err());
        assert!(parse_position("Main.lean:3").is_err());
    }

    #[test]
    fn goal_is_split_into_hypotheses_and_target() {
        let (hyps, target) = split_goal("a b : Nat\nh : a = b\n⊢ b = a");
        assert_eq!(hyps, vec!["a b : Nat".to_string(), "h : a = b".to_string()]);
        assert_eq!(target.as_deref(), Some("b = a"));
    }

    #[test]
    fn token_estimate_is_monotonic() {
        assert_eq!(tpt_lean_adapter::context::approx_tokens(""), 0);
        assert!(
            tpt_lean_adapter::context::approx_tokens("abcdefgh")
                < tpt_lean_adapter::context::approx_tokens("abcdefghijklmnop")
        );
    }
}
