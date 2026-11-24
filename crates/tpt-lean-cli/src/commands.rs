// SPDX-License-Identifier: MIT OR Apache-2.0
//! Command implementations and dispatch.

mod ext;

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

    for c in tpt_lean_tools::doctor::doctor_extensions(&root) {
        let status = match c.status {
            tpt_lean_tools::doctor::CheckStatus::Ok => "ok",
            tpt_lean_tools::doctor::CheckStatus::Warn => "warn",
            tpt_lean_tools::doctor::CheckStatus::Fail => "fail",
            tpt_lean_tools::doctor::CheckStatus::Skipped => "skipped",
        };
        let detail = match &c.suggestion {
            Some(s) => format!("{} (suggestion: {s})", c.detail),
            None => c.detail.clone(),
        };
        checks.push(Check {
            name: c.name,
            status,
            detail,
        });
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
struct ExplainError {
    message: String,
    file: String,
    line: u64,
    column: u64,
}

#[derive(Serialize)]
struct ExplainContext {
    goal: Option<String>,
    hypotheses: Vec<String>,
}

#[derive(Serialize)]
struct ExplainReport {
    error: Option<ExplainError>,
    context: ExplainContext,
    suggestions: Vec<serde_json::Value>,
}

/// Split a plain goal into hypotheses (lines before `⊢`) and the target.
fn split_goal(goal: &str) -> (Vec<String>, Option<String>) {
    let mut hyps = Vec::new();
    let mut target = None;
    for line in goal.lines() {
        if let Some(t) = line.strip_prefix("⊢ ") {
            target = Some(t.to_string());
        } else if !line.trim().is_empty() {
            hyps.push(line.to_string());
        }
    }
    (hyps, target)
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
    let goals = adapter::lsp::plain_goals(&mut client, &uri, (line - 1) as u32, utf16, limit);
    let diags = client.diagnostics_for(&uri);
    client.shutdown(std::time::Duration::from_secs(5));
    let goals = goals?;

    let (hypotheses, target) = match goals.first() {
        Some(g) => split_goal(g),
        None => (Vec::new(), None),
    };
    let target_line = (line - 1) as u64;
    let at_position = diags.iter().find(|d| {
        let s = &d["range"]["start"];
        let e = &d["range"]["end"];
        let (sl, sc) = (
            s["line"].as_u64().unwrap_or(0),
            s["character"].as_u64().unwrap_or(0),
        );
        let (el, ec) = (
            e["line"].as_u64().unwrap_or(0),
            e["character"].as_u64().unwrap_or(0),
        );
        let p = (target_line, utf16 as u64);
        (sl, sc) <= p && p <= (el, ec) && d["severity"].as_u64().unwrap_or(1) == 1
    });
    let error = at_position.map(|d| ExplainError {
        message: d["message"].as_str().unwrap_or("").to_string(),
        file: file.clone(),
        line: d["range"]["start"]["line"].as_u64().unwrap_or(0) + 1,
        column: d["range"]["start"]["character"].as_u64().unwrap_or(0) + 1,
    });
    let mut human = Vec::new();
    match &error {
        Some(e) => human.push(format!(
            "error at {}:{}:{}: {}",
            e.file, e.line, e.column, e.message
        )),
        None => human.push(format!("no error at {file}:{line}:{column}")),
    }
    if let Some(g) = goals.first() {
        human.push(String::new());
        human.push(g.clone());
    }
    let suggestions = target
        .as_deref()
        .map(|t| ext::lemma_candidates(&root, t, 5))
        .unwrap_or_default();
    let report = ExplainReport {
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

#[derive(Serialize)]
struct PackPosition {
    line: usize,
    column: usize,
}

#[derive(Serialize)]
struct PackDiagnostic {
    severity: String,
    line: u64,
    column: u64,
    message: String,
}

#[derive(Serialize)]
struct PackSymbol {
    name: String,
    kind: String,
    line: usize,
}

#[derive(Serialize)]
struct PackLemma {
    name: String,
    source: String,
    line: i64,
    score: f64,
}

#[derive(Serialize)]
struct ContextPack {
    file: String,
    position: PackPosition,
    snippet: String,
    goal: Option<String>,
    hypotheses: Vec<String>,
    diagnostics: Vec<PackDiagnostic>,
    imports: Vec<String>,
    nearby_definitions: Vec<PackSymbol>,
    relevant_lemmas: Vec<PackLemma>,
    constraints: Vec<String>,
    truncated: bool,
    approx_tokens: usize,
}

/// Rough token estimate: about four characters per token.
fn approx_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// `tpt-lean context-pack FILE:LINE:COL [--max-tokens N]`: a compact bundle for AI agents.
/// Lean supplies the goal and diagnostics; the index supplies lemma candidates.
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

    // Lean: goal and diagnostics.
    let tc = adapter::discover_toolchain();
    let (program, server_args) = adapter::lsp::server_command(&root, tc.lake.as_ref(), tc.lean.as_ref())?;
    let limit = std::time::Duration::from_secs(timeout);
    let mut client = adapter::lsp::LspClient::start(&program, &server_args, &root, limit)?;
    let uri = adapter::lsp::open_document(&mut client, &path, &text)?;
    let goals = adapter::lsp::plain_goals(&mut client, &uri, (line - 1) as u32, utf16, limit);
    let diags = client.diagnostics_for(&uri);
    client.shutdown(std::time::Duration::from_secs(5));
    let goals = goals?;
    let goal = goals.first().cloned();
    let (hypotheses, target) = match &goal {
        Some(g) => split_goal(g),
        None => (Vec::new(), None),
    };

    // Source: a window of lines around the position.
    let parse = tpt_lean_syntax::parse(&text);
    let total = index.line_count();
    let from = line.saturating_sub(6).max(1);
    let to = (line + 6).min(total);
    let mut snippet_lines = Vec::new();
    for n in from..=to {
        let start = index.line_start(n).unwrap_or(0);
        let end = index.line_start(n + 1).unwrap_or(text.len());
        let content = text[start..end].trim_end_matches(['\r', '\n']);
        snippet_lines.push(format!("{n:>5} | {content}"));
    }
    let snippet = snippet_lines.join("\n");

    let diagnostics: Vec<PackDiagnostic> = diags
        .iter()
        .map(|d| PackDiagnostic {
            severity: match d["severity"].as_u64().unwrap_or(1) {
                1 => "error",
                2 => "warning",
                3 => "info",
                _ => "hint",
            }
            .to_string(),
            line: d["range"]["start"]["line"].as_u64().unwrap_or(0) + 1,
            column: d["range"]["start"]["character"].as_u64().unwrap_or(0) + 1,
            message: d["message"].as_str().unwrap_or("").to_string(),
        })
        .collect();

    // Nearby definitions: the closest declarations above the position in this file.
    let mut nearby: Vec<PackSymbol> = parse
        .symbols
        .iter()
        .filter(|s| s.line < line)
        .map(|s| PackSymbol {
            name: s.name.clone(),
            kind: s.kind.clone(),
            line: s.line,
        })
        .collect();
    let keep_from = nearby.len().saturating_sub(5);
    nearby.drain(..keep_from);

    // Relevant lemmas: index search on the goal target, when an index exists.
    let mut lemmas = Vec::new();
    let mut constraints = vec![format!(
        "security.workspace_only=true, command_timeout={}s",
        cfg.security.command_timeout_seconds
    )];
    let dirs = ProjectDirs::new(&root);
    if dirs.index_db().exists() {
        if let (Some(t), Ok(idx)) = (&target, tpt_lean_index::Index::open(&dirs.index_db())) {
            let opts = tpt_lean_search::SearchOptions {
                limit: 5,
                ..Default::default()
            };
            let query: String = t
                .chars()
                .map(|c| if c.is_alphanumeric() || c == '_' { c } else { ' ' })
                .collect();
            if let Ok(hits) = tpt_lean_search::search(&idx, query.trim(), &opts) {
                lemmas = hits
                    .into_iter()
                    .map(|h| PackLemma {
                        name: h.name,
                        source: h.file,
                        line: h.line,
                        score: (h.score * 1000.0).round() / 1000.0,
                    })
                    .collect();
            }
        }
    } else {
        constraints.push("no index: run the index scan command to get lemma candidates".into());
    }

    let imports: Vec<String> = parse.imports.iter().map(|i| i.module.clone()).collect();

    // Budget: drop the least important parts until the estimate fits.
    let mut pack = ContextPack {
        file: file.clone(),
        position: PackPosition { line, column },
        snippet,
        goal,
        hypotheses,
        diagnostics,
        imports,
        nearby_definitions: nearby,
        relevant_lemmas: lemmas,
        constraints,
        truncated: false,
        approx_tokens: 0,
    };
    let estimate = |p: &ContextPack| serde_json::to_string(p).map(|s| approx_tokens(&s)).unwrap_or(0);
    let mut truncated = false;
    while estimate(&pack) > max_tokens {
        truncated = true;
        if pack.relevant_lemmas.pop().is_none()
            && (pack.nearby_definitions.is_empty() || {
                pack.nearby_definitions.remove(0);
                false
            })
        {
            // Nothing left to drop except the snippet, which is kept whole.
            break;
        }
    }
    pack.truncated = truncated;
    pack.approx_tokens = estimate(&pack);

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
        assert_eq!(approx_tokens(""), 0);
        assert!(approx_tokens("abcdefgh") < approx_tokens("abcdefghijklmnop"));
    }
}
