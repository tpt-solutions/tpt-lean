// SPDX-License-Identifier: MIT OR Apache-2.0
//! Schema tests. Loads `schemas/*.json`, checks that every dispatched method has a schema with
//! the same permission class, and validates real responses against the `result` and `params`
//! schemas. The validator implements only the subset these schemas use: `type`, `required`,
//! `properties`, `additionalProperties: false`, `items`, `enum`, `minimum`, `maximum`, and
//! local `$ref` into the file's `$defs`. It is test-only; the crate takes no schema dependency.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tpt_lean_adapter::discover_toolchain;
use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_index::{default_scan_roots, Index};
use tpt_lean_server::{permission, Server, PERMISSION_DENIED};

fn schema_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas")
}

/// All schema files, keyed by their `title`.
fn load_schemas() -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(schema_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
        let title = v["title"].as_str().unwrap().to_string();
        out.insert(title, v);
    }
    out
}

fn type_matches(t: &str, v: &Value) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        other => panic!("validator does not support type {other:?}"),
    }
}

/// Validate `value` against `schema`, appending a message per violation to `errs`.
/// `file` is the whole schema file, which holds the `$defs` that `$ref` points into.
fn validate(file: &Value, schema: &Value, value: &Value, at: &str, errs: &mut Vec<String>) {
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        let name = r
            .strip_prefix("#/$defs/")
            .unwrap_or_else(|| panic!("unsupported $ref {r}"));
        let target = &file["$defs"][name];
        assert!(!target.is_null(), "missing $defs/{name}");
        return validate(file, target, value, at, errs);
    }
    if let Some(t) = schema.get("type") {
        let types: Vec<&str> = match t {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().map(|x| x.as_str().unwrap()).collect(),
            _ => panic!("bad type keyword at {at}"),
        };
        if !types.iter().any(|t| type_matches(t, value)) {
            errs.push(format!("{at}: expected {t}, got {value}"));
            return;
        }
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            errs.push(format!("{at}: {value} is not one of {allowed:?}"));
        }
    }
    if let Some(n) = value.as_f64() {
        if let Some(min) = schema.get("minimum").and_then(Value::as_f64) {
            if n < min {
                errs.push(format!("{at}: {n} < minimum {min}"));
            }
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_f64) {
            if n > max {
                errs.push(format!("{at}: {n} > maximum {max}"));
            }
        }
    }
    if let Some(obj) = value.as_object() {
        for key in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let key = key.as_str().unwrap();
            if !obj.contains_key(key) {
                errs.push(format!("{at}: missing required property {key:?}"));
            }
        }
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (key, sub) in props {
                if let Some(v) = obj.get(key) {
                    validate(file, sub, v, &format!("{at}.{key}"), errs);
                }
            }
            if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                for key in obj.keys() {
                    if !props.contains_key(key) {
                        errs.push(format!("{at}: unexpected property {key:?}"));
                    }
                }
            }
        }
    }
    if let (Some(arr), Some(items)) = (value.as_array(), schema.get("items")) {
        for (i, v) in arr.iter().enumerate() {
            validate(file, items, v, &format!("{at}[{i}]"), errs);
        }
    }
}

/// Panics with every violation if `value` does not match `schema` (inside `file`).
fn assert_valid(file: &Value, schema: &Value, value: &Value, what: &str) {
    let mut errs = Vec::new();
    validate(file, schema, value, what, &mut errs);
    assert!(
        errs.is_empty(),
        "{what} does not match its schema:\n{}\nvalue: {value}",
        errs.join("\n")
    );
}

fn unique_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("tpt-server-schema-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lakefile.toml"), "name = \"probe\"\n").unwrap();
    std::fs::write(
        dir.join("Foo.lean"),
        "theorem add_comm_probe : True := trivial   \n",
    )
    .unwrap();
    dir
}

/// A project with a Lean file, an index, and write permission enabled.
fn writable_fixture(tag: &str) -> (PathBuf, Server) {
    let dir = unique_dir(tag);
    std::fs::write(dir.join("tpt-lean.toml"), "[security]\nallow_write = true\n").unwrap();
    let server = Server::new(&dir).unwrap();
    let dirs = ProjectDirs::new(&dir);
    let mut index = Index::open(&dirs.index_db()).unwrap();
    index.refresh(&default_scan_roots(&dir, false)).unwrap();
    drop(index);
    (dir, server)
}

fn rpc(server: &Server, method: &str, params: Value) -> Value {
    server
        .handle(&json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params }))
        .expect("request with id must get a reply")
}

#[test]
fn every_dispatched_method_has_a_schema_with_matching_permission() {
    let schemas = load_schemas();
    for (method, perm) in permission::METHODS {
        let schema = schemas
            .get(*method)
            .unwrap_or_else(|| panic!("no schema for {method}"));
        assert_eq!(schema["permission"], perm.as_str(), "{method}");
        assert!(schema.get("params").is_some(), "{method} has no params schema");
        assert!(schema.get("result").is_some(), "{method} has no result schema");
    }
    assert!(schemas.contains_key("JSON-RPC error response"));
    assert_eq!(schemas.len(), permission::METHODS.len() + 1, "stray schema files");
}

#[test]
fn real_responses_match_their_schemas() {
    let (dir, server) = writable_fixture("samples");
    let schemas = load_schemas();
    let error_file = schemas["JSON-RPC error response"].clone();
    let lean_missing = discover_toolchain().lean.is_none();

    let cases: Vec<(&str, Value)> = vec![
        ("project.info", json!({})),
        ("index.refresh", json!({})),
        ("index.status", json!({})),
        ("search.query", json!({ "query": "add_comm_probe", "limit": 5 })),
        ("search.symbol", json!({ "name": "add_comm_probe" })),
        ("format.text", json!({ "text": "def x := 1   \n" })),
        ("format.file", json!({ "path": "Foo.lean" })),
        ("diagnostics.get", json!({ "path": "Foo.lean" })),
        ("check.file", json!({ "path": "Foo.lean" })),
        ("goals.at", json!({ "path": "Foo.lean", "line": 1, "column": 1 })),
    ];
    for (method, params) in cases {
        let file = &schemas[method];
        assert_valid(file, &file["params"], &params, &format!("{method} params"));
        let r = rpc(&server, method, params);
        if r.get("error").is_some() {
            // Only the Lean-backed methods may fail, and only because Lean is absent.
            let expected_failure = lean_missing && matches!(method, "check.file" | "goals.at");
            assert!(expected_failure, "{method} failed unexpectedly: {r}");
            assert_valid(&error_file, &error_file, &r, &format!("{method} error"));
        } else {
            assert_valid(file, &file["result"], &r["result"], &format!("{method} result"));
        }
    }

    // Shutdown on its own server, so the fixture above keeps running for the other cases.
    let (dir2, server2) = writable_fixture("shutdown");
    let r = rpc(&server2, "server.shutdown", json!({}));
    let file = &schemas["server.shutdown"];
    assert_valid(file, &file["result"], &r["result"], "server.shutdown result");

    // A permission denial is an error envelope that matches the error schema.
    let denied_dir = unique_dir("denied");
    let denied_server = Server::new(&denied_dir).unwrap();
    let r = rpc(&denied_server, "format.file", json!({ "path": "Foo.lean" }));
    assert_eq!(r["error"]["code"], PERMISSION_DENIED);
    assert_valid(&error_file, &error_file, &r, "denied error");

    for d in [dir, dir2, denied_dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn validator_rejects_bad_responses() {
    let schemas = load_schemas();
    let format = &schemas["format.text"];
    let mut errs = Vec::new();
    validate(
        format,
        &format["result"],
        &json!({ "formatted": "x" }),
        "result",
        &mut errs,
    );
    assert!(errs
        .iter()
        .any(|e| e.contains("missing required property \"changed\"")));

    errs.clear();
    validate(
        format,
        &format["result"],
        &json!({ "formatted": 3, "changed": true }),
        "result",
        &mut errs,
    );
    assert!(errs.iter().any(|e| e.contains("expected \"string\"")), "{errs:?}");

    let check = &schemas["check.file"];
    let bad = json!({
        "path": "Foo.lean", "ok": false, "exit_code": 1, "timed_out": false,
        "output_truncated": false,
        "diagnostics": [{ "file": "Foo.lean", "line": 1, "column": 0, "severity": "fatal", "message": "x" }]
    });
    errs.clear();
    validate(check, &check["result"], &bad, "result", &mut errs);
    assert!(errs.iter().any(|e| e.contains("not one of")), "{errs:?}");

    let search = &schemas["search.query"];
    errs.clear();
    validate(
        search,
        &search["params"],
        &json!({ "query": "x", "limit": 0 }),
        "params",
        &mut errs,
    );
    assert!(errs.iter().any(|e| e.contains("minimum")), "{errs:?}");

    errs.clear();
    validate(
        search,
        &search["params"],
        &json!({ "query": "x", "extra": 1 }),
        "params",
        &mut errs,
    );
    assert!(
        errs.is_empty(),
        "extra properties are allowed by this schema: {errs:?}"
    );
}
