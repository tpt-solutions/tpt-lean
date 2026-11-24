// SPDX-License-Identifier: MIT OR Apache-2.0
//! Persistent local index for Lean projects and their local dependencies (SQLite).
//!
//! The index is an acceleration layer. It stores syntax-level facts produced by
//! `tpt-lean-syntax` and never claims mathematical truth; Lean remains the source of truth.
//!
//! Concurrency: the database runs in WAL mode so readers keep working while a writer refreshes.

use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use tpt_lean_core::hash::sha256_hex;
use tpt_lean_core::{Error, ErrorCode, Result};

/// Version of the database schema. Bump with a migration step in [`migrate`].
pub const SCHEMA_VERSION: i64 = 1;

/// Directory names never descended into during a scan.
const SKIP_DIRS: &[&str] = &[".lake", ".git", ".tpt-lean", "build", "target", "node_modules"];

/// A symbol as stored in the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRow {
    pub id: i64,
    pub package: String,
    pub file: String,
    pub name: String,
    pub short_name: Option<String>,
    pub kind: String,
    pub namespace: String,
    pub line: i64,
    pub docstring: Option<String>,
    pub is_private: bool,
}

/// Summary returned by [`Index::refresh`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshStats {
    pub scanned: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub removed: usize,
}

/// Status summary returned by [`Index::status`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub schema_version: i64,
    pub files: i64,
    pub symbols: i64,
    pub imports: i64,
    pub packages: Vec<String>,
    pub integrity_ok: bool,
}

/// A handle to an open index database.
pub struct Index {
    conn: Connection,
    path: PathBuf,
}

/// A source root to scan, labelled with the package it belongs to.
#[derive(Debug, Clone)]
pub struct ScanRoot {
    pub package: String,
    pub path: PathBuf,
}

impl Index {
    /// Open (or create) the index at `path` and migrate it to the current schema.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::from(e).with_path(parent))?;
        }
        let conn = Connection::open(path).map_err(|e| db_err(e, path))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| db_err(e, path))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| db_err(e, path))?;
        let mut index = Self {
            conn,
            path: path.to_path_buf(),
        };
        index.migrate()?;
        Ok(index)
    }

    /// Open an in-memory index (tests and one-shot queries).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(|e| db_err(e, Path::new(":memory:")))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| db_err(e, Path::new(":memory:")))?;
        let mut index = Self {
            conn,
            path: PathBuf::from(":memory:"),
        };
        index.migrate()?;
        Ok(index)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| db_err(e, &self.path))?;
        if version > SCHEMA_VERSION {
            return Err(Error::new(
                ErrorCode::IndexError,
                format!("index schema {version} is newer than supported {SCHEMA_VERSION}; upgrade tpt-lean"),
            )
            .with_path(&self.path));
        }
        if version < 1 {
            self.conn
                .execute_batch(
                    "BEGIN;
                    CREATE TABLE packages (
                        id INTEGER PRIMARY KEY,
                        name TEXT NOT NULL UNIQUE,
                        root TEXT NOT NULL
                    );
                    CREATE TABLE files (
                        id INTEGER PRIMARY KEY,
                        package_id INTEGER NOT NULL REFERENCES packages(id) ON DELETE CASCADE,
                        rel_path TEXT NOT NULL,
                        hash TEXT NOT NULL,
                        size INTEGER NOT NULL,
                        indexed_at INTEGER NOT NULL,
                        UNIQUE(package_id, rel_path)
                    );
                    CREATE TABLE symbols (
                        id INTEGER PRIMARY KEY,
                        file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                        name TEXT NOT NULL,
                        short_name TEXT,
                        kind TEXT NOT NULL,
                        namespace TEXT NOT NULL,
                        line INTEGER NOT NULL,
                        is_private INTEGER NOT NULL,
                        docstring TEXT
                    );
                    CREATE INDEX symbols_name ON symbols(name);
                    CREATE INDEX symbols_short ON symbols(short_name);
                    CREATE INDEX symbols_file ON symbols(file_id);
                    CREATE TABLE imports (
                        file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                        module TEXT NOT NULL,
                        line INTEGER NOT NULL
                    );
                    CREATE INDEX imports_module ON imports(module);
                    CREATE INDEX imports_file ON imports(file_id);
                    CREATE VIRTUAL TABLE symbol_text USING fts5(
                        name, docstring, tokenize='unicode61'
                    );
                    CREATE TABLE diagnostics (
                        id INTEGER PRIMARY KEY,
                        file_id INTEGER REFERENCES files(id) ON DELETE CASCADE,
                        severity TEXT NOT NULL,
                        line INTEGER NOT NULL,
                        column INTEGER NOT NULL,
                        message TEXT NOT NULL,
                        recorded_at INTEGER NOT NULL
                    );
                    CREATE TABLE build_runs (
                        id INTEGER PRIMARY KEY,
                        started_at INTEGER NOT NULL,
                        finished_at INTEGER,
                        status TEXT NOT NULL
                    );
                    CREATE TABLE profile_runs (
                        id INTEGER PRIMARY KEY,
                        file_id INTEGER REFERENCES files(id) ON DELETE SET NULL,
                        started_at INTEGER NOT NULL,
                        duration_ms INTEGER NOT NULL,
                        report TEXT NOT NULL
                    );
                    CREATE TABLE cache_entries (
                        key TEXT PRIMARY KEY,
                        value TEXT NOT NULL,
                        created_at INTEGER NOT NULL
                    );
                    PRAGMA user_version = 1;
                    COMMIT;",
                )
                .map_err(|e| db_err(e, &self.path))?;
        }
        Ok(())
    }

    /// Scan the given roots and bring the index up to date. Unchanged files (same SHA-256) are skipped.
    pub fn refresh(&mut self, roots: &[ScanRoot]) -> Result<RefreshStats> {
        let mut stats = RefreshStats::default();
        let mut seen: HashMap<String, BTreeSet<String>> = HashMap::new();

        for root in roots {
            let package_id = self.package_id(&root.package, &root.path)?;
            let files = collect_lean_files(&root.path)?;
            let known_paths = self.known_files(package_id)?;
            for (rel, abs) in files {
                stats.scanned += 1;
                seen.entry(root.package.clone()).or_default().insert(rel.clone());
                let bytes = std::fs::read(&abs).map_err(|e| Error::from(e).with_path(&abs))?;
                let hash = sha256_hex(&bytes);
                if known_paths.get(&rel).map(|h| h == &hash).unwrap_or(false) {
                    stats.unchanged += 1;
                    continue;
                }
                let text = String::from_utf8_lossy(&bytes).into_owned();
                self.store_file(package_id, &rel, &hash, bytes.len(), &text)?;
                stats.updated += 1;
            }
            // Remove rows for files that no longer exist under this package.
            let present = seen.get(&root.package).cloned().unwrap_or_default();
            for rel in known_paths.keys() {
                if !present.contains(rel) {
                    self.delete_file(package_id, rel)?;
                    stats.removed += 1;
                }
            }
        }
        Ok(stats)
    }

    fn package_id(&self, name: &str, root: &Path) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO packages(name, root) VALUES (?1, ?2)
                 ON CONFLICT(name) DO UPDATE SET root = excluded.root",
                params![name, root.display().to_string()],
            )
            .map_err(|e| db_err(e, &self.path))?;
        self.conn
            .query_row("SELECT id FROM packages WHERE name = ?1", params![name], |r| {
                r.get(0)
            })
            .map_err(|e| db_err(e, &self.path))
    }

    fn known_files(&self, package_id: i64) -> Result<HashMap<String, String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT rel_path, hash FROM files WHERE package_id = ?1")
            .map_err(|e| db_err(e, &self.path))?;
        let rows = stmt
            .query_map(params![package_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| db_err(e, &self.path))?;
        rows.collect::<rusqlite::Result<HashMap<_, _>>>()
            .map_err(|e| db_err(e, &self.path))
    }

    fn delete_file(&mut self, package_id: i64, rel: &str) -> Result<()> {
        let tx = self.conn.transaction().map_err(|e| db_err(e, &self.path))?;
        let file_id: Option<i64> = tx
            .query_row(
                "SELECT id FROM files WHERE package_id = ?1 AND rel_path = ?2",
                params![package_id, rel],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| db_err(e, &self.path))?;
        if let Some(id) = file_id {
            delete_file_rows(&tx, id).map_err(|e| db_err(e, &self.path))?;
            tx.execute("DELETE FROM files WHERE id = ?1", params![id])
                .map_err(|e| db_err(e, &self.path))?;
        }
        tx.commit().map_err(|e| db_err(e, &self.path))
    }

    fn store_file(&mut self, package_id: i64, rel: &str, hash: &str, size: usize, text: &str) -> Result<()> {
        let parse = tpt_lean_syntax::parse(text);
        let now = now_ms();
        let tx = self.conn.transaction().map_err(|e| db_err(e, &self.path))?;
        let map = |e| db_err(e, Path::new("index"));
        tx.execute(
            "INSERT INTO files(package_id, rel_path, hash, size, indexed_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(package_id, rel_path) DO UPDATE SET hash = excluded.hash, size = excluded.size, indexed_at = excluded.indexed_at",
            params![package_id, rel, hash, size as i64, now],
        )
        .map_err(map)?;
        let file_id: i64 = tx
            .query_row(
                "SELECT id FROM files WHERE package_id = ?1 AND rel_path = ?2",
                params![package_id, rel],
                |r| r.get(0),
            )
            .map_err(map)?;
        delete_file_rows(&tx, file_id).map_err(map)?;
        for imp in &parse.imports {
            tx.execute(
                "INSERT INTO imports(file_id, module, line) VALUES (?1, ?2, ?3)",
                params![file_id, imp.module, imp.line as i64],
            )
            .map_err(map)?;
        }
        for sym in &parse.symbols {
            tx.execute(
                "INSERT INTO symbols(file_id, name, short_name, kind, namespace, line, is_private, docstring)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    file_id,
                    sym.name,
                    sym.short_name,
                    sym.kind,
                    sym.namespace,
                    sym.line as i64,
                    sym.is_private,
                    sym.docstring
                ],
            )
            .map_err(map)?;
            let sym_id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO symbol_text(rowid, name, docstring) VALUES (?1, ?2, ?3)",
                params![sym_id, sym.name, sym.docstring.clone().unwrap_or_default()],
            )
            .map_err(map)?;
        }
        tx.commit().map_err(map)
    }

    /// Exact-name, prefix, or short-name lookup, optionally restricted to a namespace prefix.
    pub fn symbols_named(&self, query: &str, limit: usize) -> Result<Vec<SymbolRow>> {
        let pattern = format!("{}%", escape_like(query));
        let mut stmt = self
            .conn
            .prepare(
                "SELECT s.id, p.name, f.rel_path, s.name, s.short_name, s.kind, s.namespace, s.line, s.docstring, s.is_private
                 FROM symbols s JOIN files f ON f.id = s.file_id JOIN packages p ON p.id = f.package_id
                 WHERE s.name LIKE ?1 ESCAPE '\\' OR s.short_name LIKE ?1 ESCAPE '\\'
                 ORDER BY (s.name = ?2) DESC, length(s.name), s.name
                 LIMIT ?3",
            )
            .map_err(|e| db_err(e, &self.path))?;
        let rows = stmt
            .query_map(params![pattern, query, limit as i64], row_to_symbol)
            .map_err(|e| db_err(e, &self.path))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| db_err(e, &self.path))
    }

    /// Full-text search over names and docstrings (FTS5). `query` is passed as an FTS5 expression.
    /// Callers should sanitize user text first; see [`fts_query`].
    pub fn symbols_fts(&self, fts: &str, limit: usize) -> Result<Vec<SymbolRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT s.id, p.name, f.rel_path, s.name, s.short_name, s.kind, s.namespace, s.line, s.docstring, s.is_private
                 FROM symbol_text t
                 JOIN symbols s ON s.id = t.rowid JOIN files f ON f.id = s.file_id JOIN packages p ON p.id = f.package_id
                 WHERE symbol_text MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )
            .map_err(|e| db_err(e, &self.path))?;
        let rows = stmt
            .query_map(params![fts, limit as i64], row_to_symbol)
            .map_err(|e| db_err(e, &self.path))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| db_err(e, &self.path))
    }

    /// Modules imported by any file in the index that match `module`.
    pub fn importers_of(&self, module: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT f.rel_path FROM imports i JOIN files f ON f.id = i.file_id WHERE i.module = ?1 ORDER BY f.rel_path",
            )
            .map_err(|e| db_err(e, &self.path))?;
        let rows = stmt
            .query_map(params![module], |r| r.get::<_, String>(0))
            .map_err(|e| db_err(e, &self.path))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| db_err(e, &self.path))
    }

    /// Health and size summary (used by `tpt-lean index status` and `doctor`).
    pub fn status(&self) -> Result<Status> {
        let count = |sql: &str| -> Result<i64> {
            self.conn
                .query_row(sql, [], |r| r.get(0))
                .map_err(|e| db_err(e, &self.path))
        };
        let version = count("PRAGMA user_version")?;
        let integrity: String = self
            .conn
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(|e| db_err(e, &self.path))?;
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM packages ORDER BY name")
            .map_err(|e| db_err(e, &self.path))?;
        let packages = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| db_err(e, &self.path))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| db_err(e, &self.path))?;
        Ok(Status {
            schema_version: version,
            files: count("SELECT COUNT(*) FROM files")?,
            symbols: count("SELECT COUNT(*) FROM symbols")?,
            imports: count("SELECT COUNT(*) FROM imports")?,
            packages,
            integrity_ok: integrity == "ok",
        })
    }

    /// Delete every row and reclaim space. The schema is kept.
    pub fn clear(&mut self) -> Result<()> {
        self.conn
            .execute_batch(
                "DELETE FROM symbol_text; DELETE FROM imports; DELETE FROM symbols; DELETE FROM files; DELETE FROM packages; VACUUM;",
            )
            .map_err(|e| db_err(e, &self.path))
    }
}

fn delete_file_rows(tx: &rusqlite::Transaction<'_>, file_id: i64) -> rusqlite::Result<()> {
    let mut stmt = tx.prepare("SELECT id FROM symbols WHERE file_id = ?1")?;
    let ids: Vec<i64> = stmt
        .query_map(params![file_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in ids {
        tx.execute("DELETE FROM symbol_text WHERE rowid = ?1", params![id])?;
    }
    tx.execute("DELETE FROM symbols WHERE file_id = ?1", params![file_id])?;
    tx.execute("DELETE FROM imports WHERE file_id = ?1", params![file_id])?;
    Ok(())
}

fn row_to_symbol(r: &rusqlite::Row<'_>) -> rusqlite::Result<SymbolRow> {
    Ok(SymbolRow {
        id: r.get(0)?,
        package: r.get(1)?,
        file: r.get(2)?,
        name: r.get(3)?,
        short_name: r.get(4)?,
        kind: r.get(5)?,
        namespace: r.get(6)?,
        line: r.get(7)?,
        docstring: r.get(8)?,
        is_private: r.get(9)?,
    })
}

/// Walk `root` for `.lean` files, skipping [`SKIP_DIRS`] and never following symlinks.
/// Returns `(relative_path_with_forward_slashes, absolute_path)` pairs, sorted.
pub fn collect_lean_files(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| Error::from(e).with_path(&dir))?;
        for entry in entries {
            let entry = entry.map_err(|e| Error::from(e).with_path(&dir))?;
            let file_type = entry
                .file_type()
                .map_err(|e| Error::from(e).with_path(entry.path()))?;
            let path = entry.path();
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                let name = entry.file_name();
                if !SKIP_DIRS.iter().any(|s| name == *s) {
                    stack.push(path);
                }
            } else if file_type.is_file() && path.extension().is_some_and(|e| e == "lean") {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, path));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Discover scan roots for a project: the project itself plus every package under `.lake/packages`.
pub fn default_scan_roots(project_root: &Path, include_packages: bool) -> Vec<ScanRoot> {
    let mut roots = vec![ScanRoot {
        package: "project".into(),
        path: project_root.to_path_buf(),
    }];
    if include_packages {
        if let Ok(entries) = std::fs::read_dir(project_root.join(".lake").join("packages")) {
            let mut pkgs: Vec<ScanRoot> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| ScanRoot {
                    package: e.file_name().to_string_lossy().into_owned(),
                    path: e.path(),
                })
                .collect();
            pkgs.sort_by(|a, b| a.package.cmp(&b.package));
            roots.extend(pkgs);
        }
    }
    roots
}

/// Escape `%`, `_` and `\` for use in a LIKE pattern with `ESCAPE '\'`.
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Turn free text into a safe FTS5 query: each word becomes a quoted prefix term joined by AND.
pub fn fts_query(text: &str) -> Option<String> {
    let terms: Vec<String> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '\''))
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"*", t.replace('"', "")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

fn db_err(e: rusqlite::Error, path: &Path) -> Error {
    Error::new(ErrorCode::IndexError, e.to_string()).with_path(path)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tpt-index-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Sub")).unwrap();
        std::fs::write(dir.join("Main.lean"), "import Std\n\nnamespace Geo\n/-- Compact sets are closed. -/\ntheorem compact_closed : True := trivial\nend Geo\n").unwrap();
        std::fs::write(dir.join("Sub").join("Other.lean"), "def helper := 1\n").unwrap();
        std::fs::create_dir_all(dir.join(".lake").join("packages").join("dep")).unwrap();
        std::fs::write(
            dir.join(".lake").join("packages").join("dep").join("X.lean"),
            "theorem hidden : True := trivial\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn scan_skips_lake_build_dirs_by_default() {
        let dir = temp_project("scan");
        let files = collect_lean_files(&dir).unwrap();
        let names: Vec<&str> = files.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(names, vec!["Main.lean", "Sub/Other.lean"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_is_incremental_and_searchable() {
        let dir = temp_project("refresh");
        let mut idx = Index::open_in_memory().unwrap();
        let roots = default_scan_roots(&dir, true);
        let first = idx.refresh(&roots).unwrap();
        assert_eq!(first.updated, 3);
        let second = idx.refresh(&roots).unwrap();
        assert_eq!(second.updated, 0);
        assert_eq!(second.unchanged, 3);

        let hits = idx.symbols_named("Geo.compact", 10).unwrap();
        assert_eq!(hits[0].name, "Geo.compact_closed");
        assert_eq!(hits[0].package, "project");

        let fts = idx
            .symbols_fts(&fts_query("compact closed").unwrap(), 10)
            .unwrap();
        assert!(fts.iter().any(|s| s.name == "Geo.compact_closed"));

        let st = idx.status().unwrap();
        assert!(st.integrity_ok);
        assert_eq!(st.packages, vec!["dep".to_string(), "project".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removed_files_are_dropped() {
        let dir = temp_project("removed");
        let mut idx = Index::open_in_memory().unwrap();
        let roots = vec![ScanRoot {
            package: "project".into(),
            path: dir.clone(),
        }];
        idx.refresh(&roots).unwrap();
        std::fs::remove_file(dir.join("Sub").join("Other.lean")).unwrap();
        let stats = idx.refresh(&roots).unwrap();
        assert_eq!(stats.removed, 1);
        assert!(idx.symbols_named("helper", 5).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn like_wildcards_are_literal() {
        assert_eq!(escape_like("a_b%"), "a\\_b\\%");
    }

    #[test]
    fn fts_query_is_sanitized() {
        assert_eq!(fts_query("a\"b (c)").unwrap(), "\"a\"* AND \"b\"* AND \"c\"*");
        assert!(fts_query("!!!").is_none());
    }

    #[test]
    fn migration_is_recorded() {
        let idx = Index::open_in_memory().unwrap();
        assert_eq!(idx.status().unwrap().schema_version, SCHEMA_VERSION);
    }
}
