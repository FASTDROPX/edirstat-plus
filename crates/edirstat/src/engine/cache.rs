//! Per-user persistent scan cache. Snapshots are hints, always verified by a fresh scan.
use compact_str::CompactString;
use edirstat_core::arena::{FileArenaSnapshot, NodeStorage, precompute_dir_counts};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

static GENERATION: AtomicU64 = AtomicU64::new(0);
pub struct ScanCache {
    directory: PathBuf,
    writer: std::sync::Mutex<()>,
}
impl ScanCache {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            writer: std::sync::Mutex::new(()),
        }
    }
    fn key(path: &Path, same_fs: bool) -> String {
        let path = path.to_string_lossy();
        let normalized = if cfg!(windows) {
            path.to_lowercase()
        } else {
            path.into_owned()
        };
        let mut hash = 0xcbf29ce484222325u64;
        for byte in normalized.bytes().chain([u8::from(same_fs)]) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
        format!("{hash:016x}")
    }
    fn identity(path: &Path) -> Option<(u64, u64)> {
        let metadata = fs::metadata(path).ok()?;
        let id = edirstat_core::file_id::get_file_id(path, &metadata);
        (id != (0, 0)).then_some(id)
    }
    fn manifest(&self, path: &Path, same_fs: bool) -> PathBuf {
        self.directory
            .join(format!("{}.json", Self::key(path, same_fs)))
    }
    pub fn last_path(&self) -> Option<PathBuf> {
        let value: Value =
            serde_json::from_slice(&fs::read(self.directory.join("last.json")).ok()?).ok()?;
        let path = PathBuf::from(value.get("root")?.as_str()?);
        path.is_absolute().then_some(path)
    }
    pub fn load(&self, path: &Path, same_fs: bool) -> Option<FileArenaSnapshot> {
        let metadata: Value =
            serde_json::from_slice(&fs::read(self.manifest(path, same_fs)).ok()?).ok()?;
        let identity = Self::identity(path)?;
        if metadata["version"].as_u64()? != 1
            || metadata["root"].as_str()? != path.to_string_lossy()
            || metadata["same_fs"].as_bool()? != same_fs
            || metadata["device"].as_u64()? != identity.0
            || metadata["file_id"].as_u64()? != identity.1
        {
            return None;
        }
        let file = metadata["snapshot"].as_str()?;
        if Path::new(file).components().count() != 1 || !file.starts_with(&Self::key(path, same_fs))
        {
            return None;
        }
        let loaded =
            edirstat_core::snapshot::load_snapshot_full(&self.directory.join(file)).ok()?;
        let root = loaded.arena.nodes().first()?;
        if !root.is_directory() || loaded.string_pool.get(root.name_id)? != path.to_string_lossy() {
            return None;
        }
        Some(FileArenaSnapshot {
            dir_counts: Arc::new(precompute_dir_counts(loaded.arena.nodes())),
            nodes: Arc::new(NodeStorage::Mmapped(loaded.arena)),
            string_pool: Arc::new(loaded.string_pool),
            extensions: loaded.extensions,
        })
    }
    #[cfg(test)]
    pub fn save(
        &self,
        path: &Path,
        same_fs: bool,
        snapshot: &FileArenaSnapshot,
    ) -> Result<(), String> {
        self.save_if_current(path, same_fs, snapshot, || true)
    }
    pub fn save_if_current(
        &self,
        path: &Path,
        same_fs: bool,
        snapshot: &FileArenaSnapshot,
        current: impl Fn() -> bool,
    ) -> Result<(), String> {
        let _writer = self.writer.lock().map_err(|e| e.to_string())?;
        if !current() {
            return Ok(());
        }
        let identity = Self::identity(path).ok_or("No stable root identity")?;
        if snapshot
            .nodes
            .first()
            .is_none_or(|root| !root.is_directory() || root.has_no_permission())
        {
            return Err("Incomplete scan is not cacheable".into());
        }
        fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        let key = Self::key(path, same_fs);
        let unique = format!(
            "{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
            GENERATION.fetch_add(1, Ordering::Relaxed)
        );
        let file = format!("{key}-{unique}.edst");
        let temp = self.directory.join(format!("{file}.tmp"));
        edirstat_core::snapshot::save_snapshot_ext(
            &snapshot.nodes,
            &snapshot.string_pool,
            &temp,
            true,
            &snapshot.extensions,
        )
        .map_err(|e| e.to_string())?;
        fs::rename(&temp, self.directory.join(&file)).map_err(|e| e.to_string())?;
        if !current() {
            let _ = fs::remove_file(self.directory.join(&file));
            return Ok(());
        }
        let manifest = self.manifest(path, same_fs);
        let previous: Option<Value> = fs::read(&manifest)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let metadata = json!({"version":1,"root":path.to_string_lossy(),"same_fs":same_fs,
            "device":identity.0,"file_id":identity.1,"snapshot":file});
        self.atomic_json(&manifest, &metadata, &unique)?;
        self.atomic_json(
            &self.directory.join("last.json"),
            &json!({"root":path.to_string_lossy()}),
            &unique,
        )?;
        if let Some(old) = previous.and_then(|v| v["snapshot"].as_str().map(str::to_owned)) {
            if old != file && Path::new(&old).components().count() == 1 && old.starts_with(&key) {
                let _ = fs::remove_file(self.directory.join(old));
            }
        }
        Ok(())
    }
    fn atomic_json(&self, destination: &Path, value: &Value, unique: &str) -> Result<(), String> {
        let temp = destination.with_extension(format!("{unique}.tmp"));
        let mut file = fs::File::create(&temp).map_err(|e| e.to_string())?;
        file.write_all(value.to_string().as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(temp, destination).map_err(|e| e.to_string())
    }
}

pub fn extension_stats(snapshot: &FileArenaSnapshot) -> Vec<(CompactString, u64, u32)> {
    let mut map = std::collections::HashMap::<CompactString, (u64, u32)>::new();
    for node in snapshot.nodes.iter().filter(|node| !node.is_directory()) {
        if let Some(name) = snapshot.string_pool.get(node.name_id) {
            let ext = edirstat_core::arena::get_ext_slice(name);
            edirstat_core::arena::with_lowercase_ext(ext, |ext| {
                let entry = map.entry(CompactString::new(ext)).or_default();
                entry.0 = entry.0.saturating_add(node.size);
                entry.1 = entry.1.saturating_add(1);
            });
        }
    }
    let mut stats: Vec<_> = map
        .into_iter()
        .map(|(ext, (size, count))| (ext, size, count))
        .collect();
    stats.sort_unstable_by(|a, b| b.1.cmp(&a.1));
    stats
}

// Exact comparison for identical arena order; a different order is safely treated as changed.
// No timestamp or aggregate-only shortcut may skip a changed file of the same size.
pub fn same_tree(a: &FileArenaSnapshot, b: &FileArenaSnapshot) -> bool {
    a.nodes.len() == b.nodes.len()
        && a.nodes.iter().zip(b.nodes.iter()).all(|(a_node, b_node)| {
            a_node.parent == b_node.parent
                && a_node.first_child == b_node.first_child
                && a_node.next_sibling == b_node.next_sibling
                && a_node.size == b_node.size
                && a_node.modified_timestamp == b_node.modified_timestamp
                && a_node.created_timestamp == b_node.created_timestamp
                && a_node.file_count == b_node.file_count
                && a_node.flags == b_node.flags
                && a.string_pool.get(a_node.name_id) == b.string_pool.get(b_node.name_id)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{scanner::EngineScanController, traversal::TraversalEngine};
    use edirstat_core::arena::{FileNode, StringPool};
    use edirstat_core::state::SharedState;
    use edirstat_gui::ScanController;
    use std::time::{Duration, Instant};

    fn fixture() -> (PathBuf, ScanCache) {
        let directory = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!(
                "cache-test-{}-{}",
                std::process::id(),
                GENERATION.fetch_add(1, Ordering::Relaxed)
            ));
        let root = directory.join("disk");
        fs::create_dir_all(&root).unwrap();
        (
            fs::canonicalize(root).unwrap(),
            ScanCache::new(directory.join("cache")),
        )
    }
    fn snapshot(root: &Path, name: &[u8], bytes: u64) -> FileArenaSnapshot {
        let mut pool = StringPool::new();
        let root_name = pool.get_or_insert(root.to_string_lossy().as_bytes());
        let file_name = pool.get_or_insert(name);
        let mut directory = FileNode::new(root_name, None, true, false, 0, 0);
        directory.size = bytes;
        directory.file_count = 1;
        directory.first_child = 1;
        let mut file = FileNode::new(file_name, Some(0), false, false, 0, 0);
        file.size = bytes;
        let nodes = vec![directory, file];
        FileArenaSnapshot {
            dir_counts: Arc::new(precompute_dir_counts(&nodes)),
            nodes: Arc::new(NodeStorage::Owned(nodes)),
            string_pool: Arc::new(pool),
            extensions: Default::default(),
        }
    }
    fn wait_until(mut condition: impl FnMut() -> bool) {
        let started = Instant::now();
        while !condition() {
            assert!(started.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn persistent_roundtrip_restores_tree_and_separates_scan_options() {
        let (root, cache) = fixture();
        let tree = snapshot(&root, b"photo.png", 1234);
        cache.save(&root, false, &tree).unwrap();
        let restored = cache.load(&root, false).unwrap();
        assert!(same_tree(&tree, &restored));
        assert!(cache.load(&root, true).is_none());
        assert_eq!(cache.last_path(), Some(root));
    }
    #[test]
    fn corrupt_missing_and_foreign_volume_cache_fall_back() {
        let (root, cache) = fixture();
        let tree = snapshot(&root, b"a.txt", 1);
        assert!(cache.load(&root, false).is_none());
        cache.save(&root, false, &tree).unwrap();
        let path = cache.manifest(&root, false);
        let mut metadata: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        metadata["device"] = json!(metadata["device"].as_u64().unwrap().wrapping_add(1));
        fs::write(&path, metadata.to_string()).unwrap();
        assert!(cache.load(&root, false).is_none());
        cache.save(&root, false, &tree).unwrap();
        let metadata: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        fs::write(
            cache.directory.join(metadata["snapshot"].as_str().unwrap()),
            b"broken",
        )
        .unwrap();
        assert!(cache.load(&root, false).is_none());
    }
    #[test]
    fn atomic_generation_replaces_completed_cache_and_ignores_partial_file() {
        let (root, cache) = fixture();
        let tree = snapshot(&root, b"a.txt", 1);
        cache.save(&root, false, &tree).unwrap();
        fs::write(cache.directory.join("interrupted.tmp"), b"partial").unwrap();
        assert!(same_tree(&tree, &cache.load(&root, false).unwrap()));
        let newer = snapshot(&root, b"renamed.txt", 2);
        cache.save(&root, false, &newer).unwrap();
        assert!(same_tree(&newer, &cache.load(&root, false).unwrap()));
        let files = fs::read_dir(&cache.directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "edst"))
            .count();
        assert_eq!(files, 1);
    }
    #[test]
    fn obsolete_writer_cannot_replace_a_newer_cache() {
        let (root, cache) = fixture();
        let current = snapshot(&root, b"current.txt", 999);
        cache.save(&root, false, &current).unwrap();
        cache
            .save_if_current(&root, false, &snapshot(&root, b"obsolete.txt", 1), || false)
            .unwrap();
        assert!(same_tree(&current, &cache.load(&root, false).unwrap()));
    }

    #[test]
    fn equal_totals_do_not_hide_renamed_or_modified_files() {
        let (root, _) = fixture();
        let old = snapshot(&root, b"old.txt", 100);
        assert!(same_tree(&old, &old));
        assert!(!same_tree(&old, &snapshot(&root, b"new.txt", 100)));
        let mut changed = (*old.nodes).to_vec();
        changed[1].modified_timestamp = 99;
        let newer = FileArenaSnapshot {
            nodes: Arc::new(NodeStorage::Owned(changed)),
            ..old.clone()
        };
        assert!(!same_tree(&old, &newer));
    }
    #[test]
    fn scanner_reopens_cache_refreshes_added_changed_deleted_files_and_keeps_completed_copy() {
        let (root, cache) = fixture();
        let location = cache.directory.clone();
        fs::write(root.join("old.txt"), b"old").unwrap();
        let shared = Arc::new(SharedState::new());
        let scanner = EngineScanController::new(
            Arc::new(TraversalEngine::new(shared.scan_stats.clone())),
            shared.clone(),
        )
        .with_cache(cache);
        scanner.start_scan(root.clone(), false);
        wait_until(|| !shared.is_scanning.load(Ordering::Acquire));
        scanner.flush_cache();
        wait_until(|| {
            ScanCache::new(location.clone())
                .load(&root, false)
                .is_some()
        });
        fs::remove_file(root.join("old.txt")).unwrap();
        fs::write(root.join("new.txt"), b"updated-data").unwrap();
        let reopened = Arc::new(SharedState::new());
        let second = EngineScanController::new(
            Arc::new(TraversalEngine::new(reopened.scan_stats.clone())),
            reopened.clone(),
        )
        .with_cache(ScanCache::new(location.clone()));
        assert_eq!(second.last_cached_path(), Some(root.clone()));
        second.start_scan(root.clone(), false);
        wait_until(|| !reopened.is_scanning.load(Ordering::Acquire));
        second.flush_cache();
        let tree = reopened.current_snapshot.load_full();
        assert_eq!(tree.nodes[0].size, 12);
        assert!(
            tree.nodes
                .iter()
                .any(|node| tree.string_pool.get(node.name_id) == Some("new.txt"))
        );
        assert!(
            !tree
                .nodes
                .iter()
                .any(|node| tree.string_pool.get(node.name_id) == Some("old.txt"))
        );
        wait_until(|| {
            ScanCache::new(location.clone())
                .load(&root, false)
                .is_some_and(|tree| tree.nodes[0].size == 12)
        });
    }
    #[test]
    fn cancel_before_verification_retains_cached_tree_and_never_overwrites_it() {
        let (root, cache) = fixture();
        cache
            .save(&root, false, &snapshot(&root, b"cached.txt", 777))
            .unwrap();
        let location = cache.directory.clone();
        let shared = Arc::new(SharedState::new());
        shared.scan_cancel.store(true, Ordering::Release);
        let scanner = EngineScanController::new(
            Arc::new(TraversalEngine::new(shared.scan_stats.clone())),
            shared.clone(),
        )
        .with_cache(cache);
        scanner.start_scan(root.clone(), false);
        wait_until(|| !shared.is_scanning.load(Ordering::Acquire));
        assert_eq!(shared.cache_status.load(Ordering::Acquire), 3);
        assert_eq!(shared.current_snapshot.load().nodes[0].size, 777);
        assert_eq!(
            ScanCache::new(location).load(&root, false).unwrap().nodes[0].size,
            777
        );
    }
}
