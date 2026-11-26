// SPDX-License-Identifier: MIT OR Apache-2.0
//! `tpt-lean setup`: write editor language-server entries and MCP client entries for a project.
//!
//! Two kinds of file are written. Files a person may already have (`.mcp.json`,
//! `.cursor/mcp.json`, `.helix/languages.toml`) are merged: existing content is kept, and an
//! existing `tpt-lean` entry is never overwritten. Files that belong to this tool
//! (`.vscode/tpt-lean-lsp.json`, `.tpt-lean/editors/*`) are created when absent and left alone
//! when they differ. Every change is shown as a diff before it is written; `--dry-run` writes
//! nothing.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};
use tpt_lean_core::{Error, Result};

use crate::args::Args;
use crate::{Ctx, Mode};

use super::{project_root, OK};

/// Editors `setup --editor` knows.
const EDITORS: &[&str] = &["vscode", "neovim", "helix", "emacs"];
/// MCP clients `setup --mcp` knows.
const MCP_CLIENTS: &[&str] = &["claude", "cursor"];
/// Name of the server entry, in every client's config.
const SERVER_NAME: &str = "tpt-lean";
/// Exit status when `--dry-run` would change something. Lets a CI job check for drift.
const WOULD_CHANGE: i32 = 1;

/// One file the setup would write.
struct Change {
    /// Project-relative path with forward slashes.
    path: String,
    action: Action,
    /// The text to write (the whole file after the change).
    new_text: String,
    /// Human notes, such as a skipped entry or a file that was kept as it is.
    notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Create,
    Update,
    Unchanged,
}

#[derive(Serialize)]
struct ChangeReport {
    path: String,
    action: Action,
    notes: Vec<String>,
}

/// `setup [--editor NAME]... [--mcp NAME]... [--dry-run]`.
pub fn setup_cmd(ctx: &Ctx, args: &Args) -> Result<i32> {
    args.reject_unknown(
        &["--editor", "--mcp", "--dry-run", "--project"],
        &["--editor", "--mcp", "--project"],
    )?;
    let editors = args.values("--editor");
    let clients = args.values("--mcp");
    if editors.is_empty() && clients.is_empty() {
        return Err(Error::invalid(format!(
            "choose what to set up: --editor {} and/or --mcp {}",
            EDITORS.join("|"),
            MCP_CLIENTS.join("|")
        )));
    }
    for e in &editors {
        if !EDITORS.contains(&e.as_str()) {
            return Err(Error::invalid(format!(
                "unknown editor `{e}`; use one of: {}",
                EDITORS.join(", ")
            )));
        }
    }
    for c in &clients {
        if !MCP_CLIENTS.contains(&c.as_str()) {
            return Err(Error::invalid(format!(
                "unknown MCP client `{c}`; use one of: {}",
                MCP_CLIENTS.join(", ")
            )));
        }
    }
    let root = project_root(args)?;
    let dry_run = args.flag("--dry-run");

    let mut changes = Vec::new();
    for e in &editors {
        changes.push(editor_change(&root, e)?);
    }
    for c in &clients {
        changes.push(mcp_change(&root, c)?);
    }

    let reports: Vec<ChangeReport> = changes
        .iter()
        .map(|c| ChangeReport {
            path: c.path.clone(),
            action: c.action,
            notes: c.notes.clone(),
        })
        .collect();
    let mut human = Vec::new();
    for c in &changes {
        human.push(match c.action {
            Action::Create => format!("create {}", c.path),
            Action::Update => format!("update {}", c.path),
            Action::Unchanged => format!("unchanged {}", c.path),
        });
        if c.action != Action::Unchanged {
            human.extend(diff_lines(&read_existing(&root, &c.path)?, &c.new_text));
        }
        human.extend(c.notes.iter().map(|n| format!("  note: {n}")));
    }
    let writes = changes.iter().any(|c| c.action != Action::Unchanged);
    if dry_run {
        human.push(if writes {
            "dry run: nothing was written".to_string()
        } else {
            "dry run: nothing to change".to_string()
        });
    }
    if ctx.mode == Mode::Human {
        // Items are printed as text lines; the reports go to JSON only.
        for line in &human {
            println!("{line}");
        }
    } else {
        ctx.emit_items("setup", &[], &reports)?;
    }

    if dry_run {
        return Ok(if writes { WOULD_CHANGE } else { OK });
    }
    for c in &changes {
        if c.action == Action::Unchanged {
            continue;
        }
        let target = root.join(&c.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::from(e).with_path(parent))?;
        }
        std::fs::write(&target, &c.new_text).map_err(|e| Error::from(e).with_path(&target))?;
    }
    Ok(OK)
}

/// The file for an editor, or the reason it is already set up.
fn editor_change(root: &Path, editor: &str) -> Result<Change> {
    match editor {
        "helix" => {
            let path = ".helix/languages.toml";
            let existing = read_existing(root, path)?;
            let (new_text, notes) = merge_helix(&existing);
            Ok(merge_change(path, &existing, new_text, notes))
        }
        "vscode" => owned_change(root, ".vscode/tpt-lean-lsp.json", VSCODE_CONFIG),
        "neovim" => owned_change(root, ".tpt-lean/editors/neovim.lua", NEOVIM_CONFIG),
        "emacs" => owned_change(root, ".tpt-lean/editors/emacs.el", EMACS_CONFIG),
        other => Err(Error::invalid(format!("unknown editor `{other}`"))),
    }
}

/// The MCP client's config file, with the `tpt-lean` server entry merged in.
fn mcp_change(root: &Path, client: &str) -> Result<Change> {
    let path = match client {
        "claude" => ".mcp.json",
        "cursor" => ".cursor/mcp.json",
        other => return Err(Error::invalid(format!("unknown MCP client `{other}`"))),
    };
    let existing = read_existing(root, path)?;
    let mut doc: Value = if existing.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&existing)
            .map_err(|e| Error::invalid(format!("{path} is not valid JSON ({e}); fix or remove it first")))?
    };
    let entry = json!({
        "command": "tpt-lean",
        "args": ["mcp", "stdio", "--project", "."]
    });
    let servers = doc
        .as_object_mut()
        .ok_or_else(|| Error::invalid(format!("{path} must hold a JSON object")))?
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| Error::invalid(format!("`mcpServers` in {path} must be an object")))?;
    let mut notes = Vec::new();
    match servers.get(SERVER_NAME) {
        Some(current) if *current == entry => {}
        Some(_) => notes.push(format!(
            "{path} already has a `{SERVER_NAME}` entry that differs; it was kept as it is"
        )),
        None => {
            servers.insert(SERVER_NAME.to_string(), entry);
        }
    }
    let new_text = format!("{}\n", serde_json::to_string_pretty(&doc).unwrap_or_default());
    // A differing entry is kept, so nothing changes in that case.
    let new_text = if notes.is_empty() {
        new_text
    } else {
        existing.clone()
    };
    Ok(merge_change(path, &existing, new_text, notes))
}

/// A file that belongs to this tool: created when absent, kept as it is when different.
fn owned_change(root: &Path, path: &str, body: &str) -> Result<Change> {
    let existing = read_existing(root, path)?;
    if existing.is_empty() {
        return Ok(Change {
            path: path.to_string(),
            action: Action::Create,
            new_text: body.to_string(),
            notes: Vec::new(),
        });
    }
    if existing == body {
        return Ok(Change {
            path: path.to_string(),
            action: Action::Unchanged,
            new_text: existing,
            notes: Vec::new(),
        });
    }
    Ok(Change {
        path: path.to_string(),
        action: Action::Unchanged,
        new_text: existing,
        notes: vec![format!(
            "{path} exists and differs from the template; it was left as it is"
        )],
    })
}

/// Turn a merged text into a change: nothing when the text is the same, else create or update.
fn merge_change(path: &str, existing: &str, new_text: String, notes: Vec<String>) -> Change {
    let action = if new_text == existing {
        Action::Unchanged
    } else if existing.is_empty() {
        Action::Create
    } else {
        Action::Update
    };
    Change {
        path: path.to_string(),
        action,
        new_text,
        notes,
    }
}

/// Add the tpt-lean server and, when there is none, a `lean` language entry that uses it.
/// Existing sections are never rewritten.
fn merge_helix(existing: &str) -> (String, Vec<String>) {
    let mut out = existing.to_string();
    let mut notes = Vec::new();
    // Judged on the existing text only: the server section written below also names tpt-lean.
    let lean_lists_server = existing
        .lines()
        .any(|l| l.contains("language-servers") && l.contains("\"tpt-lean\""));
    if !out.contains("[language-server.tpt-lean]") {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(HELIX_SERVER);
    }
    if out.contains("name = \"lean\"") {
        if !lean_lists_server {
            notes.push(
                "add \"tpt-lean\" to the language-servers list of your existing `lean` language entry"
                    .to_string(),
            );
        }
    } else {
        out.push('\n');
        out.push_str(HELIX_LANGUAGE);
    }
    (out, notes)
}

const HELIX_SERVER: &str = "[language-server.tpt-lean]
command = \"tpt-lean\"
args = [\"lsp\", \"--project\", \".\"]
";

const HELIX_LANGUAGE: &str = "[[language]]
name = \"lean\"
language-servers = [\"tpt-lean\"]
";

const VSCODE_CONFIG: &str = "{
  \"command\": \"tpt-lean\",
  \"args\": [\"lsp\", \"--project\", \"${workspaceFolder}\"],
  \"languageId\": \"lean\",
  \"filetypes\": [\"lean\"],
  \"rootMarkers\": [\"lakefile.lean\", \"lakefile.toml\"]
}
";

const NEOVIM_CONFIG: &str =
    "-- tpt-lean language server (see docs/editors.md). Source this file from your init.lua.
vim.lsp.config(\"tpt_lean\", {
  cmd = { \"tpt-lean\", \"lsp\", \"--project\", \".\" },
  filetypes = { \"lean\" },
  root_markers = { \"lakefile.lean\", \"lakefile.toml\" },
})
vim.lsp.enable(\"tpt_lean\")
";

const EMACS_CONFIG: &str =
    ";; tpt-lean language server (see docs/editors.md). Load this file from your init.el.
(add-to-list 'eglot-server-programs
             '(lean4-mode . (\"tpt-lean\" \"lsp\" \"--project\" \".\")))
";

/// The current text of a project file, or empty when it does not exist.
fn read_existing(root: &Path, rel: &str) -> Result<String> {
    let path: PathBuf = root.join(rel);
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(Error::from(e).with_path(&path)),
    }
}

/// A line diff in the form `+ line` / `- line`, with unchanged lines omitted. Lines come from a
/// longest-common-subsequence walk, so a moved line shows as a removal and an insertion.
fn diff_lines(old: &str, new: &str) -> Vec<String> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let (n, m) = (a.len(), b.len());
    // lcs[i][j] is the LCS length of a[i..] and b[j..].
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            out.push(format!("+ {}", b[j]));
            j += 1;
        } else {
            out.push(format!("- {}", a[i]));
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_entry_is_added_and_existing_servers_are_kept() {
        let dir = std::env::temp_dir().join(format!("tpt-setup-mcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".mcp.json"),
            "{\"mcpServers\":{\"other\":{\"command\":\"x\",\"args\":[]}}}",
        )
        .unwrap();
        let c = mcp_change(&dir, "claude").unwrap();
        assert_eq!(c.action, Action::Update);
        let v: Value = serde_json::from_str(&c.new_text).unwrap();
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert_eq!(v["mcpServers"]["tpt-lean"]["command"], "tpt-lean");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_differing_tpt_lean_entry_is_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("tpt-setup-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let original = "{\"mcpServers\":{\"tpt-lean\":{\"command\":\"/opt/mine\",\"args\":[]}}}";
        std::fs::write(dir.join(".mcp.json"), original).unwrap();
        let c = mcp_change(&dir, "claude").unwrap();
        assert_eq!(c.action, Action::Unchanged);
        assert_eq!(c.new_text, original);
        assert_eq!(c.notes.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_json_is_an_error_not_an_overwrite() {
        let dir = std::env::temp_dir().join(format!("tpt-setup-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".mcp.json"), "{ not json").unwrap();
        assert!(mcp_change(&dir, "claude").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn helix_keeps_existing_sections() {
        let (text, notes) = merge_helix("[editor]\nline-number = \"relative\"\n");
        assert!(text.starts_with("[editor]"));
        assert!(text.contains("[language-server.tpt-lean]"));
        assert!(text.contains("name = \"lean\""));
        assert!(notes.is_empty());
        // Running it again changes nothing.
        let (again, _) = merge_helix(&text);
        assert_eq!(again, text);
    }

    #[test]
    fn helix_with_an_existing_lean_entry_asks_for_the_server_name() {
        let existing = "[[language]]\nname = \"lean\"\nlanguage-servers = [\"other\"]\n";
        let (_, notes) = merge_helix(existing);
        assert_eq!(notes.len(), 1);
    }

    #[test]
    fn diff_marks_added_and_removed_lines() {
        let d = diff_lines("a\nb\nc\n", "a\nx\nc\n");
        assert!(d.contains(&"- b".to_string()));
        assert!(d.contains(&"+ x".to_string()));
        assert!(!d.iter().any(|l| l == "  a"));
    }
}
