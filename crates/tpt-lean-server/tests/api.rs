// SPDX-License-Identifier: MIT OR Apache-2.0
//! Behaviour tests for the API: permission enforcement, path safety, parameter validation,
//! index and formatting methods, and the TCP lifecycle (`run_foreground`, `status`, `stop`).

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tpt_lean_adapter::discover_toolchain;
use tpt_lean_core::dirs::ProjectDirs;
use tpt_lean_index::{default_scan_roots, Index};
use tpt_lean_server::{
    run_foreground, serve_connection, state, Server, StopOutcome, APP_ERROR, INVALID_PARAMS,
    PATH_OUTSIDE_WORKSPACE, PERMISSION_DENIED,
};

/// A temporary project directory removed on drop.
struct TempProject {
    dir: PathBuf,
}

impl TempProject {
    fn new(tag: &str, config: Option<&str>) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("tpt-server-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lakefile.toml"), "name = \"probe\"\n").unwrap();
        if let Some(cfg) = config {
            std::fs::write(dir.join("tpt-lean.toml"), cfg).unwrap();
        }
        Self { dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn write(&self, rel: &str, text: &str) {
        std::fs::write(self.dir.join(rel), text).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.dir.join(rel)).unwrap()
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const WRITE_ON: &str = "[security]\nallow_write = true\n";
const BUILD_OFF: &str = "[security]\nallow_build = false\n";

/// Send one request through `handle` and return the full response object.
fn rpc(server: &Server, method: &str, params: Value) -> Value {
    server
        .handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .expect("request with id must get a reply")
}

fn result_of(response: &Value) -> &Value {
    assert!(response.get("error").is_none(), "unexpected error: {response}");
    &response["result"]
}

fn error_code(response: &Value) -> i64 {
    response["error"]["code"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected an error, got {response}"))
}

#[test]
fn project_info_reports_permissions_from_config() {
    let default = TempProject::new("info-default", None);
    let server = Server::new(default.path()).unwrap();
    let r = rpc(&server, "project.info", json!({}));
    let perms = &result_of(&r)["permissions"];
    assert_eq!(perms["write"], false);
    assert_eq!(perms["build"], true);

    let writable = TempProject::new("info-write", Some(WRITE_ON));
    let server = Server::new(writable.path()).unwrap();
    let r = rpc(&server, "project.info", json!({}));
    assert_eq!(result_of(&r)["permissions"]["write"], true);
}

#[test]
fn write_methods_are_denied_by_default_and_leave_files_alone() {
    let project = TempProject::new("deny-write", None);
    project.write("Foo.lean", "def x := 1   \n");
    let server = Server::new(project.path()).unwrap();

    let r = rpc(&server, "format.file", json!({ "path": "Foo.lean" }));
    assert_eq!(error_code(&r), PERMISSION_DENIED);
    assert_eq!(r["error"]["data"]["permission"], "write");
    assert_eq!(r["error"]["data"]["config_key"], "security.allow_write");
    assert_eq!(project.read("Foo.lean"), "def x := 1   \n");

    let r = rpc(&server, "index.refresh", json!({}));
    assert_eq!(error_code(&r), PERMISSION_DENIED);
}

#[test]
fn build_methods_are_denied_when_disabled() {
    let project = TempProject::new("deny-build", Some(BUILD_OFF));
    project.write("Foo.lean", "def x := 1\n");
    let server = Server::new(project.path()).unwrap();
    for method in ["check.file", "goals.at", "check.project"] {
        let r = rpc(
            &server,
            method,
            json!({ "path": "Foo.lean", "line": 1, "column": 1 }),
        );
        assert_eq!(error_code(&r), PERMISSION_DENIED, "{method}");
        assert_eq!(r["error"]["data"]["config_key"], "security.allow_build");
    }
}

#[test]
fn format_file_writes_once_then_is_idempotent() {
    let project = TempProject::new("format-file", Some(WRITE_ON));
    project.write("Foo.lean", "def x := 1   \n");
    let server = Server::new(project.path()).unwrap();

    let r = rpc(&server, "format.file", json!({ "path": "Foo.lean" }));
    let res = result_of(&r);
    assert_eq!(res["changed"], true);
    assert_eq!(res["written"], true);
    assert_eq!(project.read("Foo.lean"), "def x := 1\n");
    // No temporary file is left behind.
    assert!(!project.path().join(".Foo.lean.tpt-lean-tmp").exists());

    let r = rpc(&server, "format.file", json!({ "path": "Foo.lean" }));
    assert_eq!(result_of(&r)["changed"], false);
    assert_eq!(result_of(&r)["written"], false);
}

#[test]
fn format_file_refuses_uncertain_syntax_and_keeps_the_file() {
    let project = TempProject::new("format-refuse", Some(WRITE_ON));
    let original = "theorem t := by\n\tsimp\n";
    project.write("Bad.lean", original);
    let server = Server::new(project.path()).unwrap();
    let r = rpc(&server, "format.file", json!({ "path": "Bad.lean" }));
    assert_eq!(error_code(&r), APP_ERROR);
    assert!(r["error"]["message"]
        .as_str()
        .unwrap()
        .contains("refusing to format"));
    assert_eq!(project.read("Bad.lean"), original);
}

#[test]
fn path_parameters_are_confined_to_the_project() {
    let project = TempProject::new("paths", Some(WRITE_ON));
    let outside = TempProject::new("paths-outside", None);
    outside.write("Secret.lean", "def s := 1\n");
    let server = Server::new(project.path()).unwrap();

    let escaping = [
        "../Secret.lean".to_string(),
        outside.path().join("Secret.lean").display().to_string(),
    ];
    for path in &escaping {
        for method in ["format.file", "diagnostics.get", "check.file"] {
            let r = rpc(&server, method, json!({ "path": path }));
            assert_eq!(error_code(&r), PATH_OUTSIDE_WORKSPACE, "{method} {path}");
        }
    }
    // The outside file was not touched.
    assert_eq!(outside.read("Secret.lean"), "def s := 1\n");
}

#[test]
fn path_parameters_must_be_lean_files_without_nul() {
    let project = TempProject::new("paths-shape", Some(WRITE_ON));
    project.write("Notes.txt", "x  \n");
    let server = Server::new(project.path()).unwrap();

    let r = rpc(&server, "format.file", json!({ "path": "Notes.txt" }));
    assert_eq!(error_code(&r), INVALID_PARAMS);
    assert_eq!(project.read("Notes.txt"), "x  \n");

    let r = rpc(&server, "diagnostics.get", json!({ "path": "bad\u{0}name.lean" }));
    assert_eq!(error_code(&r), INVALID_PARAMS);

    let r = rpc(&server, "diagnostics.get", json!({}));
    assert_eq!(error_code(&r), INVALID_PARAMS);
}

#[test]
fn index_refresh_then_symbol_and_query_lookups() {
    let project = TempProject::new("index", Some(WRITE_ON));
    project.write("Probe.lean", "theorem add_comm_probe : True := trivial\n");
    let server = Server::new(project.path()).unwrap();

    let r = rpc(&server, "search.symbol", json!({ "name": "add_comm_probe" }));
    assert_eq!(error_code(&r), APP_ERROR, "no index before refresh");

    let r = rpc(&server, "index.refresh", json!({}));
    assert!(result_of(&r)["scanned"].as_u64().unwrap() >= 1);

    let r = rpc(&server, "search.symbol", json!({ "name": "add_comm_probe" }));
    let items = result_of(&r)["items"].as_array().unwrap();
    assert_eq!(items[0]["name"], "add_comm_probe");
    assert_eq!(items[0]["source"], "Probe.lean");

    let r = rpc(
        &server,
        "search.query",
        json!({ "query": "add_comm_probe", "limit": 3 }),
    );
    assert_eq!(result_of(&r)["items"][0]["name"], "add_comm_probe");
    let r = rpc(&server, "index.status", json!({}));
    assert!(result_of(&r)["files"].as_i64().unwrap() >= 1);
}

#[test]
fn index_written_by_the_library_is_visible_through_the_api() {
    // Index written by the library directly, then read through the API.
    let project = TempProject::new("index-direct", None);
    project.write("Probe.lean", "def probe_value := 1\n");
    let dirs = ProjectDirs::new(project.path());
    let mut index = Index::open(&dirs.index_db()).unwrap();
    index.refresh(&default_scan_roots(project.path(), false)).unwrap();
    drop(index);
    let server = Server::new(project.path()).unwrap();
    let r = rpc(&server, "search.symbol", json!({ "name": "probe_value" }));
    assert_eq!(result_of(&r)["items"][0]["name"], "probe_value");
}

#[test]
fn diagnostics_get_is_empty_until_check_runs() {
    let project = TempProject::new("diag", None);
    project.write("Foo.lean", "def x := 1\n");
    let server = Server::new(project.path()).unwrap();
    let r = rpc(&server, "diagnostics.get", json!({ "path": "Foo.lean" }));
    let res = result_of(&r);
    assert_eq!(res["known"], false);
    assert_eq!(res["diagnostics"].as_array().unwrap().len(), 0);
}

#[test]
fn check_file_either_runs_lean_or_reports_it_missing() {
    let project = TempProject::new("check", None);
    project.write("Foo.lean", "def x := 1\n");
    let server = Server::new(project.path()).unwrap();
    let r = rpc(&server, "check.file", json!({ "path": "Foo.lean" }));
    if discover_toolchain().lean.is_none() {
        assert_eq!(error_code(&r), APP_ERROR);
        assert!(r["error"]["message"].as_str().unwrap().contains("lean"));
    } else {
        assert!(result_of(&r)["ok"].is_boolean());
    }
}

#[test]
fn goals_at_validates_positions_before_starting_lean() {
    let project = TempProject::new("goals-pos", None);
    project.write("Foo.lean", "def x := 1\n");
    let server = Server::new(project.path()).unwrap();
    let r = rpc(
        &server,
        "goals.at",
        json!({ "path": "Foo.lean", "line": 0, "column": 1 }),
    );
    assert_eq!(error_code(&r), INVALID_PARAMS);
    let r = rpc(
        &server,
        "goals.at",
        json!({ "path": "Foo.lean", "line": 99, "column": 1 }),
    );
    assert_eq!(error_code(&r), INVALID_PARAMS);
    let r = rpc(
        &server,
        "goals.at",
        json!({ "path": "Foo.lean", "line": "1", "column": 1 }),
    );
    assert_eq!(error_code(&r), INVALID_PARAMS);
}

#[test]
fn unknown_method_and_shutdown_flag() {
    let project = TempProject::new("shutdown", None);
    let server = Server::new(project.path()).unwrap();
    let r = rpc(&server, "no.such.method", json!({}));
    assert_eq!(error_code(&r), -32601);
    assert!(!server.is_shutdown_requested());
    let r = rpc(&server, "server.shutdown", json!({}));
    assert_eq!(result_of(&r)["stopping"], true);
    assert!(server.is_shutdown_requested());
}

/// Serve one connection over a loopback socket and return the first reply line.
fn one_line_over_tcp(server: Arc<Server>, request: &str) -> Value {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = listener.local_addr().unwrap();
    let handler = {
        let server = Arc::clone(&server);
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve_connection(&server, stream).unwrap();
        })
    };
    let mut client = TcpStream::connect(addr).unwrap();
    client.write_all(request.as_bytes()).unwrap();
    client.write_all(b"\n").unwrap();
    client.shutdown(std::net::Shutdown::Write).unwrap();
    let mut line = String::new();
    BufReader::new(client).read_line(&mut line).unwrap();
    handler.join().unwrap();
    serde_json::from_str(&line).unwrap()
}

#[test]
fn tcp_round_trip_and_parse_error() {
    let project = TempProject::new("tcp", None);
    let server = Arc::new(Server::new(project.path()).unwrap());
    let r = one_line_over_tcp(
        Arc::clone(&server),
        r#"{"jsonrpc":"2.0","id":9,"method":"project.info"}"#,
    );
    assert_eq!(r["id"], 9);
    assert!(r["result"]["methods"].is_array());

    let r = one_line_over_tcp(server, "{not json");
    assert_eq!(r["error"]["code"], -32700);
}

fn wait_for_state(root: &Path) -> state::ServerState {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(s) = state::read_state(root).unwrap() {
            return s;
        }
        assert!(Instant::now() < deadline, "server state file never appeared");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn lifecycle_status_stop_and_second_start_refused() {
    let project = TempProject::new("lifecycle", None);
    let root = project.path().to_path_buf();
    let runner = {
        let root = root.clone();
        thread::spawn(move || run_foreground(&root, 0))
    };
    let st = wait_for_state(&root);
    assert!(st.port > 0);

    let status = state::status(&root).unwrap();
    assert!(status.running && !status.stale);
    assert_eq!(status.state.as_ref().unwrap().port, st.port);

    // A second start for the same project is refused while the first answers.
    let second = run_foreground(&root, 0);
    assert!(second.is_err());

    let report = state::stop(&root).unwrap();
    assert_eq!(report.outcome, StopOutcome::Stopped);
    runner.join().unwrap().unwrap();

    assert!(!state::status(&root).unwrap().running);
    assert_eq!(state::stop(&root).unwrap().outcome, StopOutcome::NotRunning);
}

#[cfg(unix)]
#[test]
fn unix_socket_lifecycle_uses_an_owner_only_socket() {
    use std::os::unix::fs::PermissionsExt;
    use tpt_lean_server::Transport;

    let project = TempProject::new("unix-socket", None);
    let root = project.path().to_path_buf();
    let runner = {
        let root = root.clone();
        thread::spawn(move || state::run_foreground_on(&root, Transport::Unix))
    };
    let st = wait_for_state(&root);
    assert_eq!(st.port, 0);
    let socket = st.socket.clone().expect("state records the socket path");
    assert_eq!(socket, state::socket_path(&root));
    let mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    let status = state::status(&root).unwrap();
    assert!(status.running && !status.stale);

    let reply = tpt_lean_server::client::call_endpoint(
        &st.endpoint(),
        "project.info",
        json!({}),
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(reply.get("result").is_some());

    assert_eq!(state::stop(&root).unwrap().outcome, StopOutcome::Stopped);
    runner.join().unwrap().unwrap();
    assert!(!socket.exists(), "socket file is removed on exit");
}

#[test]
fn stale_state_file_is_reported_and_cleaned_up() {
    let project = TempProject::new("stale", None);
    // Bind and drop a listener to get a port nothing listens on.
    let port = {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        l.local_addr().unwrap().port()
    };
    let dir = project.path().join(".tpt-lean");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("server.json"),
        json!({ "pid": 4242, "port": port, "project": "x", "version": "0" }).to_string(),
    )
    .unwrap();

    let status = state::status(project.path()).unwrap();
    assert!(!status.running);
    assert!(status.stale);

    let report = state::stop(project.path()).unwrap();
    assert_eq!(report.outcome, StopOutcome::StaleRemoved);
    assert!(!dir.join("server.json").exists());
}

#[test]
fn profile_run_needs_allow_profile() {
    let project = TempProject::new("deny-profile", Some("[security]\nallow_profile = false\n"));
    project.write("Foo.lean", "theorem t : 1 = 1 := rfl\n");
    let server = Server::new(project.path()).unwrap();
    let r = rpc(&server, "profile.run", json!({ "path": "Foo.lean" }));
    assert_eq!(error_code(&r), PERMISSION_DENIED);
    assert_eq!(r["error"]["data"]["permission"], "profile");
    assert_eq!(
        r["error"]["data"]["config_key"],
        "security.allow_build and security.allow_profile"
    );
}

#[test]
fn profile_run_returns_a_report() {
    if tpt_lean_adapter::discover_toolchain().lean.is_none() {
        eprintln!("skipping: lean not found");
        return;
    }
    let project = TempProject::new("profile-run", None);
    project.write("Foo.lean", "theorem t : 1 = 1 := rfl\n");
    let server = Server::new(project.path()).unwrap();
    let r = rpc(&server, "profile.run", json!({ "path": "Foo.lean" }));
    let report = result_of(&r);
    assert_eq!(report["format_version"], 1);
    assert!(report["declarations"].is_array(), "{report}");
    assert!(report["wall_ms"].as_f64().unwrap_or(-1.0) >= 0.0, "{report}");
    // The run is stored under its id, and profile.get returns the same report.
    let id = report["id"]
        .as_str()
        .expect("profile.run returns an id")
        .to_string();
    let stored = rpc(&server, "profile.get", json!({ "id": id }));
    assert_eq!(
        result_of(&stored)["report"]["wall_ms"],
        report["wall_ms"],
        "{stored}"
    );
}

#[test]
fn profile_get_lists_and_returns_stored_reports() {
    let project = TempProject::new("profile-get", None);
    let dir = project.path().join(".tpt-lean").join("profiles");
    std::fs::create_dir_all(&dir).unwrap();
    let report = json!({
        "format_version": 1,
        "file": "Foo.lean",
        "wall_ms": 1.0,
        "lean_exit_code": null,
        "lean_errors": 0,
        "declared_ms": 0.0,
        "declarations": [],
        "categories": {},
        "tactics": [],
        "suggestions": [],
        "notes": []
    });
    std::fs::write(dir.join("0000000000001-Foo.json"), report.to_string()).unwrap();
    let server = Server::new(project.path()).unwrap();

    let listed = rpc(&server, "profile.get", json!({}));
    let ids = result_of(&listed)["ids"].as_array().unwrap().clone();
    assert_eq!(ids, vec![json!("0000000000001-Foo")]);
    assert!(result_of(&listed)["report"].is_null());

    let one = rpc(&server, "profile.get", json!({ "id": "0000000000001-Foo" }));
    assert_eq!(result_of(&one)["report"]["file"], "Foo.lean");

    // Ids that could escape the profiles directory are refused, not read.
    let bad = rpc(&server, "profile.get", json!({ "id": "../config" }));
    assert_eq!(error_code(&bad), -32602, "{bad}");
    let unknown = rpc(&server, "profile.get", json!({ "id": "missing" }));
    assert_eq!(error_code(&unknown), -32602, "{unknown}");
}

#[test]
fn build_status_and_cancel_report_nothing_running_without_a_build() {
    let project = TempProject::new("build-none", None);
    let server = Server::new(project.path()).unwrap();
    let status = rpc(&server, "build.status", json!({}));
    assert_eq!(result_of(&status)["status"], "none");
    let cancel = rpc(&server, "build.cancel", json!({}));
    assert_eq!(result_of(&cancel)["cancelled"], false);
}

#[test]
fn build_run_rejects_bad_targets() {
    let project = TempProject::new("build-target", None);
    let server = Server::new(project.path()).unwrap();
    let r = rpc(&server, "build.run", json!({ "target": "../escape" }));
    assert!(r.get("error").is_some(), "{r}");
    let r = rpc(&server, "build.run", json!({ "target": 3 }));
    assert_eq!(error_code(&r), -32602, "{r}");
}

#[test]
fn build_run_builds_in_the_background_and_reports_the_result() {
    if tpt_lean_adapter::discover_toolchain().lake.is_none() {
        eprintln!("skipping: lake not found");
        return;
    }
    let project = TempProject::new("build-run", None);
    project.write(
        "lakefile.lean",
        "import Lake\nopen Lake DSL\n\npackage demo\n\n@[default_target]\nlean_lib Demo\n",
    );
    project.write("Demo.lean", "def greeting : String := \"hi\"\n");
    let server = Server::new(project.path()).unwrap();

    let started = rpc(&server, "build.run", json!({}));
    let run_id = result_of(&started)["run_id"].as_str().unwrap().to_string();
    assert_eq!(result_of(&started)["status"], "running");

    // A second build is refused while the first runs.
    let again = rpc(&server, "build.run", json!({}));
    assert!(again.get("error").is_some(), "{again}");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let finished = loop {
        let st = rpc(&server, "build.status", json!({ "run_id": run_id }));
        if result_of(&st)["status"] != "running" {
            break st;
        }
        assert!(std::time::Instant::now() < deadline, "build did not finish");
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    assert_eq!(result_of(&finished)["status"], "passed", "{finished}");
    assert_eq!(result_of(&finished)["result"]["success"], true, "{finished}");
}

#[test]
fn build_cancel_is_consistent_with_the_final_status() {
    if tpt_lean_adapter::discover_toolchain().lake.is_none() {
        eprintln!("skipping: lake not found");
        return;
    }
    let project = TempProject::new("build-cancel", None);
    project.write(
        "lakefile.lean",
        "import Lake\nopen Lake DSL\n\npackage demo\n\n@[default_target]\nlean_lib Demo\n",
    );
    project.write("Demo.lean", "def greeting : String := \"hi\"\n");
    let server = Server::new(project.path()).unwrap();
    let started = rpc(&server, "build.run", json!({}));
    let run_id = result_of(&started)["run_id"].as_str().unwrap().to_string();

    // The build may already have finished, in which case cancel reports nothing to stop.
    let cancel = rpc(&server, "build.cancel", json!({ "run_id": run_id }));
    let stopped = result_of(&cancel)["cancelled"] == true;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let finished = loop {
        let st = rpc(&server, "build.status", json!({ "run_id": run_id }));
        if result_of(&st)["status"] != "running" {
            break st;
        }
        assert!(std::time::Instant::now() < deadline, "build did not finish");
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let status = result_of(&finished)["status"].clone();
    if stopped {
        assert_eq!(status, "cancelled", "{finished}");
    } else {
        assert_eq!(status, "passed", "{finished}");
    }
}

#[test]
fn imports_suggest_finds_declaring_modules_from_the_index() {
    let project = TempProject::new("imports-suggest", Some(WRITE_ON));
    project.write("Probe.lean", "theorem add_comm_probe : True := trivial\n");
    let server = Server::new(project.path()).unwrap();
    rpc(&server, "index.refresh", json!({}));

    let r = rpc(&server, "imports.suggest", json!({ "query": "add_comm_probe" }));
    let suggestions = result_of(&r)["suggestions"].as_array().unwrap();
    assert!(!suggestions.is_empty(), "{r}");
    assert_eq!(suggestions[0]["module"], "Probe", "{r}");

    let none = rpc(&server, "imports.suggest", json!({ "query": "  " }));
    assert!(none.get("error").is_some(), "{none}");
    let missing = rpc(&server, "imports.suggest", json!({}));
    assert_eq!(error_code(&missing), -32602, "{missing}");
    let big = rpc(&server, "imports.suggest", json!({ "query": "x", "limit": 500 }));
    assert_eq!(error_code(&big), -32602, "{big}");
}

#[test]
fn imports_unused_reports_imports_nothing_uses() {
    let project = TempProject::new("imports-unused", None);
    project.write("Helper.lean", "def helper : Nat := 1\n");
    project.write("Unused.lean", "import Helper\n\ndef answer : Nat := 42\n");
    let server = Server::new(project.path()).unwrap();
    let r = rpc(&server, "imports.unused", json!({ "path": "Unused.lean" }));
    let findings = result_of(&r)["findings"].as_array().unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f["code"] == "import_unused" && f["message"].as_str().unwrap_or("").contains("Helper")),
        "{r}"
    );

    let missing = rpc(&server, "imports.unused", json!({ "path": "Nope.lean" }));
    assert_eq!(error_code(&missing), -32602, "{missing}");
}

#[test]
fn check_project_builds_and_reports_the_ci_result() {
    if tpt_lean_adapter::discover_toolchain().lake.is_none() {
        eprintln!("skipping: lake not found");
        return;
    }
    let project = TempProject::new("check-project", None);
    project.write(
        "lakefile.lean",
        "import Lake\nopen Lake DSL\n\npackage demo\n\n@[default_target]\nlean_lib Demo\n",
    );
    project.write("Demo.lean", "def greeting : String := \"hi\"\n");
    let server = Server::new(project.path()).unwrap();
    let r = rpc(&server, "check.project", json!({ "timeout": 300 }));
    let report = result_of(&r);
    assert_eq!(report["ok"], true, "{r}");
    assert_eq!(report["build"]["status"], "passed", "{r}");
    assert!(report["diagnostics"].as_array().unwrap().is_empty(), "{r}");

    let bad = rpc(&server, "check.project", json!({ "timeout": 0 }));
    assert_eq!(error_code(&bad), -32602, "{bad}");
}

#[test]
fn diagnostics_explain_reports_the_error_and_goal_at_a_position() {
    if tpt_lean_adapter::discover_toolchain().lean.is_none() {
        eprintln!("skipping: lean not found");
        return;
    }
    let project = TempProject::new("explain", None);
    project.write("Bad.lean", "theorem bad_eq : 1 = 2 := rfl\n");
    let server = Server::new(project.path()).unwrap();
    let r = rpc(
        &server,
        "diagnostics.explain",
        json!({ "path": "Bad.lean", "line": 1, "column": 9 }),
    );
    let explained = result_of(&r);
    assert_eq!(explained["error"]["line"], 1, "{r}");
    assert!(
        explained["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("definitionally"),
        "{r}"
    );
    assert!(explained["suggestions"].is_array(), "{r}");
    assert!(explained["lean"]["compatibility"].is_string(), "{r}");

    let missing = rpc(
        &server,
        "diagnostics.explain",
        json!({ "path": "Nope.lean", "line": 1, "column": 1 }),
    );
    assert_eq!(error_code(&missing), -32602, "{missing}");
    let zero = rpc(
        &server,
        "diagnostics.explain",
        json!({ "path": "Bad.lean", "line": 0, "column": 1 }),
    );
    assert_eq!(error_code(&zero), -32602, "{zero}");
}

#[test]
fn context_pack_bundles_the_snippet_and_error_and_respects_the_budget() {
    if tpt_lean_adapter::discover_toolchain().lean.is_none() {
        eprintln!("skipping: lean not found");
        return;
    }
    let project = TempProject::new("context-pack", None);
    project.write("Bad.lean", "theorem bad_eq : 1 = 2 := rfl\n");
    let server = Server::new(project.path()).unwrap();
    let r = rpc(
        &server,
        "context.pack",
        json!({ "path": "Bad.lean", "line": 1, "column": 9 }),
    );
    let pack = result_of(&r);
    assert!(
        pack["snippet"].as_str().unwrap_or("").contains("theorem bad_eq"),
        "{r}"
    );
    assert!(!pack["diagnostics"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(pack["truncated"], false, "{r}");

    // A tiny budget trims optional parts, but the snippet is always kept.
    let tiny = rpc(
        &server,
        "context.pack",
        json!({ "path": "Bad.lean", "line": 1, "column": 9, "max_tokens": 1 }),
    );
    let trimmed = result_of(&tiny);
    assert_eq!(trimmed["truncated"], true, "{tiny}");
    assert!(
        trimmed["snippet"]
            .as_str()
            .unwrap_or("")
            .contains("theorem bad_eq"),
        "{tiny}"
    );

    let bad = rpc(
        &server,
        "context.pack",
        json!({ "path": "Bad.lean", "line": 1, "column": 9, "max_tokens": 0 }),
    );
    assert_eq!(error_code(&bad), -32602, "{bad}");
}

#[test]
fn project_doctor_reports_the_same_checks_as_the_cli() {
    let project = TempProject::new("project-doctor", None);
    let server = Server::new(project.path()).unwrap();
    let r = rpc(&server, "project.doctor", json!({}));
    let report = result_of(&r);
    let names: Vec<&str> = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    for expected in ["lean", "lake", "project", "config", "write", "index", "cache_dir"] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    let config = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "config")
        .unwrap();
    assert_eq!(config["status"], "ok", "{r}");
}

#[test]
fn search_goal_reads_the_goal_at_a_tactic_position() {
    if tpt_lean_adapter::discover_toolchain().lean.is_none() {
        eprintln!("skipping: lean not found");
        return;
    }
    let project = TempProject::new("search-goal", None);
    project.write(
        "Goal.lean",
        "theorem add_swap (a b : Nat) : a + b = b + a := by\n  skip\n",
    );
    let server = Server::new(project.path()).unwrap();
    let r = rpc(
        &server,
        "search.goal",
        json!({ "path": "Goal.lean", "line": 2, "column": 3 }),
    );
    let found = result_of(&r);
    assert!(found["goal"].as_str().unwrap_or("").contains("a + b"), "{r}");
    assert_eq!(found["indexed"], false, "{r}");
    assert!(found["results"].as_array().unwrap().is_empty(), "{r}");

    let bad = rpc(
        &server,
        "search.goal",
        json!({ "path": "Goal.lean", "line": 2, "column": 3, "limit": 0 }),
    );
    assert_eq!(error_code(&bad), -32602, "{bad}");
}

#[test]
fn warm_session_is_reused_and_follows_edits() {
    if tpt_lean_adapter::discover_toolchain().lean.is_none() {
        eprintln!("skipping: lean not found");
        return;
    }
    let project = TempProject::new("warm-session", None);
    project.write("Warm.lean", "theorem warm_eq : 1 = 2 := rfl\n");
    let server = Server::new(project.path()).unwrap();
    let params = json!({ "path": "Warm.lean", "line": 1, "column": 9 });

    let first = rpc(&server, "context.pack", params.clone());
    assert!(!result_of(&first)["diagnostics"].as_array().unwrap().is_empty(), "{first}");
    assert_eq!(server.warm_session_starts(), 1);

    // The same text again reuses the running server and gives the same answer.
    let second = rpc(&server, "context.pack", params.clone());
    assert!(!result_of(&second)["diagnostics"].as_array().unwrap().is_empty(), "{second}");
    assert_eq!(server.warm_session_starts(), 1, "the session is reused");

    // An edit that fixes the error must not report the old error.
    project.write("Warm.lean", "theorem warm_eq : 1 = 1 := rfl\n");
    let fixed = rpc(&server, "context.pack", params.clone());
    let diagnostics = result_of(&fixed)["diagnostics"].as_array().unwrap().clone();
    assert!(
        diagnostics.iter().all(|d| d["severity"] != "error"),
        "stale error after the edit: {fixed}"
    );
    assert_eq!(server.warm_session_starts(), 1);

    // Releasing the session stops the server; the next request starts another.
    server.release_warm_session();
    let again = rpc(&server, "context.pack", params);
    assert!(again.get("result").is_some(), "{again}");
    assert_eq!(server.warm_session_starts(), 2);
    server.release_warm_session();
}

#[test]
fn idle_warm_session_is_reaped() {
    if tpt_lean_adapter::discover_toolchain().lean.is_none() {
        eprintln!("skipping: lean not found");
        return;
    }
    let project = TempProject::new("warm-idle", None);
    project.write("Idle.lean", "theorem idle_ok : True := trivial\n");
    let server = Server::new(project.path()).unwrap();
    let params = json!({ "path": "Idle.lean", "line": 1, "column": 9 });
    let _ = rpc(&server, "context.pack", params.clone());
    assert_eq!(server.warm_session_starts(), 1);
    // Not idle yet: a long idle limit keeps it.
    assert!(!server.reap_idle_session(std::time::Duration::from_secs(3600)));
    // Idle for zero seconds: it is stopped, and the next request starts a fresh one.
    assert!(server.reap_idle_session(std::time::Duration::ZERO));
    let _ = rpc(&server, "context.pack", params);
    assert_eq!(server.warm_session_starts(), 2);
    server.release_warm_session();
}

/// One raw HTTP exchange with the server; returns the status code and the body.
fn http_exchange(port: u16, request: &str) -> (u16, String) {
    use std::io::Read as _;
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let status: u16 = raw.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

fn post(path: &str, host: &str, body: &str) -> String {
    format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

#[test]
fn http_transport_serves_rpc_and_refuses_other_hosts_and_methods() {
    let project = TempProject::new("http", None);
    let root = project.path().to_path_buf();
    let runner = {
        let root = root.clone();
        thread::spawn(move || state::run_foreground_with_http(&root, tpt_lean_server::Transport::Tcp(0), Some(0)))
    };
    let st = wait_for_state(&root);
    let http_port = st.http_port.expect("state records the HTTP port");
    let info = r#"{"jsonrpc":"2.0","id":7,"method":"project.info","params":{}}"#;

    let (status, body) = http_exchange(http_port, &post("/v1/rpc", "127.0.0.1", info));
    assert_eq!(status, 200, "{body}");
    let reply: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(reply["id"], 7);
    assert!(reply.get("result").is_some(), "{body}");

    let (status, _) = http_exchange(http_port, &post("/v1/rpc", "localhost", info));
    assert_eq!(status, 200);
    let (status, _) = http_exchange(http_port, &post("/v1/rpc", "evil.example", info));
    assert_eq!(status, 403, "a non-loopback Host is refused");

    let (status, _) = http_exchange(http_port, &post("/v1/other", "127.0.0.1", info));
    assert_eq!(status, 404);
    let get = format!("GET /v1/rpc HTTP/1.1\r\nHost: 127.0.0.1:{http_port}\r\n\r\n");
    let (status, _) = http_exchange(http_port, &get);
    assert_eq!(status, 405);

    let note = r#"{"jsonrpc":"2.0","method":"project.info","params":{}}"#;
    let (status, body) = http_exchange(http_port, &post("/v1/rpc", "127.0.0.1", note));
    assert_eq!(status, 204, "{body}");
    let (status, _) = http_exchange(http_port, &post("/v1/rpc", "127.0.0.1", "not json"));
    assert_eq!(status, 400);

    // Shutdown over the TCP transport also stops the HTTP listener.
    assert_eq!(state::stop(&root).unwrap().outcome, StopOutcome::Stopped);
    runner.join().unwrap().unwrap();
}

#[test]
fn build_events_stream_as_server_sent_events_and_end_with_done() {
    let project = TempProject::new("events", None);
    let root = project.path().to_path_buf();
    let runner = {
        let root = root.clone();
        thread::spawn(move || state::run_foreground_with_http(&root, tpt_lean_server::Transport::Tcp(0), Some(0)))
    };
    let st = wait_for_state(&root);
    let http_port = st.http_port.expect("HTTP port recorded");
    let run = format!(
        "POST /v1/rpc HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{}",
        r#"{"jsonrpc":"2.0","id":1,"method":"build.run","params":{}}"#.len(),
        r#"{"jsonrpc":"2.0","id":1,"method":"build.run","params":{}}"#
    );
    let (status, body) = http_exchange(http_port, &run);
    assert_eq!(status, 200, "{body}");

    // The stream follows the build and ends with `event: done`, which the server sends only after
    // the build has finished.
    let stream = "GET /v1/events HTTP/1.1
Host: 127.0.0.1

".to_string();
    let mut raw = String::new();
    {
        use std::io::Read as _;
        let mut conn = TcpStream::connect((Ipv4Addr::LOCALHOST, http_port)).unwrap();
        conn.set_read_timeout(Some(Duration::from_secs(120))).unwrap();
        conn.write_all(stream.as_bytes()).unwrap();
        conn.read_to_string(&mut raw).unwrap();
    }
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "{raw}");
    assert!(raw.contains("Content-Type: text/event-stream"), "{raw}");
    assert!(raw.contains("event: build_started"), "{raw}");
    assert!(raw.contains("event: done"), "{raw}");
    assert!(raw.contains("event: done
data: {"), "{raw}");

    let unknown = "GET /v1/events?run_id=nope HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
    let (status, _) = http_exchange(http_port, unknown);
    assert_eq!(status, 404);

    assert_eq!(state::stop(&root).unwrap().outcome, StopOutcome::Stopped);
    runner.join().unwrap().unwrap();
}
