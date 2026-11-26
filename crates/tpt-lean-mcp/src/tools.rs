// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tool catalogue (names, permission classes, JSON schemas) and the tool implementations.

use crate::lean::{self, PackInput};
use crate::permission::Permission;
use crate::Server;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tpt_lean_adapter::{self as adapter, Command as LeanCommand};
use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_core::policy::CancelToken;
use tpt_lean_index::Index;
use tpt_lean_search::SearchOptions;

/// Maximum characters of process output returned by `tpt_lean_build_project`.
const OUTPUT_TAIL_CHARS: usize = 4000;

/// One tool: its metadata and the JSON schemas it advertises.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub class: Permission,
    pub input: Value,
    pub output: Value,
}

/// All tools, in the order `tools/list` reports them.
pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "tpt_lean_search",
            description: "Search the local Lean index by name, prefix, or docstring text.",
            class: Permission::Read,
            input: object(
                json!({
                    "query": string("Name, prefix, or words from a docstring."),
                    "limit": integer("Maximum hits (default 10).", 1, Some(200)),
                    "namespace": string("Only symbols in this namespace."),
                }),
                &["query"],
            ),
            output: object(
                json!({ "results": array(hit_schema()) }),
                &["results"],
            ),
        },
        ToolSpec {
            name: "tpt_lean_format_text",
            description: "Format Lean source text in memory. Never writes files.",
            class: Permission::Read,
            input: object(json!({ "text": string("Lean source text.") }), &["text"]),
            output: object(
                json!({
                    "formatted": string("Formatted text."),
                    "changed": boolean("Whether the formatter changed the text."),
                }),
                &["formatted", "changed"],
            ),
        },
        ToolSpec {
            name: "tpt_lean_index_status",
            description: "Report index schema version, sizes, packages, and integrity.",
            class: Permission::Read,
            input: object(json!({}), &[]),
            output: object(
                json!({
                    "schema_version": integer_out(),
                    "files": integer_out(),
                    "symbols": integer_out(),
                    "imports": integer_out(),
                    "packages": array(json!({"type": "string"})),
                    "integrity_ok": boolean("Whether the index passed its integrity check."),
                }),
                &["schema_version", "files", "symbols", "imports", "packages", "integrity_ok"],
            ),
        },
        ToolSpec {
            name: "tpt_lean_check_file",
            description: "Run `lean` on a .lean file inside the workspace and return diagnostics. Requires build permission.",
            class: Permission::Build,
            input: object(
                json!({
                    "file": file_prop(),
                    "timeout_seconds": integer("Timeout; capped at security.command_timeout_seconds.", 1, None),
                }),
                &["file"],
            ),
            output: object(
                json!({
                    "file": string("Workspace-relative path."),
                    "success": boolean("Lean exited 0 with no error diagnostics."),
                    "timed_out": boolean("The run hit its timeout."),
                    "cancelled": boolean("The run was cancelled by the client."),
                    "diagnostics": array(diagnostic_schema()),
                }),
                &["file", "success", "timed_out", "diagnostics"],
            ),
        },
        ToolSpec {
            name: "tpt_lean_format_file",
            description: "Format a .lean file in the workspace and write it back when it changes. Requires write permission (security.allow_write).",
            class: Permission::Write,
            input: object(
                json!({
                    "file": file_prop(),
                    "dry_run": boolean("Report what would change without writing (default false)."),
                }),
                &["file"],
            ),
            output: object(
                json!({
                    "file": string("Workspace-relative path."),
                    "changed": boolean("The formatted text differs from the file."),
                    "written": boolean("The file was written."),
                    "dry_run": boolean("No write was attempted."),
                }),
                &["file", "changed", "written", "dry_run"],
            ),
        },
        ToolSpec {
            name: "tpt_lean_goals_at",
            description: "Plain goals at a 1-based file position, from the Lean language server. Requires build permission because it runs Lean.",
            class: Permission::Build,
            input: object(position_props(), &["file", "line", "column"]),
            output: object(
                json!({
                    "file": string("Workspace-relative path."),
                    "line": integer_out(),
                    "column": integer_out(),
                    "goals": array(json!({"type": "string"})),
                }),
                &["file", "line", "column", "goals"],
            ),
        },
        ToolSpec {
            name: "tpt_lean_explain_diagnostic",
            description: "Diagnostics at a 1-based position plus the goal there. Diagnostics whose range covers the position are returned; if none do, diagnostics starting on that line are. Requires build permission.",
            class: Permission::Build,
            input: object(position_props(), &["file", "line", "column"]),
            output: object(
                json!({
                    "file": string("Workspace-relative path."),
                    "position": position_out(),
                    "goal": { "type": ["string", "null"], "description": "First goal at the position." },
                    "hypotheses": array(json!({"type": "string"})),
                    "target": { "type": ["string", "null"] },
                    "diagnostics": array(diagnostic_schema()),
                    "other_diagnostics_in_file": integer_out(),
                }),
                &["file", "position", "goal", "hypotheses", "target", "diagnostics", "other_diagnostics_in_file"],
            ),
        },
        ToolSpec {
            name: "tpt_lean_context_pack",
            description: "Compact bundle for an AI agent at a position: goal, hypotheses, diagnostics, candidate symbols, and a source excerpt, trimmed to max_tokens. Requires build permission because it runs Lean.",
            class: Permission::Build,
            input: object(
                {
                    let mut p = position_props();
                    p["max_tokens"] = integer("Token budget estimate (default 6000).", 200, Some(200_000));
                    p
                },
                &["file", "line", "column"],
            ),
            output: object(
                json!({
                    "file": string("Workspace-relative path."),
                    "position": position_out(),
                    "goal": { "type": ["string", "null"] },
                    "hypotheses": array(json!({"type": "string"})),
                    "diagnostics": array(diagnostic_schema()),
                    "relevant_symbols": array(symbol_schema()),
                    "source_excerpt": string("Numbered source lines around the position."),
                    "max_tokens": integer_out(),
                    "estimated_tokens": integer_out(),
                    "truncated": boolean("Parts were dropped to fit max_tokens."),
                }),
                &["file", "position", "goal", "hypotheses", "diagnostics", "relevant_symbols", "source_excerpt", "truncated"],
            ),
        },
        ToolSpec {
            name: "tpt_lean_build_project",
            description: "Run `lake build` (optionally one target) in the project root with the configured timeout. Requires build permission.",
            class: Permission::Build,
            input: object(
                json!({
                    "target": string("Lake target name, for example Main."),
                    "timeout_seconds": integer("Timeout; capped at security.command_timeout_seconds.", 1, None),
                }),
                &[],
            ),
            output: object(
                json!({
                    "target": { "type": ["string", "null"] },
                    "success": boolean("Lake exited 0."),
                    "timed_out": boolean("The build hit its timeout."),
                    "cancelled": boolean("The build was cancelled by the client."),
                    "exit_code": { "type": ["integer", "null"] },
                    "duration_ms": integer_out(),
                    "diagnostics": array(diagnostic_schema()),
                    "output_tail": string("Last characters of build output."),
                }),
                &["success", "timed_out", "diagnostics", "output_tail"],
            ),
        },
        ToolSpec {
            name: "tpt_lean_profile_file",
            description: "Profile a .lean file: wall time, per-declaration timings from Lean's trace, tactic totals and suggestions. Runs Lean. Requires security.allow_build and security.allow_profile.",
            class: Permission::Profile,
            input: object(json!({ "file": file_prop() }), &["file"]),
            output: object(json!({}), &[]),
        },
        ToolSpec {
            name: "tpt_lean_imports_suggest",
            description: "Suggest modules to import for a query: modules whose files declare matching symbols in the index.",
            class: Permission::Read,
            input: object(
                json!({
                    "query": string("Symbol name or words to look for."),
                    "limit": integer("Maximum suggestions (default 10).", 1, Some(50)),
                    "file": file_prop_optional("Optional .lean file; modules it already imports are left out."),
                }),
                &["query"],
            ),
            output: object(
                json!({
                    "query": string("The query as given."),
                    "suggestions": array(json!({
                        "type": "object",
                        "properties": {
                            "module": string("Module name to import."),
                            "statement": string("The import line."),
                            "package": string("Index package the file belongs to."),
                            "file": string("Source file that declares the symbols."),
                            "matching_symbols": array(json!({"type": "string"})),
                        },
                        "required": ["module", "statement", "package", "file", "matching_symbols"],
                    })),
                }),
                &["query", "suggestions"],
            ),
        },
    ]
}

/// Tool descriptors for `tools/list`: name, description, inputSchema, outputSchema.
pub fn tool_descriptors() -> Value {
    Value::Array(
        specs()
            .into_iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "description": s.description,
                    "inputSchema": s.input,
                    "outputSchema": s.output,
                })
            })
            .collect(),
    )
}

/// The spec for a tool name, if it exists.
pub fn find(name: &str) -> Option<ToolSpec> {
    specs().into_iter().find(|s| s.name == name)
}

// ---- schema helpers -------------------------------------------------------------------

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn string(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn boolean(description: &str) -> Value {
    json!({ "type": "boolean", "description": description })
}

fn integer(description: &str, min: i64, max: Option<i64>) -> Value {
    let mut v = json!({ "type": "integer", "minimum": min, "description": description });
    if let Some(max) = max {
        v["maximum"] = json!(max);
    }
    v
}

fn integer_out() -> Value {
    json!({ "type": "integer" })
}

fn array(items: Value) -> Value {
    json!({ "type": "array", "items": items })
}

fn file_prop() -> Value {
    string("Path to a .lean file: relative to the project root, or absolute inside it.")
}

fn file_prop_optional(description: &str) -> Value {
    string(description)
}

fn position_props() -> Value {
    json!({
        "file": file_prop(),
        "line": integer("1-based line.", 1, None),
        "column": integer("1-based column, counted in characters.", 1, None),
        "timeout_seconds": integer("Timeout; capped at security.command_timeout_seconds.", 1, None),
    })
}

fn position_out() -> Value {
    json!({
        "type": "object",
        "properties": { "line": integer_out(), "column": integer_out() },
        "required": ["line", "column"],
    })
}

fn diagnostic_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "file": string("File as reported."),
            "line": integer_out(),
            "column": integer_out(),
            "severity": { "type": "string", "enum": ["error", "warning", "info", "hint"] },
            "message": string("Diagnostic text."),
        },
        "required": ["file", "line", "column", "severity", "message"],
    })
}

fn hit_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": string("Declaration name."),
            "kind": string("Declaration kind."),
            "package": string("Index package."),
            "source": string("Source file, relative to the package root."),
            "line": integer_out(),
            "score": { "type": "number" },
            "docstring": { "type": ["string", "null"] },
        },
        "required": ["name", "kind", "package", "source", "line", "score"],
    })
}

fn symbol_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": string("Declaration name."),
            "kind": string("Declaration kind."),
            "source": string("Source file."),
            "line": integer_out(),
        },
        "required": ["name"],
    })
}

// ---- argument helpers -----------------------------------------------------------------

pub(crate) fn arg_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing string argument `{key}`"))
}

/// A required 1-based integer (line or column).
fn arg_position(args: &Value, key: &str) -> Result<u32, String> {
    let v = args
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing integer argument `{key}`"))?;
    if v == 0 || v > u32::MAX as u64 {
        return Err(format!("`{key}` must be a 1-based integer"));
    }
    Ok(v as u32)
}

fn tail_chars(text: &str, n: usize) -> String {
    let count = text.chars().count();
    if count <= n {
        text.to_string()
    } else {
        text.chars().skip(count - n).collect()
    }
}

/// Module name of an index file: its path relative to the package root, without `.lean`,
/// with `/` replaced by `.`. The lakefile's source roots are not read.
fn module_of(rel: &str) -> String {
    rel.strip_suffix(".lean").unwrap_or(rel).replace('/', ".")
}

/// Modules a file imports, from its syntax tree.
fn imported_modules(text: &str) -> BTreeSet<String> {
    tpt_lean_syntax::parse(text)
        .imports
        .into_iter()
        .map(|i| i.module)
        .collect()
}

// ---- tool implementations ---------------------------------------------------------------

impl Server {
    /// Dispatch a tool that has already passed its permission check.
    pub(crate) fn run_tool(&self, name: &str, args: &Value, cancel: &CancelToken) -> Result<Value, String> {
        match name {
            "tpt_lean_search" => self.tool_search(args),
            "tpt_lean_format_text" => self.tool_format_text(args),
            "tpt_lean_index_status" => self.index_status(),
            "tpt_lean_check_file" => self.tool_check_file(args, cancel),
            "tpt_lean_format_file" => self.tool_format_file(args),
            "tpt_lean_goals_at" => self.tool_goals_at(args),
            "tpt_lean_explain_diagnostic" => self.tool_explain_diagnostic(args),
            "tpt_lean_context_pack" => self.tool_context_pack(args),
            "tpt_lean_build_project" => self.tool_build_project(args, cancel),
            "tpt_lean_profile_file" => self.tool_profile_file(args),
            "tpt_lean_imports_suggest" => self.tool_imports_suggest(args),
            other => Err(format!("unknown tool {other}")),
        }
    }

    fn index(&self) -> Result<Index, String> {
        let dirs = ProjectDirs::new(self.root());
        if !dirs.index_db().exists() {
            return Err("no index yet; run `tpt-lean index scan` first".into());
        }
        Index::open(&dirs.index_db()).map_err(|e| e.to_string())
    }

    /// Workspace-relative display path with forward slashes.
    pub(crate) fn relative(&self, path: &Path) -> String {
        path.strip_prefix(self.root())
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Resolve a `.lean` file inside the workspace and read it.
    /// Returns the absolute path, the workspace-relative path, and the text.
    pub(crate) fn lean_file(&self, file: &str) -> Result<(PathBuf, String, String), String> {
        let path = self.workspace().resolve(file).map_err(|e| e.to_string())?;
        if path.extension().and_then(|e| e.to_str()) != Some("lean") {
            return Err(format!("`{file}` is not a .lean file"));
        }
        let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read `{file}`: {e}"))?;
        let rel = self.relative(&path);
        Ok((path, rel, text))
    }

    fn lean_file_arg(&self, args: &Value) -> Result<(PathBuf, String, String), String> {
        self.lean_file(&arg_str(args, "file")?)
    }

    /// Requested timeout, never above the configured limit.
    fn timeout_seconds(&self, args: &Value) -> u64 {
        let configured = self.config().security.command_timeout_seconds.max(1);
        args.get("timeout_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(configured)
            .clamp(1, configured)
    }

    /// Keep the diagnostics cache in step with a run: diagnostics are grouped by their file.
    fn remember_run_diagnostics(&self, diagnostics: &[Value]) {
        let mut by_file: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for d in diagnostics {
            let file = d["file"].as_str().unwrap_or("").to_string();
            let key = self.relative(Path::new(&file));
            by_file.entry(key).or_default().push(d.clone());
        }
        for (file, ds) in by_file {
            self.store_diagnostics(&file, ds);
        }
    }

    pub(crate) fn index_status(&self) -> Result<Value, String> {
        let status = self.index()?.status().map_err(|e| e.to_string())?;
        Ok(json!({
            "schema_version": status.schema_version,
            "files": status.files,
            "symbols": status.symbols,
            "imports": status.imports,
            "packages": status.packages,
            "integrity_ok": status.integrity_ok,
        }))
    }

    fn tool_search(&self, args: &Value) -> Result<Value, String> {
        let query = arg_str(args, "query")?;
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .clamp(1, 200) as usize;
        let opts = SearchOptions {
            limit,
            namespace: args.get("namespace").and_then(Value::as_str).map(str::to_string),
            ..Default::default()
        };
        let index = self.index()?;
        let hits = tpt_lean_search::search(&index, &query, &opts).map_err(|e| e.to_string())?;
        let results: Vec<Value> = hits
            .into_iter()
            .map(|h| {
                json!({
                    "name": h.name,
                    "kind": h.kind,
                    "package": h.package,
                    "source": h.file,
                    "line": h.line,
                    "score": (h.score * 1000.0).round() / 1000.0,
                    "docstring": h.docstring,
                })
            })
            .collect();
        Ok(json!({ "results": results }))
    }

    fn tool_format_text(&self, args: &Value) -> Result<Value, String> {
        let text = arg_str(args, "text")?;
        match tpt_lean_fmt::format_text(&text) {
            Ok(out) => Ok(json!({ "formatted": out.output, "changed": out.changed })),
            Err(e) => Err(e.to_string()),
        }
    }

    fn tool_format_file(&self, args: &Value) -> Result<Value, String> {
        let (path, rel, text) = self.lean_file_arg(args)?;
        let dry_run = args.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
        let formatted = tpt_lean_fmt::format_text(&text).map_err(|e| e.to_string())?;
        let mut written = false;
        if formatted.changed && !dry_run {
            std::fs::write(&path, &formatted.output).map_err(|e| format!("cannot write `{rel}`: {e}"))?;
            written = true;
        }
        Ok(json!({
            "file": rel,
            "changed": formatted.changed,
            "written": written,
            "dry_run": dry_run,
        }))
    }

    fn tool_check_file(&self, args: &Value, cancel: &CancelToken) -> Result<Value, String> {
        let (path, rel, _text) = self.lean_file_arg(args)?;
        let timeout = self.timeout_seconds(args);
        let tc = adapter::discover_toolchain();
        let policy = adapter::lean_policy(timeout);
        let result = adapter::execute(
            &tc,
            &LeanCommand::LeanCheck { file: path },
            Some(self.root()),
            &policy,
            cancel,
        )
        .map_err(|e| e.to_string())?;
        let diagnostics = serde_json::to_value(&result.diagnostics).map_err(|e| e.to_string())?;
        if let Some(list) = diagnostics.as_array() {
            self.store_diagnostics(&rel, list.clone());
        }
        Ok(json!({
            "file": rel,
            "success": result.output.success(),
            "timed_out": result.output.timed_out,
            "cancelled": result.output.cancelled,
            "diagnostics": diagnostics,
        }))
    }

    fn tool_build_project(&self, args: &Value, cancel: &CancelToken) -> Result<Value, String> {
        let target = match args.get("target") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let t = v.as_str().ok_or("`target` must be a string")?;
                adapter::validate_target(t).map_err(|e| e.to_string())?;
                Some(t.to_string())
            }
        };
        if !self.root().join("lakefile.lean").is_file() && !self.root().join("lakefile.toml").is_file() {
            return Err("no lakefile.lean or lakefile.toml in the project root; nothing to build".into());
        }
        let timeout = self.timeout_seconds(args);
        let tc = adapter::discover_toolchain();
        let policy = adapter::lean_policy(timeout);
        let command = LeanCommand::LakeBuild {
            target: target.clone(),
        };
        let result =
            adapter::execute(&tc, &command, Some(self.root()), &policy, cancel).map_err(|e| e.to_string())?;
        let diagnostics = serde_json::to_value(&result.diagnostics).map_err(|e| e.to_string())?;
        if let Some(list) = diagnostics.as_array() {
            self.remember_run_diagnostics(list);
        }
        let output = &result.output;
        let combined = format!("{}{}", output.stdout, output.stderr);
        let report = json!({
            "target": target,
            "success": output.success(),
            "timed_out": output.timed_out,
            "cancelled": output.cancelled,
            "exit_code": output.exit_code,
            "duration_ms": output.duration.as_millis() as u64,
            "diagnostics": diagnostics,
            "output_tail": tail_chars(&combined, OUTPUT_TAIL_CHARS),
        });
        // Kept for the build-log resource (`tpt-lean://build/log`).
        self.remember_build(report.clone());
        Ok(report)
    }

    pub(crate) fn tool_goals_at(&self, args: &Value) -> Result<Value, String> {
        let (path, rel, text) = self.lean_file_arg(args)?;
        let line = arg_position(args, "line")?;
        let column = arg_position(args, "column")?;
        let snap = lean::elaborate(
            self.root(),
            &path,
            &text,
            line,
            column,
            self.timeout_seconds(args),
        )?;
        Ok(json!({ "file": rel, "line": line, "column": column, "goals": snap.goals }))
    }

    /// Diagnostics covering the position (or starting on its line when none cover it).
    fn tool_explain_diagnostic(&self, args: &Value) -> Result<Value, String> {
        let (path, rel, text) = self.lean_file_arg(args)?;
        let line = arg_position(args, "line")?;
        let column = arg_position(args, "column")?;
        let character = lean::utf16_column(&text, line, column)? as u64;
        let snap = lean::elaborate(
            self.root(),
            &path,
            &text,
            line,
            column,
            self.timeout_seconds(args),
        )?;

        let line0 = line as u64 - 1;
        let all: Vec<Value> = snap
            .diagnostics
            .iter()
            .map(|d| lean::lsp_diagnostic(&rel, d))
            .collect();
        let covering: Vec<Value> = snap
            .diagnostics
            .iter()
            .filter(|d| lean::diagnostic_covers(d, line0, character))
            .map(|d| lean::lsp_diagnostic(&rel, d))
            .collect();
        let here = if covering.is_empty() {
            snap.diagnostics
                .iter()
                .filter(|d| d["range"]["start"]["line"].as_u64() == Some(line0))
                .map(|d| lean::lsp_diagnostic(&rel, d))
                .collect()
        } else {
            covering
        };
        self.store_diagnostics(&rel, all.clone());

        let goal = snap.goals.first().cloned();
        let (hypotheses, target) = match &goal {
            Some(g) => lean::split_goal(g),
            None => (Vec::new(), None),
        };
        let other = all.len().saturating_sub(here.len());
        Ok(json!({
            "file": rel,
            "position": { "line": line, "column": column },
            "goal": goal,
            "hypotheses": hypotheses,
            "target": target,
            "diagnostics": here,
            "other_diagnostics_in_file": other,
        }))
    }

    fn tool_context_pack(&self, args: &Value) -> Result<Value, String> {
        let (path, rel, text) = self.lean_file_arg(args)?;
        let line = arg_position(args, "line")?;
        let column = arg_position(args, "column")?;
        let max_tokens = args
            .get("max_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(6000)
            .clamp(200, 200_000) as usize;
        lean::utf16_column(&text, line, column)?;
        let snap = lean::elaborate(
            self.root(),
            &path,
            &text,
            line,
            column,
            self.timeout_seconds(args),
        )?;

        let goal = snap.goals.first().cloned();
        let (hypotheses, target) = match &goal {
            Some(g) => lean::split_goal(g),
            None => (Vec::new(), None),
        };
        let diagnostics: Vec<Value> = snap
            .diagnostics
            .iter()
            .map(|d| lean::lsp_diagnostic(&rel, d))
            .collect();
        self.store_diagnostics(&rel, diagnostics.clone());

        let symbols = self.context_symbols(&rel, &text, line, target.as_deref());
        let pack = lean::assemble_context(PackInput {
            file: &rel,
            line,
            column,
            text: &text,
            goal,
            hypotheses,
            diagnostics,
            symbols,
            max_tokens,
        });
        Ok(pack)
    }

    /// Candidate symbols for a context pack: index hits for the goal's target, then the
    /// declarations above the position in the same file.
    fn context_symbols(&self, rel: &str, text: &str, line: u32, target: Option<&str>) -> Vec<Value> {
        let mut symbols = Vec::new();
        if let (Some(target), Ok(index)) = (target, self.index()) {
            let query: String = target
                .chars()
                .map(|c| if c.is_alphanumeric() || c == '_' { c } else { ' ' })
                .collect();
            let opts = SearchOptions {
                limit: 5,
                ..Default::default()
            };
            if let Ok(hits) = tpt_lean_search::search(&index, query.trim(), &opts) {
                for h in hits {
                    symbols.push(json!({
                        "name": h.name,
                        "kind": h.kind,
                        "source": h.file,
                        "line": h.line,
                        "score": (h.score * 1000.0).round() / 1000.0,
                    }));
                }
            }
        }
        let parse = tpt_lean_syntax::parse(text);
        let mut local: Vec<Value> = parse
            .symbols
            .iter()
            .filter(|s| s.line < line as usize)
            .map(|s| json!({ "name": s.name, "kind": s.kind, "source": rel, "line": s.line }))
            .collect();
        let keep_from = local.len().saturating_sub(5);
        local.drain(..keep_from);
        symbols.extend(local);
        symbols
    }

    fn tool_profile_file(&self, args: &Value) -> Result<Value, String> {
        let (path, _rel, _text) = self.lean_file_arg(args)?;
        let timeout = self.timeout_seconds(args);
        let report =
            tpt_lean_profile::profile_file(self.root(), &path, timeout).map_err(|e| e.to_string())?;
        serde_json::to_value(&report).map_err(|e| e.to_string())
    }

    fn tool_imports_suggest(&self, args: &Value) -> Result<Value, String> {
        let query = arg_str(args, "query")?;
        let query = query.trim().to_string();
        if query.is_empty() {
            return Err("`query` must not be empty".into());
        }
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .clamp(1, 50) as usize;
        let already: BTreeSet<String> = match args.get("file").and_then(Value::as_str) {
            Some(file) => {
                let (_, _, text) = self.lean_file(file)?;
                imported_modules(&text)
            }
            None => BTreeSet::new(),
        };
        let index = self.index()?;
        let rows = match tpt_lean_index::fts_query(&query) {
            Some(fts) => index.symbols_fts(&fts, 500),
            None => index.symbols_named(&query, 500),
        }
        .map_err(|e| e.to_string())?;

        // Group matching public symbols by the module their file defines.
        struct Entry {
            package: String,
            file: String,
            symbols: BTreeSet<String>,
        }
        let mut modules: BTreeMap<String, Entry> = BTreeMap::new();
        for row in rows.iter().filter(|r| !r.is_private && r.file.ends_with(".lean")) {
            let module = module_of(&row.file);
            if already.contains(&module) {
                continue;
            }
            let entry = modules.entry(module).or_insert_with(|| Entry {
                package: row.package.clone(),
                file: row.file.clone(),
                symbols: BTreeSet::new(),
            });
            entry.symbols.insert(row.name.clone());
        }
        let mut ranked: Vec<(String, Entry)> = modules.into_iter().collect();
        ranked.sort_by(|a, b| {
            b.1.symbols
                .len()
                .cmp(&a.1.symbols.len())
                .then_with(|| a.0.cmp(&b.0))
        });
        ranked.truncate(limit);
        let suggestions: Vec<Value> = ranked
            .into_iter()
            .map(|(module, e)| {
                json!({
                    "module": module,
                    "statement": format!("import {module}"),
                    "package": e.package,
                    "file": e.file,
                    "matching_symbols": e.symbols.into_iter().collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(json!({ "query": query, "suggestions": suggestions }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_schema_is_well_formed() {
        let mut names = BTreeSet::new();
        for spec in specs() {
            assert!(names.insert(spec.name), "duplicate tool {}", spec.name);
            assert!(spec.name.starts_with("tpt_lean_"), "{}", spec.name);
            assert!(!spec.description.is_empty(), "{}", spec.name);
            for (label, schema) in [("input", &spec.input), ("output", &spec.output)] {
                // Round-trip through text to prove the schema is valid JSON.
                let text = schema.to_string();
                let back: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(&back, schema, "{} {label}", spec.name);
                assert_eq!(schema["type"], "object", "{} {label}", spec.name);
                let props = schema["properties"].as_object().expect("properties object");
                for req in schema["required"].as_array().expect("required array") {
                    let key = req.as_str().unwrap();
                    assert!(
                        props.contains_key(key),
                        "{} {label}: required `{key}` is not a property",
                        spec.name
                    );
                }
            }
        }
    }

    #[test]
    fn descriptors_carry_both_schemas() {
        let list = tool_descriptors();
        let list = list.as_array().unwrap();
        assert_eq!(list.len(), 11);
        for t in list {
            assert_eq!(t["inputSchema"]["type"], "object");
            assert_eq!(t["outputSchema"]["type"], "object");
        }
    }

    #[test]
    fn permission_classes_match_the_security_model() {
        let class = |n: &str| find(n).unwrap().class;
        assert_eq!(class("tpt_lean_search"), Permission::Read);
        assert_eq!(class("tpt_lean_format_text"), Permission::Read);
        assert_eq!(class("tpt_lean_index_status"), Permission::Read);
        assert_eq!(class("tpt_lean_imports_suggest"), Permission::Read);
        assert_eq!(class("tpt_lean_format_file"), Permission::Write);
        assert_eq!(class("tpt_lean_check_file"), Permission::Build);
        assert_eq!(class("tpt_lean_build_project"), Permission::Build);
        assert_eq!(class("tpt_lean_goals_at"), Permission::Build);
        assert_eq!(class("tpt_lean_profile_file"), Permission::Profile);
    }

    #[test]
    fn module_names_follow_file_paths() {
        assert_eq!(module_of("Foo/Bar/Baz.lean"), "Foo.Bar.Baz");
        assert_eq!(module_of("Main.lean"), "Main");
    }

    #[test]
    fn tail_is_character_safe() {
        assert_eq!(tail_chars("ab𝔸cd", 3), "𝔸cd");
        assert_eq!(tail_chars("abc", 10), "abc");
    }

    #[test]
    fn imported_modules_are_read_from_source() {
        let mods = imported_modules("import Foo.Bar\nimport Baz\n\ntheorem t : True := trivial\n");
        assert!(mods.contains("Foo.Bar"));
        assert!(mods.contains("Baz"));
        assert_eq!(mods.len(), 2);
    }
}
