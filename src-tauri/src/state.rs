use crate::diskusage::{DiskUsageScanState, DiskUsageTree};
use crate::index::FileIndex;
use crate::ipc::IndexedSummary;
use crate::jobs::JobManager;
use crate::rename::RenameRecord;
use crate::roots::{IndexRootRuntime, RootState};
use crate::settings::Settings;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;

pub struct AppStateInner {
    pub index: Arc<RwLock<FileIndex>>,
    pub disk_usage: Arc<RwLock<Option<DiskUsageTree>>>,
    pub disk_usage_job: Arc<RwLock<Option<String>>>,
    /// Authoritative live scan state — updated by the scanner thread,
    /// readable from the IPC layer for tab-resume.
    pub disk_usage_state: Arc<Mutex<DiskUsageScanState>>,
    pub rename_history: Arc<RwLock<Option<Vec<RenameRecord>>>>,
    pub settings: Arc<RwLock<Settings>>,
    pub jobs: JobManager,
    pub last_index: Arc<RwLock<Option<IndexedSummary>>>,
    /// Currently-running index/search job id. None when no scan is active.
    /// The IPC layer flips this to Some(id) when starting and back to None
    /// when the scan finishes (success/error/cancel). The Search frontend
    /// uses `get_index_scan_state` to recover from missed completion events.
    pub current_index_job: Arc<RwLock<Option<String>>>,
    /// Per-root runtime status keyed by root id. Not persisted.
    pub root_runtime: Arc<RwLock<HashMap<String, IndexRootRuntime>>>,
}

pub type AppState = Arc<AppStateInner>;

pub fn new_app_state() -> AppState {
    let settings = crate::settings::load_settings();

    // Eager cache hydration. The frontend's autoload path is racy with
    // Search.svelte's parallel state-pull and a silent throw there leaves
    // the UI on "No index loaded" while the cache file on disk is fine
    // (Settings still shows it because get_cache_info reads the JSON
    // directly, independent of state.index). Hydrating here guarantees
    // state.index/last_index/root_runtime reflect the cache before any IPC
    // returns, so get_index_stats reports the real total on first call.
    //
    // Failures are intentionally silent: missing cache is normal first-run,
    // and a corrupt/legacy cache should fall through to the empty-state UI
    // and let the user click Index Now — never block app start.
    //
    // Gated on settings.auto_load_cache: when the user has disabled
    // "Auto-load cached index on startup", backend state must start empty
    // so Search remains unusable until they explicitly click Load Cache or
    // Index Now. Settings still reads cache metadata directly via
    // get_cache_info, so the cache-exists indicator is unaffected.
    let (index, last, runtime) = if settings.auto_load_cache {
        hydrate_from_cache(&settings)
    } else {
        (FileIndex::default(), None, HashMap::new())
    };

    Arc::new(AppStateInner {
        index: Arc::new(RwLock::new(index)),
        disk_usage: Arc::new(RwLock::new(None)),
        disk_usage_job: Arc::new(RwLock::new(None)),
        disk_usage_state: Arc::new(Mutex::new(DiskUsageScanState::idle())),
        rename_history: Arc::new(RwLock::new(None)),
        settings: Arc::new(RwLock::new(settings)),
        jobs: JobManager::default(),
        last_index: Arc::new(RwLock::new(last)),
        current_index_job: Arc::new(RwLock::new(None)),
        root_runtime: Arc::new(RwLock::new(runtime)),
    })
}

fn hydrate_from_cache(
    settings: &Settings,
) -> (
    FileIndex,
    Option<IndexedSummary>,
    HashMap<String, IndexRootRuntime>,
) {
    let enabled_ids: std::collections::HashSet<String> = settings
        .indexed_roots
        .iter()
        .filter(|r| r.enabled)
        .map(|r| r.id.clone())
        .collect();

    let (info, index) = match crate::cache::load_cache(Some(&enabled_ids)) {
        Ok(v) => v,
        Err(e) => {
            // "No cached index" on first run is the common path, not an
            // error worth shouting about. Log everything else so a corrupt
            // cache shows up in the terminal.
            if !e.contains("No cached index") {
                eprintln!("[startup] cache hydration skipped: {e}");
            }
            return (FileIndex::default(), None, HashMap::new());
        }
    };

    if index.len() == 0 {
        // Cache exists but the enabled-root filter dropped every entry.
        // Same outcome as no cache — frontend renders "No index loaded"
        // and the user can rebuild.
        return (FileIndex::default(), None, HashMap::new());
    }

    let cached_by_id: HashMap<String, u64> = info
        .root_meta
        .iter()
        .map(|m| (m.id.clone(), m.item_count))
        .collect();

    let mut runtime: HashMap<String, IndexRootRuntime> = HashMap::new();
    let now = info.timestamp;
    for r in settings.indexed_roots.iter().filter(|r| r.enabled) {
        let Some(count) = cached_by_id.get(&r.id).copied() else {
            continue;
        };
        let reachable = Path::new(&r.path).exists();
        let entry = runtime.entry(r.id.clone()).or_default();
        entry.last_indexed = now;
        entry.item_count = count;
        if reachable {
            entry.state = RootState::Ready;
            entry.error = None;
        } else {
            entry.state = RootState::Missing;
            entry.error = Some("Path not accessible".to_string());
        }
    }

    let last = Some(IndexedSummary {
        roots: info.roots.clone(),
        timestamp: info.timestamp,
    });

    eprintln!(
        "[startup] cache hydrated: {} entries from {} root(s)",
        index.len(),
        info.roots.len()
    );

    (index, last, runtime)
}
