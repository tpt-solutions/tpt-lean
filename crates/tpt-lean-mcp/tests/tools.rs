// SPDX-License-Identifier: MIT OR Apache-2.0
//! End-to-end tool tests through the public JSON-RPC entry point.
//!
//! Tests that need a real Lean toolchain check for it first and return early when it is
//! absent, so the suite still passes on machines without Lean.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tpt_lean_adapter::discover_toolchain;
use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_index::{default_scan_roots, Index};
use tpt_lean_mcp::Server;

fn lean_available() -> bool {
    discover_toolchain().lean.is_some()
}

/// A fresh project directory with the given files (relative path, contents).
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tpt-mcp-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (rel, text) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn config(security: &str) -> (&'static str, String) {
    (".tpt-lean/config.toml", format!("[security]\n{security}\n"))
}

/// Call a tool through the JSON-RPC entry point and return the `result` object.
fn call(server: &Server, name: &str, arguments: Value) -> Value {
    let msg = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    });
    let reply = server.handle(&msg).expect("tools/call always answers");
    assert!(reply.get("error").is_none(), "protocol error: {reply}");
    reply["result"].clone()
}

fn text_of(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap()
}

fn body_of(result: &Value) -> Value {
    serde_json::from_str(text_of(result)).unwrap()
}

fn with_config(files: &mut Vec<(&'static str, String)>, security: &str) {
    files.push(config(security));
}

fn owned(files: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    files.iter().map(|(p, t)| (*p, t.to_string())).collect()
}

fn project_owned(name: &str, files: Vec<(&str, String)>) -> PathBuf {
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
    project(name, &borrowed)
}

const MESSY: &str = "def x := 1   \n";

// ---- permissions ------------------------------------------------------------------------

#[test]
fn format_file_is_denied_without_allow_write() {
    let dir = project("deny-write", &[("A.lean", MESSY)]);
    let server = Server::new(&dir).unwrap();
    let r = call(&server, "tpt_lean_format_file", json!({"file": "A.lean"}));
    assert_eq!(r["isError"], true);
    assert!(text_of(&r).contains("write permission"), "{}", text_of(&r));
    assert_eq!(
        std::fs::read_to_string(dir.join("A.lean")).unwrap(),
        MESSY,
        "file must be untouched"
    );
}

#[test]
fn format_file_writes_when_allowed() {
    let mut files = owned(&[("A.lean", MESSY)]);
    with_config(&mut files, "allow_write = true");
    let dir = project_owned("allow-write", files);
    let server = Server::new(&dir).unwrap();
    let r = call(&server, "tpt_lean_format_file", json!({"file": "A.lean"}));
    assert_eq!(r["isError"], false, "{}", text_of(&r));
    let body = body_of(&r);
    assert_eq!(body["changed"], true);
    assert_eq!(body["written"], true);
    assert_eq!(
        std::fs::read_to_string(dir.join("A.lean")).unwrap(),
        "def x := 1\n"
    );
}

#[test]
fn format_file_dry_run_does_not_write() {
    let mut files = owned(&[("A.lean", MESSY)]);
    with_config(&mut files, "allow_write = true");
    let dir = project_owned("dry-run", files);
    let server = Server::new(&dir).unwrap();
    let r = call(
        &server,
        "tpt_lean_format_file",
        json!({"file": "A.lean", "dry_run": true}),
    );
    let body = body_of(&r);
    assert_eq!(body["changed"], true);
    assert_eq!(body["written"], false);
    assert_eq!(std::fs::read_to_string(dir.join("A.lean")).unwrap(), MESSY);
}

#[test]
fn format_file_refuses_paths_outside_the_workspace() {
    let mut files = owned(&[("A.lean", MESSY)]);
    with_config(&mut files, "allow_write = true");
    let dir = project_owned("escape", files);
    let server = Server::new(&dir).unwrap();
    let r = call(
        &server,
        "tpt_lean_format_file",
        json!({"file": "../outside.lean"}),
    );
    assert_eq!(r["isError"], true);
}

#[test]
fn build_tools_are_denied_without_allow_build() {
    let mut files = owned(&[("A.lean", "def a := 1\n"), ("lakefile.toml", "name = \"x\"\n")]);
    with_config(&mut files, "allow_build = false");
    let dir = project_owned("deny-build", files);
    let server = Server::new(&dir).unwrap();
    for (tool, args) in [
        ("tpt_lean_check_file", json!({"file": "A.lean"})),
        ("tpt_lean_build_project", json!({})),
        (
            "tpt_lean_goals_at",
            json!({"file": "A.lean", "line": 1, "column": 1}),
        ),
        (
            "tpt_lean_explain_diagnostic",
            json!({"file": "A.lean", "line": 1, "column": 1}),
        ),
        (
            "tpt_lean_context_pack",
            json!({"file": "A.lean", "line": 1, "column": 1}),
        ),
    ] {
        let r = call(&server, tool, args);
        assert_eq!(r["isError"], true, "{tool} should be denied");
        assert!(
            text_of(&r).contains("build permission"),
            "{tool}: {}",
            text_of(&r)
        );
        assert!(
            text_of(&r).contains("security.allow_build"),
            "{tool}: {}",
            text_of(&r)
        );
    }
    let r = call(&server, "tpt_lean_profile_file", json!({"file": "A.lean"}));
    assert_eq!(r["isError"], true);
    assert!(text_of(&r).contains("profile permission"), "{}", text_of(&r));
    assert!(text_of(&r).contains("security.allow_build"), "{}", text_of(&r));
}

#[test]
fn profile_tool_is_denied_without_allow_profile() {
    let mut files = owned(&[("A.lean", "def a := 1\n")]);
    with_config(&mut files, "allow_profile = false");
    let dir = project_owned("deny-profile", files);
    let server = Server::new(&dir).unwrap();
    let r = call(&server, "tpt_lean_profile_file", json!({"file": "A.lean"}));
    assert_eq!(r["isError"], true);
    assert!(text_of(&r).contains("security.allow_profile"), "{}", text_of(&r));
    // Build tools are still allowed: the switches are independent. Without a Lean toolchain the
    // call fails with lean_not_found, which is fine here; the point is that it is not denied.
    let r = call(&server, "tpt_lean_check_file", json!({"file": "A.lean"}));
    assert!(!text_of(&r).contains("permission denied"), "{}", text_of(&r));
}

#[test]
fn read_tools_work_with_build_and_write_denied() {
    let mut files = owned(&[("A.lean", "def a := 1\n")]);
    with_config(&mut files, "allow_build = false\nallow_write = false");
    let dir = project_owned("read-only", files);
    let server = Server::new(&dir).unwrap();
    let r = call(&server, "tpt_lean_format_text", json!({"text": MESSY}));
    assert_eq!(r["isError"], false);
    let r = call(&server, "tpt_lean_index_status", json!({}));
    // No index yet is a tool error, not a permission error.
    assert!(!text_of(&r).contains("permission denied"));
}

// ---- profile ------------------------------------------------------------------------------

#[test]
fn profile_tool_returns_a_report_when_lean_is_available() {
    if !lean_available() {
        return;
    }
    let dir = project("profile", &[("A.lean", "def a := 1\n")]);
    let server = Server::new(&dir).unwrap();
    let r = call(&server, "tpt_lean_profile_file", json!({"file": "A.lean"}));
    assert_eq!(r["isError"], false, "{}", text_of(&r));
    let report = body_of(&r);
    assert!(report.get("wall_ms").is_some(), "{report}");
    assert!(text_of(&r).contains("A.lean"));
}

// ---- build --------------------------------------------------------------------------------

#[test]
fn build_rejects_invalid_targets_before_running_anything() {
    let dir = project("bad-target", &[("lakefile.toml", "name = \"x\"\n")]);
    let server = Server::new(&dir).unwrap();
    let r = call(&server, "tpt_lean_build_project", json!({"target": "../evil"}));
    assert_eq!(r["isError"], true);
    assert!(text_of(&r).contains("invalid Lake target"), "{}", text_of(&r));
}

#[test]
fn build_requires_a_lakefile() {
    let dir = project("no-lakefile", &[("A.lean", "def a := 1\n")]);
    let server = Server::new(&dir).unwrap();
    let r = call(&server, "tpt_lean_build_project", json!({}));
    assert_eq!(r["isError"], true);
    assert!(text_of(&r).contains("lakefile"), "{}", text_of(&r));
}

#[test]
fn build_project_runs_lake_build() {
    if !lean_available() {
        eprintln!("skipped: Lean toolchain not found");
        return;
    }
    let lakefile = "import Lake\nopen Lake DSL\n\npackage tptmcptest\n\n@[default_target]\nlean_lib Hello\n";
    let dir = project(
        "lake-build",
        &[
            ("lakefile.lean", lakefile),
            ("Hello.lean", "def hello : Nat := 1\n"),
        ],
    );
    let server = Server::new(&dir).unwrap();
    let r = call(&server, "tpt_lean_build_project", json!({"timeout_seconds": 110}));
    let body = body_of(&r);
    assert_eq!(body["timed_out"], false, "{body}");
    assert_eq!(body["success"], true, "{}", body["output_tail"]);
    assert!(body["duration_ms"].as_u64().is_some());
}

// ---- check and LSP tools ----------------------------------------------------------------

const BROKEN: &str = "theorem t (a b : Nat) : a + b = b + a := by\n  skip\n";
const CLEAN: &str = "theorem t (a b : Nat) : a + b = b + a := by\n  omega\n";

#[test]
fn check_file_reports_errors_and_fills_the_diagnostics_resource() {
    if !lean_available() {
        eprintln!("skipped: Lean toolchain not found");
        return;
    }
    let dir = project("check", &[("Bad.lean", BROKEN)]);
    let server = Server::new(&dir).unwrap();
    let r = call(&server, "tpt_lean_check_file", json!({"file": "Bad.lean"}));
    let body = body_of(&r);
    assert_eq!(body["success"], false);
    assert!(body["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["severity"] == "error"));

    let res = server
        .handle(&json!({"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":"tpt-lean://project/diagnostics"}}))
        .unwrap();
    let text = res["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(text.contains("Bad.lean"), "{text}");
}

#[test]
fn goals_at_returns_the_goal_at_a_position() {
    if !lean_available() {
        eprintln!("skipped: Lean toolchain not found");
        return;
    }
    let dir = project("goals", &[("Goal.lean", BROKEN)]);
    let server = Server::new(&dir).unwrap();
    let r = call(
        &server,
        "tpt_lean_goals_at",
        json!({"file": "Goal.lean", "line": 2, "column": 3}),
    );
    assert_eq!(r["isError"], false, "{}", text_of(&r));
    let body = body_of(&r);
    let goals: Vec<&str> = body["goals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g.as_str().unwrap())
        .collect();
    assert!(goals.iter().any(|g| g.contains("⊢ a + b = b + a")), "{goals:?}");
}

#[test]
fn explain_diagnostic_and_context_pack_describe_the_position() {
    if !lean_available() {
        eprintln!("skipped: Lean toolchain not found");
        return;
    }
    let dir = project("explain", &[("Explain.lean", BROKEN)]);
    let server = Server::new(&dir).unwrap();

    let r = call(
        &server,
        "tpt_lean_explain_diagnostic",
        json!({"file": "Explain.lean", "line": 2, "column": 3}),
    );
    assert_eq!(r["isError"], false, "{}", text_of(&r));
    let body = body_of(&r);
    assert_eq!(body["position"], json!({"line": 2, "column": 3}));
    assert!(body["goal"].as_str().unwrap_or("").contains("a + b"), "{body}");
    assert!(body["diagnostics"].is_array());

    let r = call(
        &server,
        "tpt_lean_context_pack",
        json!({"file": "Explain.lean", "line": 2, "column": 3, "max_tokens": 6000}),
    );
    assert_eq!(r["isError"], false, "{}", text_of(&r));
    let pack = body_of(&r);
    assert_eq!(pack["file"], "Explain.lean");
    assert_eq!(pack["position"], json!({"line": 2, "column": 3}));
    assert!(pack["goal"].as_str().unwrap_or("").contains("a + b"));
    assert!(pack["source_excerpt"].as_str().unwrap().contains("skip"));
    assert_eq!(pack["truncated"], false);
    assert!(pack["estimated_tokens"].as_u64().unwrap() <= 6000);
}

#[test]
fn context_pack_fits_a_small_budget() {
    if !lean_available() {
        eprintln!("skipped: Lean toolchain not found");
        return;
    }
    let dir = project("small-budget", &[("Small.lean", CLEAN)]);
    let server = Server::new(&dir).unwrap();
    let r = call(
        &server,
        "tpt_lean_context_pack",
        json!({"file": "Small.lean", "line": 2, "column": 3, "max_tokens": 200}),
    );
    assert_eq!(r["isError"], false, "{}", text_of(&r));
    let pack = body_of(&r);
    assert!(pack["estimated_tokens"].as_u64().unwrap() <= 200 || pack["truncated"] == true);
}

#[test]
fn goals_at_rejects_positions_outside_the_file() {
    let dir = project("outside-pos", &[("P.lean", "def a := 1\n")]);
    let server = Server::new(&dir).unwrap();
    // Permission passes (allow_build defaults to true); the position check fails first.
    let r = call(
        &server,
        "tpt_lean_goals_at",
        json!({"file": "P.lean", "line": 99, "column": 1}),
    );
    assert_eq!(r["isError"], true);
    assert!(text_of(&r).contains("outside the file"), "{}", text_of(&r));
    let r = call(
        &server,
        "tpt_lean_goals_at",
        json!({"file": "P.lean", "line": 0, "column": 1}),
    );
    assert_eq!(r["isError"], true);
}

// ---- index-backed tools -----------------------------------------------------------------

/// Build an index over `dir`, the same way `tpt-lean index scan` does.
fn build_index(dir: &Path) {
    let dirs = ProjectDirs::new(dir);
    dirs.ensure().unwrap();
    let mut index = Index::open(&dirs.index_db()).unwrap();
    index.refresh(&default_scan_roots(dir, false)).unwrap();
}

#[test]
fn search_and_index_status_read_the_index() {
    let dir = project(
        "search",
        &[(
            "Foo/Bar.lean",
            "namespace Foo\n\ndef addFoo (n : Nat) : Nat := n + 1\n\nend Foo\n",
        )],
    );
    build_index(&dir);
    let server = Server::new(&dir).unwrap();

    let r = call(&server, "tpt_lean_index_status", json!({}));
    let status = body_of(&r);
    assert!(status["files"].as_i64().unwrap() >= 1, "{status}");

    let r = call(&server, "tpt_lean_search", json!({"query": "addFoo", "limit": 5}));
    assert_eq!(r["isError"], false, "{}", text_of(&r));
    let results = body_of(&r)["results"].as_array().unwrap().clone();
    assert!(
        results
            .iter()
            .any(|h| h["name"].as_str().unwrap_or("").ends_with("addFoo")),
        "{results:?}"
    );
}

#[test]
fn imports_suggest_points_at_the_module_that_declares_the_symbol() {
    let dir = project(
        "imports",
        &[
            (
                "Foo/Bar.lean",
                "namespace Foo\n\ndef addFoo (n : Nat) : Nat := n + 1\n\nend Foo\n",
            ),
            ("Other.lean", "def unrelated : Nat := 0\n"),
            ("Use.lean", "import Foo.Bar\n\n#check Foo.addFoo\n"),
        ],
    );
    build_index(&dir);
    let server = Server::new(&dir).unwrap();

    let r = call(&server, "tpt_lean_imports_suggest", json!({"query": "addFoo"}));
    assert_eq!(r["isError"], false, "{}", text_of(&r));
    let body = body_of(&r);
    let first = &body["suggestions"][0];
    assert_eq!(first["module"], "Foo.Bar", "{body}");
    assert_eq!(first["statement"], "import Foo.Bar");
    assert!(first["matching_symbols"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s.as_str().unwrap().ends_with("addFoo")));

    // A file that already imports the module does not get it suggested again.
    let r = call(
        &server,
        "tpt_lean_imports_suggest",
        json!({"query": "addFoo", "file": "Use.lean"}),
    );
    let body = body_of(&r);
    let modules: Vec<&str> = body["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["module"].as_str().unwrap())
        .collect();
    assert!(!modules.contains(&"Foo.Bar"), "{modules:?}");
}

#[test]
fn imports_suggest_rejects_an_empty_query() {
    let dir = project("empty-query", &[]);
    build_index(&dir);
    let server = Server::new(&dir).unwrap();
    let r = call(&server, "tpt_lean_imports_suggest", json!({"query": "   "}));
    assert_eq!(r["isError"], true);
}

// ---- protocol through stdio -------------------------------------------------------------

#[test]
fn stdio_session_lists_resources_prompts_and_denies_writes() {
    let mut files = owned(&[("A.lean", MESSY)]);
    with_config(&mut files, "allow_write = false");
    let dir = project_owned("stdio-session", files);
    let server = Server::new(&dir).unwrap();
    let input = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}).to_string(),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
        json!({"jsonrpc":"2.0","id":2,"method":"resources/list"}).to_string(),
        json!({"jsonrpc":"2.0","id":3,"method":"prompts/get","params":{"name":"find_lemma_for_goal","arguments":{"goal":"⊢ True"}}}).to_string(),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"tpt_lean_format_file","arguments":{"file":"A.lean"}}}).to_string(),
    ]
    .join("\n");
    let mut out = Vec::new();
    tpt_lean_mcp::serve_stdio(&server, input.as_bytes(), &mut out).unwrap();
    let replies: Vec<Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(replies.len(), 4, "notifications get no reply");
    assert_eq!(replies[0]["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(replies[1]["result"]["resources"].as_array().unwrap().len(), 5);
    assert!(replies[2]["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap()
        .contains("⊢ True"));
    assert_eq!(replies[3]["result"]["isError"], true);
    assert_eq!(std::fs::read_to_string(dir.join("A.lean")).unwrap(), MESSY);
}

// ---- input schemas ----------------------------------------------------------------------

/// The tools as `tools/list` publishes them.
fn listed_tools(server: &Server) -> Vec<Value> {
    let reply = server
        .handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .expect("tools/list always answers");
    reply["result"]["tools"].as_array().unwrap().clone()
}

fn type_matches(name: &str, value: &Value) -> bool {
    match name {
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => false,
    }
}

/// Checks `value` against the parts of JSON Schema that the tool schemas use: `type` (a name or a
/// list of names), `enum`, `minimum`, `maximum`, `required`, `properties`, and `items`. Returns one
/// message per violation. It is hand-written because the workspace has no JSON Schema validator.
fn violations(schema: &Value, value: &Value, at: &str) -> Vec<String> {
    let mut out = Vec::new();
    let names: Vec<&str> = match schema.get("type") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if !names.is_empty() && !names.iter().any(|n| type_matches(n, value)) {
        out.push(format!("{at}: expected {names:?}, got {value}"));
        return out;
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            out.push(format!("{at}: {value} is not one of the allowed values"));
        }
    }
    if let Some(n) = value.as_i64() {
        if let Some(min) = schema.get("minimum").and_then(Value::as_i64) {
            if n < min {
                out.push(format!("{at}: {n} is below the minimum {min}"));
            }
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_i64) {
            if n > max {
                out.push(format!("{at}: {n} is above the maximum {max}"));
            }
        }
    }
    if let Some(obj) = value.as_object() {
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for key in required.filter_map(Value::as_str) {
            if !obj.contains_key(key) {
                out.push(format!("{at}: missing required property `{key}`"));
            }
        }
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (key, sub) in props {
                if let Some(v) = obj.get(key) {
                    out.extend(violations(sub, v, &format!("{at}.{key}")));
                }
            }
        }
    }
    if let (Some(items), Some(list)) = (schema.get("items"), value.as_array()) {
        for (i, v) in list.iter().enumerate() {
            out.extend(violations(items, v, &format!("{at}[{i}]")));
        }
    }
    out
}

/// A value that satisfies `schema`: its required properties, each the smallest allowed value of
/// its type. Optional properties are left out.
fn sample(schema: &Value) -> Value {
    if let Some(first) = schema
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        return first.clone();
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => json!("x"),
        Some("integer") => json!(schema.get("minimum").and_then(Value::as_i64).unwrap_or(1)),
        Some("boolean") => json!(true),
        Some("array") => json!([]),
        Some("object") => {
            let mut obj = serde_json::Map::new();
            let props = schema.get("properties").and_then(Value::as_object);
            for key in schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let key = key.as_str().unwrap();
                let sub = props.and_then(|p| p.get(key)).cloned().unwrap_or(json!({}));
                obj.insert(key.to_string(), sample(&sub));
            }
            Value::Object(obj)
        }
        _ => Value::Null,
    }
}

#[test]
fn every_input_schema_accepts_its_sample_and_rejects_a_missing_required_property() {
    let dir = project("schema-validate", &[("A.lean", "def a := 1\n")]);
    let server = Server::new(&dir).unwrap();
    let tools = listed_tools(&server);
    assert!(!tools.is_empty());
    for tool in &tools {
        let name = tool["name"].as_str().unwrap();
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{name}: input must be an object");
        let args = sample(schema);
        let problems = violations(schema, &args, name);
        assert!(problems.is_empty(), "{name}: sample rejected: {problems:?}");
        for key in schema["required"].as_array().unwrap() {
            let key = key.as_str().unwrap();
            let mut missing = args.clone();
            missing.as_object_mut().unwrap().remove(key);
            let problems = violations(schema, &missing, name);
            assert!(
                problems.iter().any(|p| p.contains(key)),
                "{name}: dropping `{key}` was not reported: {problems:?}"
            );
        }
    }
}

#[test]
fn schema_rejects_out_of_range_and_wrong_typed_values() {
    let dir = project("schema-values", &[("A.lean", "def a := 1\n")]);
    let server = Server::new(&dir).unwrap();
    let tools = listed_tools(&server);
    let check = tools.iter().find(|t| t["name"] == "tpt_lean_check_file").unwrap();
    let schema = &check["inputSchema"];
    assert!(violations(schema, &json!({"file": "A.lean"}), "args").is_empty());
    assert!(
        !violations(schema, &json!({"file": 7}), "args").is_empty(),
        "file must be a string"
    );
    assert!(!violations(schema, &json!({"file": "A.lean", "timeout_seconds": 0}), "args").is_empty());
    assert!(!violations(schema, &json!({"file": "A.lean", "timeout_seconds": "5"}), "args").is_empty());
    assert!(
        !violations(schema, &json!("A.lean"), "args").is_empty(),
        "arguments must be an object"
    );
}

#[test]
fn server_answers_missing_required_arguments_with_a_tool_error() {
    // Every permission granted, so a refusal can only come from the arguments.
    let mut files = owned(&[("A.lean", "def a := 1\n")]);
    with_config(
        &mut files,
        "allow_write = true\nallow_build = true\nallow_profile = true",
    );
    let dir = project_owned("missing-args", files);
    let server = Server::new(&dir).unwrap();
    for tool in listed_tools(&server) {
        let name = tool["name"].as_str().unwrap();
        if tool["inputSchema"]["required"].as_array().unwrap().is_empty() {
            continue;
        }
        let r = call(&server, name, json!({}));
        assert_eq!(r["isError"], true, "{name} accepted no arguments: {r}");
        assert!(
            !text_of(&r).contains("permission denied"),
            "{name}: {}",
            text_of(&r)
        );
    }
}

// ---- permissions, from the tool table -------------------------------------------------

#[test]
fn every_tool_outside_the_granted_permissions_is_refused_before_it_runs() {
    let mut files = owned(&[("A.lean", "def a := 1\n")]);
    with_config(&mut files, "allow_write = false\nallow_build = false");
    let dir = project_owned("perm-table", files);
    let server = Server::new(&dir).unwrap();
    let before = std::fs::read_to_string(dir.join("A.lean")).unwrap();
    // The tools that need no permission (Permission::Read in the tool table).
    let read_only = [
        "tpt_lean_search",
        "tpt_lean_format_text",
        "tpt_lean_index_status",
        "tpt_lean_imports_suggest",
    ];
    for tool in listed_tools(&server) {
        let name = tool["name"].as_str().unwrap();
        // Valid arguments, so that only the permission can refuse the call.
        let args = sample(&tool["inputSchema"]);
        let r = call(&server, name, args);
        let text = text_of(&r).to_string();
        if read_only.contains(&name) {
            assert!(
                !text.contains("permission denied"),
                "{name} needs no permission: {text}"
            );
        } else {
            assert_eq!(r["isError"], true, "{name} ran without its permission");
            assert!(text.starts_with("permission denied"), "{name}: {text}");
        }
    }
    assert_eq!(std::fs::read_to_string(dir.join("A.lean")).unwrap(), before);
}

// ---- timeouts ---------------------------------------------------------------------------

#[test]
fn check_file_with_a_one_second_timeout_returns_instead_of_hanging() {
    if !lean_available() {
        eprintln!("skipped: Lean toolchain not found");
        return;
    }
    // Elaboration of this file takes minutes; the call must come back after about one second.
    let dir = project(
        "timeout",
        &[("Slow.lean", "#eval (List.range 400000000).foldl (· + ·) 0\n")],
    );
    let server = Server::new(&dir).unwrap();
    let started = std::time::Instant::now();
    let r = call(
        &server,
        "tpt_lean_check_file",
        json!({"file": "Slow.lean", "timeout_seconds": 1}),
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(90),
        "took {:?}",
        started.elapsed()
    );
    let body = body_of(&r);
    assert_eq!(body["timed_out"], true, "{}", text_of(&r));
    assert_eq!(body["success"], false);
    assert_eq!(body["cancelled"], false);
}
