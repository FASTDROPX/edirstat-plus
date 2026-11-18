use std::{path::PathBuf, sync::Arc, sync::atomic::Ordering};

use edirstat_core::docker::DockerCollector;
use edirstat_core::state::SharedState;
use edirstat_gui::ScanController;

use super::{
    coordinator::Coordinator,
    docker::{NativeDockerCollector, attach_docker_inventory},
    traversal::TraversalEngine,
};

/// Native scanner backend: runs directory scans on background threads via the
/// traversal engine and coordinator, publishing snapshots and progress into
/// the shared state for the GUI to render.
pub struct EngineScanController {
    traversal_engine: Arc<TraversalEngine>,
    shared_state: Arc<SharedState>,
    cache: Option<Arc<super::cache::ScanCache>>,
    generation: Arc<std::sync::atomic::AtomicU64>,
    pending_cache_writes: Arc<(std::sync::Mutex<usize>, std::sync::Condvar)>,
}

impl EngineScanController {
    pub fn new(traversal_engine: Arc<TraversalEngine>, shared_state: Arc<SharedState>) -> Self {
        Self {
            traversal_engine,
            shared_state,
            cache: edirstat_gui::preferences::scan_cache_directory()
                .map(|directory| Arc::new(super::cache::ScanCache::new(directory))),
            generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            pending_cache_writes: Arc::new((std::sync::Mutex::new(0), std::sync::Condvar::new())),
        }
    }
    #[cfg(test)]
    pub(crate) fn with_cache(mut self, cache: super::cache::ScanCache) -> Self {
        self.cache = Some(Arc::new(cache));
        self
    }
}

impl ScanController for EngineScanController {
    fn flush_cache(&self) {
        let (count, wake) = &*self.pending_cache_writes;
        if let Ok(mut pending) = count.lock() {
            while *pending > 0 {
                match wake.wait(pending) {
                    Ok(value) => pending = value,
                    Err(_) => break,
                }
            }
        }
    }

    fn last_cached_path(&self) -> Option<PathBuf> {
        self.cache.as_ref()?.last_path()
    }

    fn start_scan(&self, path: PathBuf, same_filesystem: bool) {
        if self
            .shared_state
            .is_scanning
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        self.shared_state.cache_status.store(1, Ordering::Release);
        let engine = self.traversal_engine.clone();
        let shared = self.shared_state.clone();
        let cache = self.cache.clone();
        let generation = self.generation.clone();
        let pending_cache_writes = self.pending_cache_writes.clone();
        let request = generation.fetch_add(1, Ordering::SeqCst) + 1;
        std::thread::spawn(move || {
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            let cached = cache
                .as_ref()
                .and_then(|cache| cache.load(&path, same_filesystem));
            if generation.load(Ordering::Acquire) != request {
                return;
            }
            if let Some(snapshot) = &cached {
                shared
                    .extension_stats
                    .store(Arc::new(super::cache::extension_stats(snapshot)));
                shared.store_snapshot(snapshot.clone());
                shared.cache_status.store(2, Ordering::Release);
            } else {
                shared.cache_status.store(0, Ordering::Release);
            }
            if shared.scan_cancel.load(Ordering::Acquire) {
                shared
                    .cache_status
                    .store(if cached.is_some() { 3 } else { 0 }, Ordering::Release);
                shared.is_scanning.store(false, Ordering::Release);
                return;
            }
            // A cached tree remains visible while the regular MFT/traversal pipeline
            // builds its replacement privately. Scan progress is still live.
            let scan_state = if cached.is_some() {
                let mut private = SharedState::new();
                private.scan_cancel = shared.scan_cancel.clone();
                private.scan_stats = shared.scan_stats.clone();
                private.live_updates.store(false, Ordering::Relaxed);
                Arc::new(private)
            } else {
                shared.clone()
            };
            let (tx, rx) = crossbeam::channel::unbounded();
            shared.scan_stats.reset();
            match engine.start_traversal(
                path.clone(),
                same_filesystem,
                shared.scan_cancel.clone(),
                tx,
            ) {
                Ok(_) => {
                    let mut coordinator = Coordinator::new(rx, scan_state.clone());
                    coordinator.run_coordinator_loop_deferred(&path.to_string_lossy());
                    if generation.load(Ordering::Acquire) != request {
                        return;
                    }
                    let fresh = scan_state.current_snapshot.load_full();
                    let failed = fresh
                        .nodes
                        .first()
                        .is_none_or(|node| node.has_no_permission());
                    let cancelled = shared.scan_cancel.load(Ordering::Acquire);
                    if cached.is_some() && (cancelled || failed) {
                        shared.cache_status.store(3, Ordering::Release);
                        shared.is_scanning.store(false, Ordering::Release);
                        return;
                    }
                    if let Some(cached) = &cached {
                        if !super::cache::same_tree(cached, &fresh) {
                            shared.current_snapshot.store(fresh.clone());
                        }
                        shared
                            .extension_stats
                            .store(scan_state.extension_stats.load_full());
                    }
                    let writing = !cancelled && !failed && cache.is_some();
                    if writing {
                        if let Ok(mut count) = pending_cache_writes.0.lock() {
                            *count += 1;
                        }
                    }
                    shared
                        .cache_status
                        .store(if writing { 4 } else { 0 }, Ordering::Release);
                    shared.is_scanning.store(false, Ordering::Release);
                    if writing {
                        if let Some(cache) = &cache {
                            if let Err(error) =
                                cache.save_if_current(&path, same_filesystem, &fresh, || {
                                    generation.load(Ordering::Acquire) == request
                                })
                            {
                                eprintln!("Could not save scan cache: {error}");
                            }
                        }
                        if let Ok(mut count) = pending_cache_writes.0.lock() {
                            *count = count.saturating_sub(1);
                        }
                        pending_cache_writes.1.notify_all();
                        if generation.load(Ordering::Acquire) == request {
                            shared.cache_status.store(0, Ordering::Release);
                        }
                    }
                    #[cfg(not(test))]
                    if generation.load(Ordering::Acquire) == request
                        && !edirstat_gui::gui::operations::is_macos_sandbox()
                    {
                        attach_docker_after_scan(&shared);
                    }
                }
                Err(error) => {
                    shared
                        .cache_status
                        .store(if cached.is_some() { 3 } else { 0 }, Ordering::Release);
                    shared.is_scanning.store(false, Ordering::Release);
                    eprintln!("Failed to start traversal: {error}");
                }
            }
        });
    }

    fn num_threads(&self) -> usize {
        self.traversal_engine.num_threads()
    }

    fn docker_collector(&self) -> Option<Arc<dyn edirstat_gui::docker::DockerCollector>> {
        // The App Sandbox blocks every Docker access path (the VM disk lives
        // in another app's container; unix sockets are off-limits), so live
        // collection is impossible in sandboxed builds — report no collector
        // and let the UI fall back to reviewing snapshot-carried data.
        if edirstat_gui::gui::operations::is_macos_sandbox() {
            return None;
        }
        Some(Arc::new(super::docker::NativeDockerCollector::new()))
    }
}

/// Post-scan Docker collection: runs on the coordinator's background thread
/// after `is_scanning` flips false, so scan completion is never blocked.
/// Silently skips when the scan was cancelled or no Docker presence was
/// detected; collection/attach failures are logged but never disturb the
/// scan results.
fn attach_docker_after_scan(shared_state: &Arc<SharedState>) {
    if shared_state.scan_cancel.load(Ordering::Relaxed) {
        return;
    }
    let collector = NativeDockerCollector::new();
    if !collector.detect_environment().has_docker() {
        return;
    }
    match collector.collect_preferred(&shared_state.scan_cancel) {
        Ok(inventory) => {
            if let Err(e) = attach_docker_inventory(shared_state, &inventory) {
                eprintln!("Failed to attach Docker inventory: {e}");
            }
        }
        Err(e) => eprintln!("Docker inventory collection failed: {e}"),
    }
}
