use crate::diskusage::{DiskUsageScanState, DiskUsageTree};
use crate::index::FileIndex;
use crate::ipc::IndexedSummary;
use crate::jobs::JobManager;
use crate::rename::RenameRecord;
use crate::roots::IndexRootRuntime;
use crate::settings::Settings;
use std::collections::HashMap;
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
    Arc::new(AppStateInner {
        index: Arc::new(RwLock::new(FileIndex::default())),
        disk_usage: Arc::new(RwLock::new(None)),
        disk_usage_job: Arc::new(RwLock::new(None)),
        disk_usage_state: Arc::new(Mutex::new(DiskUsageScanState::idle())),
        rename_history: Arc::new(RwLock::new(None)),
        settings: Arc::new(RwLock::new(settings)),
        jobs: JobManager::default(),
        last_index: Arc::new(RwLock::new(None)),
        current_index_job: Arc::new(RwLock::new(None)),
        root_runtime: Arc::new(RwLock::new(HashMap::new())),
    })
}
