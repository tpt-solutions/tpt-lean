// SPDX-License-Identifier: MIT OR Apache-2.0
//! Command implementations and dispatch.

use crate::args::Args;
use crate::{Ctx, Mode};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tpt_lean_adapter::{self as adapter, Command as LeanCommand};
use tpt_lean_core::config::{self, Config};
use tpt_lean_core::dirs::{global_cache_dir, ProjectDirs};
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
    if args.command.is_empty() || args.rest.first().map(|s| s.as_str()) == Some("--help") {
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
        ["mcp", "stdio"] => mcp_stdio(ctx, args),
        ["server", "start"] => server_start(ctx, args),
        ["build"] => build(ctx, args),
        ["check"] => check(ctx, args),
        ["cache", "status"] => cache_status(ctx, args),
        ["cache", "prune"] => cache_prune(ctx, args),
        ["clean"] => clean(ctx, args),
        ["imports"] => imports(ctx, args),
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

#[derive(Serialize)]
struct Check {
    name: &'static str,
    status: &'static str,
    detail: String,
}

fn doctor(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&[], &[])?;
    let mut checks: Vec<Check> = Vec::new();
    let tc = adapter::discover_toolchain();
    let cancel = CancelToken::new();
    let policy = adapter::lean_policy(10);

    checks.push(match &tc.lean {
        Some(_) => match adapter::execute(&tc, &LeanCommand::LeanVersion, None, &policy, &cancel) {
            Ok(r) => Check {
                name: "lean",
                status: "ok",
                detail: adapter::parse_version(&r.output.stdout)
                    .unwrap_or_else(|| r.output.stdout.trim().to_string()),
            },
            Err(e) => Check {
                name: "lean",
                status: "fail",
                detail: e.to_string(),
            },
        },
        None => Check {
            name: "lean",
            status: "fail",
            detail: "lean not found on PATH or in ~/.elan/bin".into(),
        },
    });
    checks.push(match &tc.lake {
        Some(_) => match adapter::execute(&tc, &LeanCommand::LakeVersion, None, &policy, &cancel) {
            Ok(r) => Check {
                name: "lake",
                status: "ok",
                detail: adapter::parse_version(&r.output.stdout)
                    .unwrap_or_else(|| r.output.stdout.trim().to_string()),
            },
            Err(e) => Check {
                name: "lake",
                status: "fail",
                detail: e.to_string(),
            },
        },
        None => Check {
            name: "lake",
            status: "fail",
            detail: "lake not found on PATH or in ~/.elan/bin".into(),
        },
    });

    let root = project_root(args)?;
    match project::discover(&root) {
        Ok(p) => {
            checks.push(Check {
                name: "project",
                status: "ok",
                detail: format!("found {} at {}", p.marker, p.root.display()),
            });
            match p.toolchain {
                Some(t) => checks.push(Check {
                    name: "toolchain",
                    status: "ok",
                    detail: format!("lean-toolchain pins {t}"),
                }),
                None => checks.push(Check {
                    name: "toolchain",
                    status: "warn",
                    detail: "no lean-toolchain file".into(),
                }),
            }
        }
        Err(_) => checks.push(Check {
            name: "project",
            status: "warn",
            detail: format!("no project marker above {}", root.display()),
        }),
    }

    match load_config(args, &root) {
        Ok(_) => checks.push(Check {
            name: "config",
            status: "ok",
            detail: "configuration loads".into(),
        }),
        Err(e) => checks.push(Check {
            name: "config",
            status: "fail",
            detail: e.to_string(),
        }),
    }

    let dirs = ProjectDirs::new(&root);
    match dirs.ensure() {
        Ok(()) => checks.push(Check {
            name: "write",
            status: "ok",
            detail: format!("{} is writable", dirs.root.display()),
        }),
        Err(e) => checks.push(Check {
            name: "write",
            status: "fail",
            detail: e.to_string(),
        }),
    }

    if dirs.index_db().exists() {
        match Index::open(&dirs.index_db()).and_then(|i| i.status()) {
            Ok(s) if s.integrity_ok => checks.push(Check {
                name: "index",
                status: "ok",
                detail: format!("{} files, {} symbols", s.files, s.symbols),
            }),
            Ok(_) => checks.push(Check {
                name: "index",
                status: "fail",
                detail: "integrity check failed; run `index clear` then `index scan`".into(),
            }),
            Err(e) => checks.push(Check {
                name: "index",
                status: "fail",
                detail: e.to_string(),
            }),
        }
    } else {
        checks.push(Check {
            name: "index",
            status: "warn",
            detail: "not built yet; run `tpt-lean index scan`".into(),
        });
    }

    let mathlib = root.join(".lake").join("packages").join("mathlib");
    checks.push(if mathlib.is_dir() {
        Check {
            name: "mathlib",
            status: "ok",
            detail: "present in .lake/packages".into(),
        }
    } else {
        Check {
            name: "mathlib",
            status: "warn",
            detail: "not present (only needed for Mathlib projects)".into(),
        }
    });

    match global_cache_dir() {
        Some(d) => checks.push(Check {
            name: "cache_dir",
            status: "ok",
            detail: d.display().to_string(),
        }),
        None => checks.push(Check {
            name: "cache_dir",
            status: "warn",
            detail: "no cache directory could be determined".into(),
        }),
    }

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
    if args.flag("--verify") {
        return not_implemented("`fmt --verify` (needs the Lean adapter, Phase 2)");
    }
    let root = project_root(args)?;
    let ws = WorkspaceRoot::new(&root)?;
    let check = args.flag("--check");
    let show_diff = args.flag("--diff");
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

fn search(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &["--limit", "--namespace", "--near", "--project"],
        &["--limit", "--namespace", "--near", "--project"],
    )?;
    let positional = args.positionals(&["--limit", "--namespace", "--near", "--project"]);
    if positional.is_empty() {
        return Err(Error::invalid(
            "usage: tpt-lean search <query> [--namespace NS] [--near TERM]... [--limit N]",
        ));
    }
    let query = positional.join(" ");
    let limit = match args.value("--limit")? {
        Some(v) => v
            .parse::<usize>()
            .map_err(|_| Error::invalid("--limit must be a number"))?,
        None => 10,
    };
    let root = project_root(args)?;
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

fn build(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--project", "--timeout"], &["--project", "--timeout"])?;
    let root = project_root(args)?;
    let cfg = load_config(args, &root)?;
    if !cfg.security.allow_build {
        return Err(Error::new(
            ErrorCode::InvalidArgument,
            "building is disabled by security.allow_build",
        ));
    }
    let target = args.positionals(&["--project", "--timeout"]).into_iter().next();
    let timeout = match args.value("--timeout")? {
        Some(t) => t
            .parse::<u64>()
            .map_err(|_| Error::invalid("--timeout must be seconds"))?,
        None => cfg.security.command_timeout_seconds,
    };
    run_lean(ctx, &root, LeanCommand::LakeBuild { target }, timeout, "build")
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

fn cache_status(ctx: &Ctx, _args: &Args) -> Result<i32> {
    let dir = global_cache_dir();
    let (exists, bytes) = match &dir {
        Some(d) if d.is_dir() => (true, dir_size(d)),
        _ => (false, 0),
    };
    let location = dir
        .as_ref()
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| "unknown".into());
    let human = if exists {
        format!("global cache: {location} ({bytes} bytes)")
    } else {
        format!("global cache: {location} (not created yet)")
    };
    ctx.emit(
        "cache.status",
        &human,
        &serde_json::json!({ "location": location, "exists": exists, "bytes": bytes }),
    )?;
    Ok(OK)
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
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

#[derive(Serialize)]
struct PruneReport {
    dry_run: bool,
    removed_files: usize,
    freed_bytes: u64,
    directory: String,
}

/// `tpt-lean cache prune [--days N] [--dry-run]`: delete global cache files older than N days.
/// Only files inside the tpt-lean global cache directory are considered.
fn cache_prune(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--days", "--dry-run"], &["--days"])?;
    let days: u64 = match args.value("--days")? {
        Some(d) => d.parse().map_err(|_| Error::invalid("--days must be a number"))?,
        None => 30,
    };
    let dry_run = args.flag("--dry-run");
    let Some(dir) = global_cache_dir() else {
        return Err(Error::new(
            ErrorCode::Io,
            "no global cache directory could be determined",
        ));
    };
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(days * 86_400))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let mut removed = 0usize;
    let mut freed = 0u64;
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() && meta.modified().map(|m| m < cutoff).unwrap_or(false) {
                removed += 1;
                freed += meta.len();
                if !dry_run {
                    std::fs::remove_file(&path).map_err(|e| Error::from(e).with_path(&path))?;
                }
            }
        }
    }
    let verb = if dry_run { "would remove" } else { "removed" };
    let human = format!("{verb} {removed} file(s), {freed} bytes older than {days} day(s)");
    ctx.emit(
        "cache.prune",
        &human,
        &PruneReport {
            dry_run,
            removed_files: removed,
            freed_bytes: freed,
            directory: dir.display().to_string(),
        },
    )?;
    Ok(OK)
}

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
