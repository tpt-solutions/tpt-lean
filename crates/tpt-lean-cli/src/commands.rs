// SPDX-License-Identifier: MIT OR Apache-2.0
//! Command implementations and dispatch.

mod ext;
mod setup;

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
        ["sorry-report"] => sorry_report(ctx, args),
        ["watch"] => ext::watch_cmd(ctx, args),
        ["setup"] => setup::setup_cmd(ctx, args),
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
        ["cache", "export"] => ext::cache_export(ctx, args),
        ["cache", "import"] => ext::cache_import(ctx, args),
        ["clean"] => clean(ctx, args),
        ["goals"] => goals(ctx, args),
        ["explain"] if args.flag("--last-error") => ext::explain_last(ctx, args),
        ["explain"] => explain(ctx, args),
        ["context-pack"] => context_pack(ctx, args),
        ["imports"] => match ext::imports_analysis(ctx, args) {
            Some(r) => r,
            None => imports(ctx, args),
        },
        ["completions", shell] => completions_cmd(ctx, shell),
        ["completions"] => Err(Error::invalid(
            "usage: tpt-lean completions <bash|zsh|fish|powershell>",
        )),
        ["help"] => help(ctx),
        [cmd, ..] => Err(Error::invalid(format!(
            "unknown command `{cmd}`; run `tpt-lean help`"
        ))),
        [] => help(ctx),
    }
}

/// Print a shell completion script to stdout (human mode) or in the JSON envelope.
fn completions_cmd(ctx: &Ctx, shell: &str) -> Result<i32> {
    let text = crate::completions::script(shell).ok_or_else(|| {
        Error::invalid(format!(
            "unknown shell `{shell}`; use one of: {}",
            crate::completions::SHELLS.join(", ")
        ))
    })?;
    if ctx.mode == Mode::Human {
        print!("{text}");
    } else {
        ctx.emit(
            "completions",
            "",
            &serde_json::json!({ "shell": shell, "script": text }),
        )?;
    }
    Ok(OK)
}

fn help(ctx: &Ctx) -> Result<i32> {
    let text = "\
tpt-lean: tooling around Lean 4 (Lean remains the proof checker)

Project and environment:
  init <dir> [--mathlib] [--template NAME]    create a starter project
  doctor [--fix]                              check the local Lean environment; --fix rebuilds the index
  config show                                 print the effective configuration
  clean                                       run `lake clean`
  cache status | prune | export DIR | import DIR
  completions bash|zsh|fish|powershell        print a shell completion script

Source:
  sorry-report [paths] [--deny-sorry]         list sorry and admit per declaration
  watch [--once] [--interval-ms N]            re-check changed files; a summary per file
  setup [--editor NAME]... [--mcp NAME]... [--dry-run]  write editor and MCP config, diff first
  fmt [--check|--diff|--verify] [paths]       conservative formatter
  imports [file] [--unused|--cost|--minimize FILE|--suggest QUERY]
  search <query> [--from FILE] [--namespace NS] [--near TERM] [--limit N]
  search --type PATTERN | --goal-at FILE:LINE:COL
  index scan | refresh | status | clear | sidecar

Lean (needs `lean` and `lake`):
  build [target] [--watch|--plan|--explain-slow|--report]
  check [file]
  goals FILE:LINE:COL
  explain FILE:LINE:COL | --last-error
  context-pack FILE:LINE:COL [--max-tokens N]
  profile FILE [--imports] [--graph OUT.dot] [--flamegraph OUT.folded] [--compare A B]
  docgen [--out DIR] [--open]
  ci [--json|--sarif]
  bisect --good REV --bad REV

Servers and integrations:
  server start [--port N] [--http PORT] | status | stop
  mcp stdio | http [--port N]
  lsp

Global flags: --json, --ndjson, --project <dir>, --set section.key=value, -v, -q
See docs/cli.md for every command and flag, and docs/json-schemas.md for the JSON output.";
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

/// The Mathlib tag `init --mathlib` pins, and with it the Lean toolchain. Mathlib tags follow Lean
/// releases (`v4.36.0` is Lean 4.36.0). Update this with a tested Lean release.
const MATHLIB_PIN: &str = "v4.36.0";

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
    let template = args.value("--template")?.unwrap_or_else(|| "default".into());
    // `--template mathlib` is the same as `--mathlib` with the default starter file.
    let mathlib = args.flag("--mathlib") || template == "mathlib";
    let main_body = match template.as_str() {
        "default" | "mathlib" => format!(
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
                "unknown template {other}; use default, mathlib, theorem, teaching, or library"
            )))
        }
    };
    let mut lakefile = String::from("import Lake\nopen Lake DSL\n\n");
    if mathlib {
        lakefile
            .push_str(&format!(
                "require mathlib from git \"https://github.com/leanprover-community/mathlib4\" @ \"{MATHLIB_PIN}\"\n\n"
            ));
    }
    lakefile.push_str(&format!(
        "package «{name}» where\n\n@[default_target]\nlean_exe «{name}» where\n  root := `Main\n"
    ));
    let files: Vec<(&str, String)> = vec![
        ("lakefile.lean", lakefile),
        (
            "lean-toolchain",
            if mathlib {
                format!("leanprover/lean4:{MATHLIB_PIN}\n")
            } else {
                "leanprover/lean4:stable\n".into()
            },
        ),
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
    std::fs::create_dir_all(&dir)?;
    let mut written = Vec::new();
    for (file, body) in &files {
        std::fs::write(dir.join(file), body)?;
        written.push((*file).to_string());
    }
    if mathlib {
        ctx.note(&format!(
            "note: Mathlib is pinned to {MATHLIB_PIN}, the Lean release this tool is tested against; update lean-toolchain and the require line together"
        ));
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
    args.reject_unknown(&["--fix"], &[])?;
    let root = project_root(args)?;
    let config = load_config(args, &root).map(|_| ()).map_err(|e| e.to_string());
    let mut checks = tpt_lean_tools::project_doctor::project_checks(&root, config.clone());
    if args.flag("--fix") {
        let plan = doctor_fix_plan(&checks);
        // The plan is printed before anything changes, so the user sees what is about to run.
        if ctx.mode == Mode::Human && !plan.is_empty() {
            for line in &plan {
                println!("plan: {line}");
            }
        }
        let mut fixes = Vec::new();
        if plan.iter().any(|p| p.starts_with("index:")) {
            let failed_integrity = checks.iter().any(|c| c.name == "index" && c.status == "fail");
            let detail = match rebuild_index(args, &root, failed_integrity) {
                Ok(msg) => ("ok", msg),
                Err(e) => ("fail", e.to_string()),
            };
            fixes.push(tpt_lean_tools::project_doctor::ProjectCheck {
                name: "fix",
                status: detail.0,
                detail: format!("index: {}", detail.1),
            });
        }
        // Re-check after the fixes, so the report shows the state the project is in now.
        checks = tpt_lean_tools::project_doctor::project_checks(&root, config);
        checks.extend(fixes);
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

/// The fixes `doctor --fix` would apply to these checks, described for a person. Only fixes that
/// touch derived data are listed; a missing toolchain or a broken configuration is not something
/// the tool can repair, so those are left to the report.
fn doctor_fix_plan(checks: &[tpt_lean_tools::project_doctor::ProjectCheck]) -> Vec<String> {
    let mut plan = Vec::new();
    if let Some(c) = checks.iter().find(|c| c.name == "index") {
        if c.status == "warn" {
            plan.push("index: build the symbol index (it does not exist yet)".to_string());
        } else if c.status == "fail" {
            plan.push("index: rebuild the symbol index (the index is derived data; the damaged database is deleted first)".to_string());
        }
    }
    plan
}

/// Rebuild the symbol index from the sources. When `replace` is set the existing database is
/// removed first; it is a cache, so nothing is lost that the sources do not hold.
fn rebuild_index(args: &Args, root: &Path, replace: bool) -> Result<String> {
    let cfg = load_config(args, root)?;
    let dirs = ProjectDirs::new(root);
    dirs.ensure()?;
    let db = dirs.index_db();
    if replace {
        for suffix in ["", "-wal", "-shm"] {
            let p = PathBuf::from(format!("{}{suffix}", db.display()));
            match std::fs::remove_file(&p) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::from(e).with_path(&p)),
            }
        }
    }
    let mut index = Index::open(&db)?;
    let roots = default_scan_roots(root, cfg.index.mathlib);
    let stats = index.refresh(&roots)?;
    Ok(format!(
        "rebuilt from {} files ({} updated, {} removed)",
        stats.scanned, stats.updated, stats.removed
    ))
}

/// `sorry-report [paths] [--deny-sorry]`: every `sorry` and `admit`, per declaration. With
/// `--deny-sorry` any hit makes the exit status 1, for use in CI.
fn sorry_report(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(&["--deny-sorry", "--project"], &["--project"])?;
    let root = project_root(args)?;
    let paths = args.positionals(&[]);
    let files: Vec<PathBuf> = if paths.is_empty() {
        tpt_lean_index::collect_lean_files(&root)?
            .into_iter()
            .filter(|(rel, _)| rel != "lakefile.lean")
            .map(|(_, p)| p)
            .collect()
    } else {
        paths.iter().map(PathBuf::from).collect()
    };
    let hits = tpt_lean_tools::sorry::sorry_hits(&root, &files)?;
    let human: Vec<String> = hits
        .iter()
        .map(|h| {
            format!(
                "{}:{}:{}: {} in {}",
                h.file,
                h.line,
                h.column,
                h.keyword,
                h.declaration.as_deref().unwrap_or("(no declaration)")
            )
        })
        .chain(std::iter::once(format!(
            "{} sorry/admit in {} file(s) scanned",
            hits.len(),
            files.len()
        )))
        .collect();
    ctx.emit_items("sorry-report", &human, &hits)?;
    if args.flag("--deny-sorry") && !hits.is_empty() {
        return Ok(CHECK_FAILED);
    }
    Ok(OK)
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

#[derive(Serialize)]
struct TypeSearchItem {
    name: String,
    kind: String,
    package: String,
    file: String,
    type_text: String,
    score: f64,
}

fn search(ctx: &Ctx, args: &Args) -> Result<i32> {
    const VALUED: &[&str] = &[
        "--limit",
        "--namespace",
        "--near",
        "--project",
        "--goal-at",
        "--type",
        "--from",
    ];
    args.reject_unknown(VALUED, VALUED)?;
    let positional = args.positionals(VALUED);
    let root = project_root(args)?;
    let type_pattern = args.value("--type")?;
    let goal_at = args.value("--goal-at")?;
    if type_pattern.is_some() && goal_at.is_some() {
        return Err(Error::invalid("give either --type or --goal-at, not both"));
    }
    let query = match (&type_pattern, goal_at) {
        (Some(pattern), _) => {
            if !positional.is_empty() {
                return Err(Error::invalid("give either a query or --type, not both"));
            }
            pattern.clone()
        }
        (None, Some(spec)) => {
            if !positional.is_empty() {
                return Err(Error::invalid("give either a query or --goal-at, not both"));
            }
            goal_query_at(args, &root, &spec)?
        }
        (None, None) => {
            if positional.is_empty() {
                return Err(Error::invalid(
                    "usage: tpt-lean search <query> | --type PATTERN | --goal-at FILE:LINE:COL [--from FILE] [--namespace NS] [--near TERM]... [--limit N]",
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
    if type_pattern.is_some() {
        let opts = SearchOptions {
            limit,
            namespace: args.value("--namespace")?,
            ..Default::default()
        };
        let hits = tpt_lean_search::search_by_type(&index, &query, &opts)?;
        let items: Vec<TypeSearchItem> = hits
            .into_iter()
            .map(|h| TypeSearchItem {
                name: h.name,
                kind: h.kind,
                package: h.package,
                file: h.file,
                type_text: h.type_text,
                score: (h.score * 1000.0).round() / 1000.0,
            })
            .collect();
        let human: Vec<String> = if items.is_empty() {
            vec![format!(
                "no declarations with type matching {query:?} (types come from `tpt-lean index sidecar`)"
            )]
        } else {
            items
                .iter()
                .map(|i| format!("{:.3}  {:<9} {}  : {}", i.score, i.kind, i.name, i.type_text))
                .collect()
        };
        ctx.emit_items("search", &human, &items)?;
        return Ok(OK);
    }
    // `--from FILE`: symbols from modules the file imports rank higher.
    let imported_modules = match args.value("--from")? {
        Some(file) => {
            let ws = WorkspaceRoot::new(&root)?;
            let path = ws.resolve(&file)?;
            let text = std::fs::read_to_string(&path).map_err(|e| Error::from(e).with_path(&path))?;
            tpt_lean_syntax::parse(&text)
                .imports
                .into_iter()
                .map(|i| i.module)
                .collect()
        }
        None => Vec::new(),
    };
    let opts = SearchOptions {
        limit,
        namespace: args.value("--namespace")?,
        near: args.values("--near"),
        imported_modules,
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
    // The unlocked handles are Send, so the server can read on one thread and answer on another.
    tpt_lean_mcp::serve_stdio(
        &server,
        std::io::BufReader::new(std::io::stdin()),
        std::io::stdout(),
    )?;
    Ok(OK)
}

/// `tpt-lean server start [--port N]`: serve the JSON-RPC API in the foreground and record it in
/// `.tpt-lean/server.json` so `server status` and `server stop` can find it. On Linux and macOS
/// the default is a Unix socket at `.tpt-lean/server.sock`; `--port N` selects loopback TCP, and
/// Windows always uses loopback TCP (default port 7373).
fn server_start(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &["--project", "--port", "--http"],
        &["--project", "--port", "--http"],
    )?;
    let root = project_root(args)?;
    let http = match args.value("--http")? {
        Some(p) => Some(
            p.parse::<u16>()
                .map_err(|_| Error::invalid("--http must be 0-65535"))?,
        ),
        None => None,
    };
    let transport = match args.value("--port")? {
        Some(p) => {
            tpt_lean_server::Transport::Tcp(p.parse().map_err(|_| Error::invalid("--port must be 0-65535"))?)
        }
        None => default_transport(),
    };
    let described = match transport {
        tpt_lean_server::Transport::Tcp(port) => format!("127.0.0.1:{port}"),
        #[cfg(unix)]
        tpt_lean_server::Transport::Unix => "a Unix socket in .tpt-lean".to_string(),
        #[cfg(windows)]
        tpt_lean_server::Transport::Pipe => "a named pipe".to_string(),
    };
    let http_note = match http {
        Some(port) => format!(" and HTTP on 127.0.0.1:{port} (POST /v1/rpc)"),
        None => String::new(),
    };
    ctx.note(&format!(
        "serving JSON-RPC on {described}{http_note} (Ctrl+C to stop)"
    ));
    tpt_lean_server::run_foreground_with_http(&root, transport, http)?;
    Ok(OK)
}

#[cfg(unix)]
fn default_transport() -> tpt_lean_server::Transport {
    tpt_lean_server::Transport::Unix
}

#[cfg(windows)]
fn default_transport() -> tpt_lean_server::Transport {
    tpt_lean_server::Transport::Pipe
}

#[cfg(not(any(unix, windows)))]
fn default_transport() -> tpt_lean_server::Transport {
    tpt_lean_server::Transport::Tcp(7373)
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
/// `goals FILE:LINE:COL --sidecar [--exe PATH]`: the goals from the Lean sidecar, which elaborates the
/// file with Lean's own frontend. No language server is started. The sidecar must be built for the
/// Lean version in use (see `lean/tpt-lean-sidecar`).
fn goals_with_sidecar(
    ctx: &Ctx,
    args: &Args,
    root: &Path,
    cfg: &Config,
    file: &str,
    line: usize,
    column: usize,
) -> Result<i32> {
    let exe = match args.value("--exe")? {
        Some(path) => PathBuf::from(path),
        None => std::env::var_os("TPT_LEAN_SIDECAR")
            .map(PathBuf::from)
            .ok_or_else(|| {
                Error::invalid(
                    "sidecar not found: pass --exe or set TPT_LEAN_SIDECAR to the built tpt_sidecar binary (build lean/tpt-lean-sidecar with `lake -R build`)",
                )
            })?,
    };
    let timeout = match args.value("--timeout")? {
        Some(t) => t
            .parse::<u64>()
            .map_err(|_| Error::invalid("--timeout must be seconds"))?,
        None => cfg.security.command_timeout_seconds,
    };
    let tc = adapter::discover_toolchain();
    if tc.lean.is_none() || tc.lake.is_none() {
        return Err(Error::new(
            ErrorCode::LeanNotFound,
            "lean and lake are required (install with elan)",
        ));
    }
    let policy = adapter::lean_policy(timeout);
    let cancel = tpt_lean_core::policy::CancelToken::new();
    let sysroot = adapter::lean_sysroot(&tc, &policy, &cancel)?;
    let lean_path = adapter::sidecar::lean_path_for(&tc, root, &policy, &cancel);
    let site = adapter::sidecar::GoalSite {
        file: Path::new(file),
        line: line as u64,
        column: column as u64,
    };
    let goals = adapter::sidecar::run_sidecar_goals(
        &exe,
        &sysroot,
        &site,
        Some(root),
        lean_path.as_deref(),
        &policy,
        &cancel,
    )?;
    let human = if goals.is_empty() {
        format!("no goals at {file}:{line}:{column}")
    } else {
        goals.join(
            "

",
        )
    };
    ctx.emit(
        "goals",
        &human,
        &GoalsReport {
            file: file.to_string(),
            line,
            column,
            goals,
        },
    )?;
    Ok(OK)
}

fn goals(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &["--project", "--timeout", "--sidecar", "--exe"],
        &["--project", "--timeout", "--exe"],
    )?;
    let spec = args
        .positionals(&["--project", "--timeout", "--exe"])
        .into_iter()
        .next()
        .ok_or_else(|| Error::invalid("usage: tpt-lean goals FILE:LINE:COL"))?;
    let (file, line, column) = parse_position(&spec)?;
    let root = project_root(args)?;
    let cfg = load_config(args, &root)?;
    let ws = WorkspaceRoot::new(&root)?;
    let path = ws.resolve(&file)?;
    if args.flag("--sidecar") {
        return goals_with_sidecar(ctx, args, &root, &cfg, &file, line, column);
    }
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
