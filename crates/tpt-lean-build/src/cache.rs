// SPDX-License-Identifier: MIT OR Apache-2.0
//! Cache layers: an in-process memory map and a disk record store under the global cache dir.
//!
//! Records describe successful builds (`digest`, project, time, duration). They never replace
//! Lake's own outputs: a disk hit means "these exact inputs built successfully before", and Lake
//! still decides what needs rebuilding. The memory layer is used by watch mode to skip a rebuild
//! when a polled change leads back to a key that already built successfully in this process.
//!
//! Shared layers: directories listed in `TPT_LEAN_SHARED_CACHE` (separated like `PATH`) hold records
//! written by other machines, such as a team share or a CI cache restored before a build. A lookup
//! checks the local layer, then each shared layer in order; a shared hit is copied into the local
//! layer. Shared records match on the digest alone, because the project path differs between
//! machines. `export` and `import` move records between directories, for CI artifacts.

use crate::events::CacheLayer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tpt_lean_core::dirs::global_cache_dir;
use tpt_lean_core::{Error, Result};

/// Environment variable naming shared record directories, separated like `PATH`.
pub const SHARED_ENV: &str = "TPT_LEAN_SHARED_CACHE";

/// The shared layers named by [`SHARED_ENV`], in order. Empty when the variable is unset.
pub fn shared_layers() -> Vec<DiskCache> {
    std::env::var_os(SHARED_ENV)
        .map(|v| std::env::split_paths(&v).map(DiskCache::new).collect())
        .unwrap_or_default()
}

/// Find a successful record for `digest`: the local layer first (matching the project root too),
/// then each shared layer (matching the digest only). A shared hit is copied into the local layer.
/// Returns the layer that answered.
pub fn lookup_layers(
    local: Option<&DiskCache>,
    shared: &[DiskCache],
    digest: &str,
    project_root: &str,
) -> Result<Option<(CacheLayer, BuildRecord)>> {
    if let Some(local) = local {
        if let Some(record) = local.lookup(digest, project_root)? {
            return Ok(Some((CacheLayer::Disk, record)));
        }
    }
    for layer in shared {
        if let Some(record) = layer.lookup_digest(digest)? {
            if let Some(local) = local {
                // The local copy describes this project, so it carries this project's root. Best
                // effort: a failed copy only means the next lookup goes to the shared layer.
                let mut copy = record.clone();
                copy.project_root = project_root.to_string();
                let _ = local.store(&copy);
            }
            return Ok(Some((CacheLayer::Shared, record)));
        }
    }
    Ok(None)
}

/// Record a successful build in the local layer and in every shared layer. Failures in shared
/// layers are returned as messages instead of errors, so an unavailable share does not fail a
/// build.
pub fn store_layers(
    local: Option<&DiskCache>,
    shared: &[DiskCache],
    record: &BuildRecord,
) -> Result<Vec<String>> {
    if let Some(local) = local {
        local.store(record)?;
    }
    let mut warnings = Vec::new();
    for layer in shared {
        if let Err(e) = layer.store(record) {
            warnings.push(format!(
                "could not record build in shared cache {}: {}",
                layer.root().display(),
                e.message()
            ));
        }
    }
    Ok(warnings)
}

/// Copy every valid record from `local` into `dest`. Returns how many were copied.
pub fn export_records(local: &DiskCache, dest: &Path) -> Result<usize> {
    std::fs::create_dir_all(dest).map_err(|e| Error::from(e).with_path(dest))?;
    let mut copied = 0;
    for record in read_records(local.root())? {
        DiskCache::new(dest).store(&record)?;
        copied += 1;
    }
    Ok(copied)
}

/// Copy every valid record from `src` into `local`. Invalid files are skipped. Returns how many
/// were copied.
pub fn import_records(src: &Path, local: &DiskCache) -> Result<usize> {
    let mut copied = 0;
    for record in read_records(src)? {
        local.store(&record)?;
        copied += 1;
    }
    Ok(copied)
}

/// Every valid record in `dir`. A missing directory has no records.
fn read_records(dir: &Path) -> Result<Vec<BuildRecord>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::from(e).with_path(dir)),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<BuildRecord>(&text) else {
            continue;
        };
        if is_digest(&record.digest) && path == dir.join(format!("{}.json", record.digest)) {
            out.push(record);
        }
    }
    out.sort_by(|a, b| a.digest.cmp(&b.digest));
    Ok(out)
}

/// Metadata of one successful build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildRecord {
    pub digest: String,
    pub project_root: String,
    pub finished_at_ms: u64,
    pub duration_ms: u64,
    pub files_built: usize,
}

/// Directory holding build records: `<global cache>/build/records`.
pub fn records_dir() -> Option<PathBuf> {
    global_cache_dir().map(|d| d.join("build").join("records"))
}

fn is_digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Disk layer: one JSON file per digest.
#[derive(Debug, Clone)]
pub struct DiskCache {
    root: PathBuf,
}

impl DiskCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The disk cache in the global cache directory, if one can be determined.
    pub fn default_location() -> Option<Self> {
        records_dir().map(Self::new)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Record a successful build.
    pub fn store(&self, record: &BuildRecord) -> Result<()> {
        if !is_digest(&record.digest) {
            return Err(Error::invalid("cache digest must be 64 hex characters"));
        }
        std::fs::create_dir_all(&self.root).map_err(|e| Error::from(e).with_path(&self.root))?;
        let final_path = self.path_for(&record.digest);
        let tmp = self.root.join(format!("{}.tmp", record.digest));
        let text = serde_json::to_string_pretty(record)
            .map_err(|e| Error::new(tpt_lean_core::ErrorCode::Internal, e.to_string()))?;
        std::fs::write(&tmp, text).map_err(|e| Error::from(e).with_path(&tmp))?;
        std::fs::rename(&tmp, &final_path).map_err(|e| Error::from(e).with_path(&final_path))?;
        Ok(())
    }

    /// Find a successful record for `digest` built in `project_root`. Unreadable or mismatched
    /// records count as absent.
    pub fn lookup(&self, digest: &str, project_root: &str) -> Result<Option<BuildRecord>> {
        Ok(self
            .lookup_digest(digest)?
            .filter(|r| r.project_root == project_root))
    }

    /// Find a successful record for `digest`, whichever project wrote it. Unreadable records count
    /// as absent.
    pub fn lookup_digest(&self, digest: &str) -> Result<Option<BuildRecord>> {
        if !is_digest(digest) {
            return Ok(None);
        }
        let path = self.path_for(digest);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::from(e).with_path(&path)),
        };
        Ok(serde_json::from_str::<BuildRecord>(&text)
            .ok()
            .filter(|r| r.digest == digest))
    }

    /// Delete cache files older than `age` in this cache directory.
    pub fn prune(&self, age: Duration, dry_run: bool) -> Result<PruneReport> {
        prune_older_than(&self.root, age, dry_run)
    }

    fn path_for(&self, digest: &str) -> PathBuf {
        self.root.join(format!("{digest}.json"))
    }
}

/// Size of a cache directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheStatus {
    pub directory: String,
    pub exists: bool,
    pub files: u64,
    pub bytes: u64,
}

/// Summary of a prune operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PruneReport {
    pub directory: String,
    pub dry_run: bool,
    pub removed_files: usize,
    pub freed_bytes: u64,
}

/// Count files and bytes under `dir`. A missing directory reports `exists: false`.
pub fn status(dir: &Path) -> CacheStatus {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let exists = dir.is_dir();
    if exists {
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(meta) = entry.metadata() else { continue };
                if meta.is_dir() {
                    stack.push(entry.path());
                } else if meta.is_file() {
                    files += 1;
                    bytes += meta.len();
                }
            }
        }
    }
    CacheStatus {
        directory: dir.display().to_string(),
        exists,
        files,
        bytes,
    }
}

/// Delete files under `dir` whose modification time is older than `age`. With `dry_run` nothing
/// is deleted, but the report says what would be removed. A missing directory is not an error.
pub fn prune_older_than(dir: &Path, age: Duration, dry_run: bool) -> Result<PruneReport> {
    let mut report = PruneReport {
        directory: dir.display().to_string(),
        dry_run,
        removed_files: 0,
        freed_bytes: 0,
    };
    if !dir.is_dir() {
        return Ok(report);
    }
    let cutoff = SystemTime::now().checked_sub(age).unwrap_or(UNIX_EPOCH);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = std::fs::read_dir(&d).map_err(|e| Error::from(e).with_path(&d))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() && meta.modified().map(|m| m < cutoff).unwrap_or(false) {
                report.removed_files += 1;
                report.freed_bytes += meta.len();
                if !dry_run {
                    std::fs::remove_file(&path).map_err(|e| Error::from(e).with_path(&path))?;
                }
            }
        }
    }
    Ok(report)
}

/// Process-local layer: digest to the record of its last successful build.
#[derive(Debug, Default)]
pub struct MemoryCache {
    entries: HashMap<String, BuildRecord>,
}

impl MemoryCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, digest: &str) -> Option<&BuildRecord> {
        self.entries.get(digest)
    }

    pub fn put(&mut self, record: BuildRecord) {
        self.entries.insert(record.digest.clone(), record);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique temporary directory, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let p = std::env::temp_dir().join(format!("tpt-lean-build-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn digest(c: char) -> String {
        std::iter::repeat_n(c, 64).collect()
    }

    fn record(d: &str, root: &str) -> BuildRecord {
        BuildRecord {
            digest: d.into(),
            project_root: root.into(),
            finished_at_ms: 1,
            duration_ms: 2,
            files_built: 3,
        }
    }

    #[test]
    fn store_then_lookup_round_trips() {
        let tmp = TempDir::new("store");
        let cache = DiskCache::new(tmp.0.join("records"));
        let d = digest('a');
        assert!(cache.lookup(&d, "/p").unwrap().is_none());
        cache.store(&record(&d, "/p")).unwrap();
        assert_eq!(cache.lookup(&d, "/p").unwrap(), Some(record(&d, "/p")));
        // A different project does not see another project's record.
        assert!(cache.lookup(&d, "/other").unwrap().is_none());
        // Malformed digests never touch the filesystem.
        assert!(cache.lookup("../escape", "/p").unwrap().is_none());
        assert!(cache.store(&record("short", "/p")).is_err());
    }

    #[test]
    fn corrupt_record_is_a_miss() {
        let tmp = TempDir::new("corrupt");
        let cache = DiskCache::new(tmp.0.clone());
        let d = digest('b');
        std::fs::write(tmp.0.join(format!("{d}.json")), "{not json").unwrap();
        assert!(cache.lookup(&d, "/p").unwrap().is_none());
    }

    #[test]
    fn status_counts_files_and_bytes() {
        let tmp = TempDir::new("status");
        let cache = DiskCache::new(tmp.0.join("records"));
        assert!(!status(&tmp.0.join("records")).exists);
        cache.store(&record(&digest('c'), "/p")).unwrap();
        let s = status(&tmp.0);
        assert!(s.exists);
        assert_eq!(s.files, 1);
        assert!(s.bytes > 0);
    }

    #[test]
    fn prune_removes_only_old_files() {
        let tmp = TempDir::new("prune");
        let old = tmp.0.join("old.json");
        let new = tmp.0.join("new.json");
        std::fs::write(&old, "x").unwrap();
        std::fs::write(&new, "yy").unwrap();
        // Make `old` look ten days old.
        let ten_days_ago = SystemTime::now() - Duration::from_secs(10 * 86_400);
        let f = std::fs::OpenOptions::new().write(true).open(&old).unwrap();
        f.set_modified(ten_days_ago).unwrap();
        drop(f);

        let dry = prune_older_than(&tmp.0, Duration::from_secs(86_400), true).unwrap();
        assert_eq!(dry.removed_files, 1);
        assert!(old.exists(), "dry run must not delete");

        let real = prune_older_than(&tmp.0, Duration::from_secs(86_400), false).unwrap();
        assert_eq!(real.removed_files, 1);
        assert_eq!(real.freed_bytes, 1);
        assert!(!old.exists());
        assert!(new.exists());
    }

    #[test]
    fn prune_missing_directory_is_empty() {
        let tmp = TempDir::new("missing");
        let r = prune_older_than(&tmp.0.join("nope"), Duration::from_secs(1), false).unwrap();
        assert_eq!(r.removed_files, 0);
    }

    #[test]
    fn memory_layer_keeps_last_success() {
        let mut m = MemoryCache::new();
        assert!(m.is_empty());
        m.put(record(&digest('d'), "/p"));
        assert!(m.get(&digest('d')).is_some());
        assert_eq!(m.len(), 1);
        m.clear();
        assert!(m.get(&digest('d')).is_none());
    }
}

#[cfg(test)]
mod layer_tests {
    use super::*;

    const DIGEST: &str = "ab00000000000000000000000000000000000000000000000000000000000000";

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tpt-cache-layers-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn record(root: &str) -> BuildRecord {
        BuildRecord {
            digest: DIGEST.into(),
            project_root: root.into(),
            finished_at_ms: 1,
            duration_ms: 2,
            files_built: 3,
        }
    }

    #[test]
    fn shared_hit_matches_on_digest_and_is_copied_local() {
        let base = dir("shared-hit");
        let local = DiskCache::new(base.join("local"));
        let shared = vec![DiskCache::new(base.join("share"))];
        // A teammate built this project at another path and wrote to the share.
        shared[0].store(&record("/other/machine/project")).unwrap();

        let (layer, found) = lookup_layers(Some(&local), &shared, DIGEST, "/this/machine/project")
            .unwrap()
            .expect("shared hit");
        assert_eq!(layer, CacheLayer::Shared);
        assert_eq!(found.project_root, "/other/machine/project");
        // The next lookup finds it locally.
        let (layer, _) = lookup_layers(Some(&local), &shared, DIGEST, "/this/machine/project")
            .unwrap()
            .unwrap();
        assert_eq!(layer, CacheLayer::Disk);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn local_hit_needs_the_same_project_root() {
        let base = dir("local-root");
        let local = DiskCache::new(base.join("local"));
        local.store(&record("/a")).unwrap();
        assert!(lookup_layers(Some(&local), &[], DIGEST, "/a").unwrap().is_some());
        assert!(lookup_layers(Some(&local), &[], DIGEST, "/b").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn store_writes_every_layer_and_reports_a_failing_share() {
        let base = dir("store");
        let local = DiskCache::new(base.join("local"));
        let good = DiskCache::new(base.join("share"));
        // A file where a directory should be: the share cannot be written.
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("not-a-dir"), "x").unwrap();
        let bad = DiskCache::new(base.join("not-a-dir").join("sub"));
        let warnings = store_layers(Some(&local), &[good.clone(), bad], &record("/a")).unwrap();
        assert!(good.lookup_digest(DIGEST).unwrap().is_some());
        assert!(local.lookup_digest(DIGEST).unwrap().is_some());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("shared cache"), "{warnings:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn export_and_import_round_trip_and_skip_invalid_files() {
        let base = dir("export");
        let src = DiskCache::new(base.join("src"));
        src.store(&record("/a")).unwrap();
        std::fs::write(base.join("src").join("garbage.json"), "{not json").unwrap();
        std::fs::write(base.join("src").join("notes.txt"), "hello").unwrap();

        let artifact = base.join("artifact");
        assert_eq!(export_records(&src, &artifact).unwrap(), 1);
        let ci = DiskCache::new(base.join("ci"));
        assert_eq!(import_records(&artifact, &ci).unwrap(), 1);
        assert!(ci.lookup_digest(DIGEST).unwrap().is_some());
        assert_eq!(import_records(&base.join("missing"), &ci).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&base);
    }
}
