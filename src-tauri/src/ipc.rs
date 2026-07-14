use crate::cache::{self, CacheInfo};
use crate::diskusage::{self, DiskScanPhase, DiskScanStatus, DiskUsageInfo, DiskUsageScanState};
use crate::rename::{self, RenamePreviewItem, RenameRule};
use crate::roots::{
    self, IndexRootRuntime, IndexRootView, RootState,
};
use crate::search::{SearchFilters, SearchPage, SearchSort};
use crate::settings::{stable_root_id, IndexRoot, Settings};
use crate::state::AppState;
use crate::walker::{self, WalkRoot};
use serde::Serialize;
use std::path::Path;
use tauri::{command, AppHandle, Emitter, State};

#[derive(Serialize)]
pub struct IndexStats {
    pub total: usize,
    pub files: usize,
    pub dirs: usize,
    pub roots: Vec<String>,
    pub indexed_at: i64,
}

/// Authoritative index/search scan state, used by the frontend to recover
/// from missed completion events (e.g. tab switch race, fast scan, dropped
/// event). Mirrors `DiskUsageScanState` but for the file index.
#[derive(Serialize)]
pub struct IndexScanStateView {
    pub is_running: bool,
    pub current_job_id: Option<String>,
    pub total: usize,
    pub files: usize,
    pub dirs: usize,
    pub roots: Vec<String>,
    pub indexed_at: i64,
    pub root_views: Vec<IndexRootView>,
}

#[command]
pub async fn scan_index(
    root: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    scan_index_multi(vec![root], app, state).await
}

/// Index a list of raw paths. Used by older callers and the first-boot
/// fallback (when no enabled roots are configured). Each path is registered
/// as an enabled IndexRoot in settings (with a stable id derived from the
/// path) before scanning so the resulting cache entries pass the
/// enabled-root filter at search time. Without that registration the search
/// pipeline treats the entries as orphaned and returns zero results.
#[command]
pub async fn scan_index_multi(
    roots: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let walk_roots: Vec<WalkRoot> = roots
        .iter()
        .map(|p| WalkRoot {
            id: stable_root_id(p),
            path: p.clone(),
        })
        .collect();

    // Register any not-yet-configured paths so search's enabled-root filter
    // recognizes them. Existing entries are left alone (their enabled flag
    // is preserved); only missing ones are added with enabled=true.
    let snapshot = {
        let mut s = state.settings.write().await;
        let mut changed = false;
        for p in &roots {
            let id = stable_root_id(p);
            if !s.indexed_roots.iter().any(|r| r.id == id) {
                s.indexed_roots.push(IndexRoot::from_path(p.clone()));
                changed = true;
            }
        }
        if changed {
            Some(s.clone())
        } else {
            None
        }
    };
    if let Some(snap) = snapshot {
        crate::settings::save_settings(&snap)?;
    }

    start_scan(walk_roots, None, app, state).await
}

/// Index every enabled root in settings. Primary entry point for the UI.
#[command]
pub async fn start_index_all(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let walk_roots: Vec<WalkRoot> = {
        let s = state.settings.read().await;
        s.indexed_roots
            .iter()
            .filter(|r| r.enabled)
            .map(|r| WalkRoot {
                id: r.id.clone(),
                path: r.path.clone(),
            })
            .collect()
    };
    if walk_roots.is_empty() {
        return Err("No enabled indexed roots configured".to_string());
    }
    start_scan(walk_roots, None, app, state).await
}

/// Index a single root by id (replaces it in the index). Other roots are
/// rebuilt from settings to keep the index complete — partial index updates
/// would require a different on-disk model than we have today.
#[command]
pub async fn rescan_index_root(
    root_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let walk_roots: Vec<WalkRoot> = {
        let s = state.settings.read().await;
        let target = s.indexed_roots.iter().find(|r| r.id == root_id).cloned();
        match target {
            Some(r) if r.enabled => {
                // Include all enabled roots so the resulting index covers
                // everything the user expects. The single-root rescan UX
                // is mostly about acknowledging that one root needs a
                // refresh; the walker will handle the rest.
                s.indexed_roots
                    .iter()
                    .filter(|x| x.enabled)
                    .map(|x| WalkRoot {
                        id: x.id.clone(),
                        path: x.path.clone(),
                    })
                    .collect()
            }
            Some(_) => return Err("Root is disabled".to_string()),
            None => return Err("Root not found".to_string()),
        }
    };
    start_scan(walk_roots, Some(root_id), app, state).await
}

async fn start_scan(
    walk_roots: Vec<WalkRoot>,
    _focus_root_id: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    if walk_roots.is_empty() {
        return Err("No roots to index".to_string());
    }

    // Atomic publish: prepare the new job's id/token first, then acquire the
    // `current_index_job` write lock exactly once and swap old → new in a
    // single operation. This eliminates the previous transient `None` window
    // (take → release → reacquire → set) where `load_cached_index` could
    // squeeze in, observe `None`, and commit the cached index right as a
    // scan was being started. After the swap we cancel the prior token —
    // the walker checks the token cooperatively, so cancellation can run
    // outside the guard without re-introducing a race.
    let (job_id, token) = state.jobs.create();
    let prior = {
        let mut g = state.current_index_job.write().await;
        g.replace(job_id.clone())
    };
    if let Some(prev) = prior {
        state.jobs.cancel(&prev);
    }

    // Mark each root as Pending up front so the UI can show pending dots.
    {
        let mut rt = state.root_runtime.write().await;
        for r in &walk_roots {
            let entry = rt.entry(r.id.clone()).or_default();
            entry.state = RootState::Pending;
            entry.error = None;
        }
    }

    let index_arc = state.index.clone();
    let jid = job_id.clone();
    let jobs = state.jobs.clone();
    let excluded = state.settings.read().await.excluded_patterns.clone();
    let stats_arc = state.last_index.clone();
    let current_job_arc = state.current_index_job.clone();
    let runtime_arc = state.root_runtime.clone();
    let publish_guard_arc = state.publish_guard.clone();

    tokio::task::spawn_blocking(move || {
        let walk_roots_clone = walk_roots.clone();
        match walker::walk_directories(&walk_roots_clone, token, jid.clone(), app.clone(), &excluded) {
            Ok((new_index, summary)) => {
                let rt = tokio::runtime::Handle::current();
                let still_active = rt.block_on(async {
                    let cur = current_job_arc.read().await;
                    matches!(cur.as_deref(), Some(active) if active == jid.as_str())
                });
                if still_active {
                    let published = rt.block_on(async {
                        // Serialize against `clear_cache`. Holding
                        // `publish_guard` across the re-validate +
                        // `save_cache` + commit window makes the worker's
                        // publish path mutually exclusive with Clear Cache's
                        // cancel + delete + wipe path. Either the worker
                        // fully wins (saves and commits, then Clear Cache
                        // wipes both memory and disk afterward), or Clear
                        // Cache fully wins (sets current_index_job to None
                        // first, so this re-check fails and `save_cache` is
                        // never called). The unsafe middle case — Clear
                        // Cache deletes the file between our active-job
                        // check and our save — is now impossible.
                        let _publish_lock = publish_guard_arc.lock().await;

                        // Build the set of root_ids whose entries should be
                        // *replaced* by this scan — only roots that actually
                        // walked successfully. Failed/missing roots are left
                        // alone so a temporarily missing removable drive
                        // doesn't silently delete its previously cached
                        // entries.
                        let replaced_roots: std::collections::HashSet<String> = summary
                            .per_root
                            .iter()
                            .filter(|r| r.error.is_none())
                            .map(|r| r.root_id.clone())
                            .collect();

                        let mut idx = index_arc.write().await;
                        // Re-validate under the index lock. The outer
                        // `still_active` read is a cheap fast-path; between
                        // dropping it and acquiring index.write, Clear Cache
                        // (or a replacement scan) may have swapped the slot
                        // to None. Without this re-check, a scan that
                        // happened to finish just as Clear Cache fired would
                        // re-save the cache the user just wiped.
                        {
                            let cur = current_job_arc.read().await;
                            if !matches!(cur.as_deref(), Some(active) if active == jid.as_str()) {
                                return false;
                            }
                        }
                        let merged = crate::index::FileIndex::merge_replacing(
                            &idx,
                            new_index,
                            &replaced_roots,
                        );
                        // Save the *merged* index to cache so failed-root
                        // entries persist across restarts.
                        let _ = cache::save_cache(&merged);
                        *idx = merged;
                        // Aggregate canonical roots represented in the
                        // merged index for the IndexedSummary. Successful
                        // scans of this job + previously-known roots that
                        // we preserved.
                        let canonical_roots: Vec<String> = idx
                            .roots
                            .values()
                            .map(|m| m.canonical_path.clone())
                            .collect();
                        drop(idx);

                        let mut last = stats_arc.write().await;
                        let now_ts = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        *last = Some(IndexedSummary {
                            roots: canonical_roots.clone(),
                            timestamp: now_ts,
                        });
                        // Per-root runtime state from this scan's per_root
                        // results. Successful roots → Ready, errored →
                        // Error/Missing (entries preserved from prior scan
                        // are kept; the runtime entry surfaces the error).
                        let mut runtime = runtime_arc.write().await;
                        for r in &summary.per_root {
                            let entry = runtime.entry(r.root_id.clone()).or_default();
                            entry.last_indexed = now_ts;
                            if let Some(err) = &r.error {
                                entry.state = RootState::Error;
                                entry.error = Some(err.clone());
                                // Don't zero item_count — preserved entries
                                // still contribute to search results.
                            } else {
                                entry.item_count = r.item_count;
                                entry.state = RootState::Ready;
                                entry.error = None;
                            }
                        }
                        let mut cur = current_job_arc.write().await;
                        if cur.as_deref() == Some(jid.as_str()) {
                            *cur = None;
                        }
                        true
                    });
                    if published {
                        let _ = app.emit("index.complete", summary);
                    } else {
                        eprintln!(
                            "[index] dropped completion for replaced/cancelled job_id={}",
                            jid
                        );
                    }
                } else {
                    eprintln!(
                        "[index] dropped completion for replaced/cancelled job_id={}",
                        jid
                    );
                }
            }
            Err(e) => {
                let rt = tokio::runtime::Handle::current();
                rt.block_on(async {
                    let mut cur = current_job_arc.write().await;
                    if cur.as_deref() == Some(jid.as_str()) {
                        *cur = None;
                    }
                    // If the job was cancelled, mark pending roots as idle/ready
                    // so the UI doesn't get stuck on "indexing" badges.
                    let mut runtime = runtime_arc.write().await;
                    for r in &walk_roots_clone {
                        if let Some(entry) = runtime.get_mut(&r.id) {
                            if entry.state == RootState::Pending
                                || entry.state == RootState::Indexing
                            {
                                entry.state = if entry.item_count > 0 {
                                    RootState::Ready
                                } else {
                                    RootState::Idle
                                };
                            }
                        }
                    }
                });
                let _ = app.emit(
                    "index.error",
                    serde_json::json!({ "job_id": jid, "error": e }),
                );
            }
        }
        jobs.remove(&jid);
    });

    Ok(job_id)
}

#[derive(Clone)]
pub struct IndexedSummary {
    pub roots: Vec<String>,
    pub timestamp: i64,
}

#[command]
pub async fn search(
    query: String,
    filters: SearchFilters,
    sort: SearchSort,
    page: Option<usize>,
    page_size: Option<usize>,
    state: State<'_, AppState>,
) -> Result<SearchPage, String> {
    let index = state.index.read().await;
    let p = page.unwrap_or(0);
    let ps = page_size.unwrap_or(200);
    let s = state.settings.read().await;
    let case_sensitive = s.case_sensitive;
    let match_path = s.match_path;
    let enabled_ids: std::collections::HashSet<String> = s
        .indexed_roots
        .iter()
        .filter(|r| r.enabled)
        .map(|r| r.id.clone())
        .collect();
    drop(s);
    Ok(crate::search::search(
        &index,
        &query,
        &filters,
        &sort,
        p,
        ps,
        case_sensitive,
        match_path,
        Some(&enabled_ids),
    ))
}

#[command]
pub async fn cancel_job(id: String, state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.jobs.cancel(&id))
}

#[command]
pub async fn get_index_stats(state: State<'_, AppState>) -> Result<IndexStats, String> {
    let index = state.index.read().await;
    let total = index.len();
    let mut files = 0usize;
    let mut dirs = 0usize;
    for i in 0..total {
        if index.is_dir(i) {
            dirs += 1;
        } else {
            files += 1;
        }
    }
    let last = state.last_index.read().await.clone();
    let (roots, indexed_at) = match last {
        Some(s) => (s.roots, s.timestamp),
        None => (Vec::new(), 0),
    };
    Ok(IndexStats {
        total,
        files,
        dirs,
        roots,
        indexed_at,
    })
}

#[command]
pub async fn get_index_scan_state(
    state: State<'_, AppState>,
) -> Result<IndexScanStateView, String> {
    let current_job_id = state.current_index_job.read().await.clone();
    let is_running = current_job_id.is_some();
    let index = state.index.read().await;
    let total = index.len();
    let mut files = 0usize;
    let mut dirs = 0usize;
    for i in 0..total {
        if index.is_dir(i) {
            dirs += 1;
        } else {
            files += 1;
        }
    }
    drop(index);
    let last = state.last_index.read().await.clone();
    let (roots, indexed_at) = match last {
        Some(s) => (s.roots, s.timestamp),
        None => (Vec::new(), 0),
    };
    let root_views = build_root_views(&state).await;
    Ok(IndexScanStateView {
        is_running,
        current_job_id,
        total,
        files,
        dirs,
        roots,
        indexed_at,
        root_views,
    })
}

async fn build_root_views(state: &State<'_, AppState>) -> Vec<IndexRootView> {
    let s = state.settings.read().await;
    let runtime = state.root_runtime.read().await;
    s.indexed_roots
        .iter()
        .map(|r| roots::make_view(r, &runtime))
        .collect()
}

// ---------- Root management commands ----------

#[command]
pub async fn get_index_roots(
    state: State<'_, AppState>,
) -> Result<Vec<IndexRootView>, String> {
    Ok(build_root_views(&state).await)
}

#[command]
pub async fn add_index_root(
    path: String,
    display_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<IndexRootView, String> {
    let canonical = roots::validate_root_path(&path)?;
    let mut s = state.settings.write().await;
    if s.indexed_roots.iter().any(|r| r.path == canonical) {
        return Err(format!("Root already added: {canonical}"));
    }
    // Resolve every existing configured root to a canonical path when
    // possible so overlap detection works against real filesystem layout
    // rather than the user-typed string. A root that no longer canonicalizes
    // (unplugged drive) falls back to its stored path.
    let existing_canonical: Vec<String> = s
        .indexed_roots
        .iter()
        .map(|r| {
            dunce::canonicalize(&r.path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| r.path.clone())
        })
        .collect();
    if let Some(parent) = roots::nested_under(&canonical, &existing_canonical) {
        return Err(format!(
            "This location is already covered by indexed root: {parent}"
        ));
    }
    if let Some(child) = roots::nested_contains(&canonical, &existing_canonical) {
        return Err(format!(
            "This location contains an already indexed root: {child}"
        ));
    }
    let mut new_root = IndexRoot::from_path(canonical.clone());
    if let Some(name) = display_name {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            new_root.display_name = trimmed.to_string();
        }
    }
    s.indexed_roots.push(new_root.clone());
    let snapshot = s.clone();
    drop(s);
    crate::settings::save_settings(&snapshot)?;
    let runtime = state.root_runtime.read().await;
    Ok(roots::make_view(&new_root, &runtime))
}

#[command]
pub async fn remove_index_root(
    root_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut s = state.settings.write().await;
    let before = s.indexed_roots.len();
    s.indexed_roots.retain(|r| r.id != root_id);
    if s.indexed_roots.len() == before {
        return Err("Root not found".to_string());
    }
    let snapshot = s.clone();
    drop(s);
    crate::settings::save_settings(&snapshot)?;
    // Drop in-memory + cached entries owned by the removed root so they
    // don't reappear in search until a rebuild. Other roots are untouched.
    let removed: std::collections::HashSet<String> =
        std::iter::once(root_id.clone()).collect();
    {
        let mut idx = state.index.write().await;
        let pruned = crate::index::FileIndex::merge_replacing(
            &idx,
            crate::index::FileIndex::default(),
            &removed,
        );
        let _ = cache::save_cache(&pruned);
        *idx = pruned;
    }
    let mut rt = state.root_runtime.write().await;
    rt.remove(&root_id);
    Ok(())
}

#[command]
pub async fn update_index_root_enabled(
    root_id: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut s = state.settings.write().await;
    let target = s.indexed_roots.iter_mut().find(|r| r.id == root_id);
    match target {
        Some(r) => r.enabled = enabled,
        None => return Err("Root not found".to_string()),
    }
    let snapshot = s.clone();
    drop(s);
    crate::settings::save_settings(&snapshot)?;
    Ok(())
}

#[command]
pub async fn detect_index_roots() -> Result<Vec<roots::DetectedRoot>, String> {
    Ok(roots::detect_drives())
}

/// OS-appropriate default root (the user's home directory). Used by the
/// frontend as a race-proof fallback so it never hardcodes a Linux path
/// like "/home" on Windows/macOS.
#[command]
pub async fn default_root() -> String {
    dirs::home_dir()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|| if cfg!(windows) { "C:\\".to_string() } else { "/".to_string() })
}

// ---------- Disk usage ----------

#[command]
pub async fn scan_disk_usage(
    root: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    eprintln!("[diskusage] start command received: root={}", root);

    // Cancel any prior scan so a new scan replaces it cleanly.
    let prior = {
        let mut g = state.disk_usage_job.write().await;
        g.take()
    };
    if let Some(prev) = prior {
        state.jobs.cancel(&prev);
    }

    let (job_id, token) = state.jobs.create();
    {
        let mut g = state.disk_usage_job.write().await;
        *g = Some(job_id.clone());
    }
    let du_arc = state.disk_usage.clone();
    let scan_state = state.disk_usage_state.clone();
    let active_job = state.disk_usage_job.clone();
    let jid = job_id.clone();
    let jobs = state.jobs.clone();
    let excluded = state.settings.read().await.excluded_patterns.clone();

    // Reset the authoritative scan state up front so a frontend that polls
    // immediately after invoking sees a meaningful "starting" snapshot.
    if let Ok(mut s) = scan_state.lock() {
        s.scan_id = Some(jid.clone());
        s.status = DiskScanStatus::Scanning;
        s.phase = DiskScanPhase::Starting;
        s.engine = diskusage::engine_name().to_string();
        s.root_path = root.clone();
        s.current_path = root.clone();
        s.elapsed_ms = 0;
        s.files_scanned = 0;
        s.dirs_scanned = 0;
        s.bytes_scanned = 0;
        s.errors_count = 0;
        s.skipped_count = 0;
        s.message = "Starting scan…".to_string();
        s.final_summary = None;
    }

    std::thread::spawn(move || {
        let scan_app = app.clone();
        let scan_jid = jid.clone();
        let scan_state_clone = scan_state.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            diskusage::scan_disk_usage(
                &root,
                token,
                scan_jid.clone(),
                scan_app.clone(),
                &excluded,
                scan_state_clone,
            )
        }));
        match result {
            Ok(Ok((tree, summary))) => {
                let still_active = {
                    let g = active_job.blocking_read();
                    matches!(g.as_deref(), Some(active) if active == jid.as_str())
                };
                if still_active {
                    eprintln!("[diskusage] scan completed job_id={}", jid);
                    {
                        let mut du = du_arc.blocking_write();
                        *du = Some(tree);
                    }
                    if let Ok(mut s) = scan_state.lock() {
                        if s.scan_id.as_deref() == Some(jid.as_str()) {
                            s.status = DiskScanStatus::Completed;
                            s.phase = DiskScanPhase::Done;
                            s.message = "Scan complete".to_string();
                            s.final_summary = Some(summary.clone());
                        }
                    }
                    let _ = app.emit("diskusage.complete", summary);
                } else {
                    eprintln!(
                        "[diskusage] dropped completion for replaced/cancelled job_id={}",
                        jid
                    );
                }
            }
            Ok(Err(e)) => {
                eprintln!("[diskusage] scan error job_id={} err={}", jid, e);
                if let Ok(mut s) = scan_state.lock() {
                    if matches!(s.status, DiskScanStatus::Scanning) {
                        s.status = if e == "Scan cancelled" {
                            DiskScanStatus::Cancelled
                        } else {
                            DiskScanStatus::Error
                        };
                        s.phase = DiskScanPhase::Done;
                        s.message = e.clone();
                    }
                }
                let _ = app.emit(
                    "diskusage.error",
                    serde_json::json!({ "job_id": jid, "error": e }),
                );
            }
            Err(panic) => {
                let msg = match panic.downcast_ref::<&str>() {
                    Some(s) => s.to_string(),
                    None => match panic.downcast_ref::<String>() {
                        Some(s) => s.clone(),
                        None => "scan panicked".to_string(),
                    },
                };
                eprintln!("[diskusage] panic: {msg}");
                if let Ok(mut s) = scan_state.lock() {
                    s.status = DiskScanStatus::Error;
                    s.phase = DiskScanPhase::Done;
                    s.message = format!("Scan crashed: {msg}");
                }
                let _ = app.emit(
                    "diskusage.error",
                    serde_json::json!({ "job_id": jid, "error": format!("Scan crashed: {msg}") }),
                );
            }
        }
        jobs.remove(&jid);
    });

    Ok(job_id)
}

#[command]
pub async fn cancel_disk_usage_scan(state: State<'_, AppState>) -> Result<bool, String> {
    let prior = {
        let mut g = state.disk_usage_job.write().await;
        g.take()
    };
    if let Some(id) = prior {
        eprintln!("[diskusage] cancel command received job_id={}", id);
        Ok(state.jobs.cancel(&id))
    } else {
        Ok(false)
    }
}

#[command]
pub async fn get_disk_usage_scan_state(
    state: State<'_, AppState>,
) -> Result<DiskUsageScanState, String> {
    let s = state
        .disk_usage_state
        .lock()
        .map_err(|e| format!("state lock poisoned: {e}"))?;
    Ok(s.clone())
}

#[command]
pub async fn get_disk_usage(
    path: String,
    state: State<'_, AppState>,
) -> Result<DiskUsageInfo, String> {
    let du = state.disk_usage.read().await;
    let tree = du.as_ref().ok_or("No disk usage data")?;
    tree.get_info(&path)
        .ok_or_else(|| format!("Path not found: {path}"))
}

#[command]
pub async fn preview_rename(
    paths: Vec<String>,
    rules: Vec<RenameRule>,
) -> Result<Vec<RenamePreviewItem>, String> {
    Ok(rename::preview_rename(&paths, &rules))
}

#[command]
pub async fn apply_rename(
    paths: Vec<String>,
    rules: Vec<RenameRule>,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let previews = rename::preview_rename(&paths, &rules);
    let has_issues = previews
        .iter()
        .any(|p| p.status == "error" || p.status == "conflict");
    if has_issues {
        return Err("Cannot apply: there are errors or conflicts".to_string());
    }
    let records = rename::apply_rename(&previews)?;
    let count = records.len();
    let mut history = state.rename_history.write().await;
    *history = Some(records);
    Ok(count)
}

#[command]
pub async fn undo_rename(state: State<'_, AppState>) -> Result<usize, String> {
    let mut history = state.rename_history.write().await;
    let records = history.take().ok_or("No rename to undo")?;
    let count = records.len();
    rename::undo_rename(&records)?;
    Ok(count)
}

#[command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    Ok(state.settings.read().await.clone())
}

#[command]
pub async fn save_settings(
    settings: Settings,
    state: State<'_, AppState>,
) -> Result<(), String> {
    crate::settings::save_settings(&settings)?;
    state
        .close_to_tray
        .store(settings.close_to_tray, std::sync::atomic::Ordering::Relaxed);
    let mut current = state.settings.write().await;
    *current = settings;
    Ok(())
}

#[command]
pub async fn get_cache_info() -> Result<Option<CacheInfo>, String> {
    Ok(cache::get_cache_info())
}

/// Outcome of a `load_cached_index` call. The frontend uses the discriminator
/// to decide whether Search is now ready, should remain idle, or should keep
/// showing the existing indexing state. A skipped load never replaces
/// `state.index`, so an active scan's eventual merge cannot be clobbered.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LoadCachedIndexResponse {
    Loaded { info: CacheInfo },
    SkippedIndexing,
    SkippedEmpty,
}

#[command]
pub async fn load_cached_index(
    state: State<'_, AppState>,
) -> Result<LoadCachedIndexResponse, String> {
    // Fast-path: refuse to overwrite `state.index` while a scan is in
    // flight. The frontend has its own gate, but on reload/HMR the
    // frontend can be idle while the backend is still indexing — only the
    // backend can answer reliably. Authoritative re-check happens under
    // `publish_guard` below.
    {
        let cur = state.current_index_job.read().await;
        if cur.is_some() {
            return Ok(LoadCachedIndexResponse::SkippedIndexing);
        }
    }

    // Restrict the loaded index to entries owned by currently-configured,
    // currently-enabled roots. A root that was removed (intentional delete)
    // or disabled (excluded from active search) doesn't get its old cached
    // entries surfaced after restart.
    let enabled_ids: std::collections::HashSet<String> = {
        let s = state.settings.read().await;
        s.indexed_roots
            .iter()
            .filter(|r| r.enabled)
            .map(|r| r.id.clone())
            .collect()
    };

    // Serialize against `clear_cache` (and the worker's publish path) for
    // the entire cache-read + in-memory commit window. Without this,
    // `clear_cache` could fire between our cache read and our state
    // mutation, deleting the file and wiping memory — and we would then
    // resurrect the just-cleared cache-derived state into memory, undoing
    // the user's clear.
    //
    // Lock order: publish_guard -> current_index_job -> index -> last_index
    // -> root_runtime. `clear_cache` and the worker publish path acquire
    // these in the same order, so there is no acquisition cycle.
    // `start_scan` does not take `publish_guard` (only an atomic swap of
    // `current_index_job`), so holding it here cannot stall scan
    // publication.
    let _publish_lock = state.publish_guard.lock().await;

    let (info, index) = cache::load_cache(Some(&enabled_ids))?;

    // Root filter wiped every entry (cache exists but no enabled root matches
    // any cached root_id). Don't flip state.index to an empty index — that
    // would surface a "ready" search with zero results. Leave the in-memory
    // index untouched and tell the caller it was a no-op.
    if index.len() == 0 {
        return Ok(LoadCachedIndexResponse::SkippedEmpty);
    }

    // Precompute per-root runtime hydration BEFORE acquiring the
    // `current_index_job` commit guard. `Path::exists` is a filesystem
    // probe that can stall for seconds on removable/network FS; keeping
    // it outside the commit guard preserves the original "don't block
    // start_scan publication on a slow probe" behavior. We're still under
    // `publish_guard`, which only blocks `clear_cache` and the worker —
    // `start_scan` is unaffected.
    let now = info.timestamp;
    let cached_by_id: std::collections::HashMap<String, u64> = info
        .root_meta
        .iter()
        .map(|m| (m.id.clone(), m.item_count))
        .collect();
    struct RootHydration {
        id: String,
        item_count: u64,
        reachable: bool,
    }
    let hydrations: Vec<RootHydration> = {
        let s = state.settings.read().await;
        s.indexed_roots
            .iter()
            .filter(|r| r.enabled)
            .filter_map(|r| {
                let count = *cached_by_id.get(&r.id)?;
                // Filesystem probe — kept OUTSIDE the commit guard.
                let reachable = Path::new(&r.path).exists();
                Some(RootHydration {
                    id: r.id.clone(),
                    item_count: count,
                    reachable,
                })
            })
            .collect()
    };

    // Atomic commit: hold `current_index_job` write across the re-check
    // and every state mutation below so a concurrent `start_scan` cannot
    // record a new job between our check and `*idx = index`. The guarded
    // section is strictly fast in-memory work — every disk/IO probe has
    // already happened above, so this hold is short.
    let cur_guard = state.current_index_job.write().await;
    if cur_guard.is_some() {
        return Ok(LoadCachedIndexResponse::SkippedIndexing);
    }
    {
        let mut idx = state.index.write().await;
        *idx = index;
    }
    {
        let mut last = state.last_index.write().await;
        *last = Some(IndexedSummary {
            roots: info.roots.clone(),
            timestamp: info.timestamp,
        });
    }
    {
        let mut runtime = state.root_runtime.write().await;
        for h in &hydrations {
            let entry = runtime.entry(h.id.clone()).or_default();
            entry.last_indexed = now;
            entry.item_count = h.item_count;
            if h.reachable {
                entry.state = RootState::Ready;
                entry.error = None;
            } else {
                entry.state = RootState::Missing;
                entry.error = Some("Path not accessible".to_string());
            }
        }
    }
    drop(cur_guard);
    drop(_publish_lock);
    Ok(LoadCachedIndexResponse::Loaded { info })
}

#[command]
pub async fn clear_cache(state: State<'_, AppState>) -> Result<(), String> {
    // Serialize against the worker's publish/save critical section. Without
    // this, a worker that already passed its `still_active` re-check could
    // call `cache::save_cache` AFTER we delete the cache file below, leaving
    // stale data on disk that startup hydration would resurrect on the next
    // launch. Holding `publish_guard` for the whole wipe (cancel job +
    // delete file + clear in-memory) makes the two paths mutually exclusive.
    let _publish_lock = state.publish_guard.lock().await;

    // Cancel any in-flight indexing FIRST. Without this, a scan that's
    // about to finish would later see itself as the active job, save a
    // freshly merged index back to disk, and overwrite the runtime
    // FileIndex — silently resurrecting the data the user just cleared.
    // Take the active slot to None so the worker's `still_active` check
    // (re-validated under `index.write` below) fails, and cancel the
    // CancellationToken so anything still walking exits early.
    //
    // Lock order: publish_guard -> current_index_job -> index -> last_index
    // -> root_runtime. The worker takes publish_guard before any of the
    // other locks too, so there is no acquisition cycle.
    let cancelled = {
        let mut g = state.current_index_job.write().await;
        g.take()
    };
    if let Some(prev) = &cancelled {
        state.jobs.cancel(prev);
    }
    cache::clear_cache()?;
    // Drop the in-memory index too. Without this, Search/Disk Usage keep
    // serving the previously-loaded entries until restart even though the
    // on-disk cache is gone — the persistence is wiped but the runtime
    // copy lives on, and the two views disagree.
    {
        let mut idx = state.index.write().await;
        *idx = crate::index::FileIndex::default();
    }
    {
        let mut last = state.last_index.write().await;
        *last = None;
    }
    let mut runtime = state.root_runtime.write().await;
    for entry in runtime.values_mut() {
        entry.state = RootState::Idle;
        entry.item_count = 0;
        entry.last_indexed = 0;
        entry.error = None;
    }
    Ok(())
}

#[command]
pub async fn open_path(path: String) -> Result<(), String> {
    eprintln!("[open_path] requested {path:?}");
    let p = Path::new(&path);
    if !p.exists() {
        eprintln!("[open_path] does not exist on disk: {path}");
        return Err(format!(
            "File not found on disk: {path}\nThe index may be stale or the drive may be disconnected."
        ));
    }
    // Open the EXACT selected path. Canonicalization would silently swap a
    // symlinked search result for its target, which violates "open the path
    // the user actually picked". Canonical form is computed only as a
    // diagnostic breadcrumb in logs.
    match dunce::canonicalize(p) {
        Ok(c) => eprintln!(
            "[open_path] opening exact={path:?} (canonical={:?})",
            c.to_string_lossy()
        ),
        Err(_) => eprintln!("[open_path] opening exact={path:?}"),
    }
    let open_target = path.clone();
    let r = tokio::task::spawn_blocking(move || spawn_open(&open_target))
        .await
        .map_err(|e| format!("desktop opener task failed: {e}"))?;
    match &r {
        Ok(()) => eprintln!("[open_path] spawn ok for {path:?}"),
        Err(e) => eprintln!("[open_path] spawn err for {path:?}: {e}"),
    }
    r
}

#[command]
pub async fn reveal_in_folder(path: String) -> Result<(), String> {
    eprintln!("[reveal_in_folder] requested {path:?}");
    let p = Path::new(&path);
    if !p.exists() {
        eprintln!("[reveal_in_folder] does not exist on disk: {path}");
        return Err(format!(
            "Path not found on disk: {path}\nThe index may be stale or the drive may be disconnected."
        ));
    }
    let target_path = if p.is_file() {
        p.parent().map(|x| x.to_path_buf()).unwrap_or_else(|| p.to_path_buf())
    } else {
        p.to_path_buf()
    };
    let target = target_path.to_string_lossy().to_string();
    let canonical = dunce::canonicalize(&target_path)
        .map(|c| c.to_string_lossy().to_string())
        .unwrap_or_else(|_| target.clone());
    eprintln!("[reveal_in_folder] opening canonical={canonical:?} (input={path:?})");
    let r = tokio::task::spawn_blocking(move || spawn_open(&canonical))
        .await
        .map_err(|e| format!("desktop opener task failed: {e}"))?;
    if let Err(e) = &r {
        eprintln!("[reveal_in_folder] spawn err: {e}");
    }
    r
}

/// Hand a path off to the OS shell to "open" it.
///
/// On Linux/macOS the path is forwarded as a single argv element to
/// `xdg-open` / `open` — never shell-concatenated, so spaces, parentheses,
/// dashes, Unicode, and shell metacharacters all round-trip verbatim. On
/// Linux the helper is moved into a fresh process group with stdio
/// redirected to /dev/null; without that, the grandchild (mpv, dolphin,
/// etc.) inherits Tauri's pipes and process group, which is the failure
/// mode that shows as "the app flashes open then closes immediately".
///
/// On Windows the path goes straight through `ShellExecuteW` (Win32) so
/// `cmd.exe` never sees it — that closes the metacharacter-injection hole
/// that `cmd /C start "" <path>` left open.
#[cfg(target_os = "linux")]
fn spawn_open(target: &str) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new("xdg-open");
    cmd.arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    clean_appimage_env(&mut cmd);

    let child = cmd
        .spawn()
        .map_err(|e| format!("xdg-open failed for {target:?}: {e}"))?;
    wait_for_linux_launcher(child, target)
}

#[cfg(target_os = "linux")]
fn clean_appimage_env(cmd: &mut std::process::Command) {
    if std::env::var_os("APPIMAGE").is_none() {
        return;
    }

    for key in [
        "LD_LIBRARY_PATH",
        "LD_PRELOAD",
        "PYTHONPATH",
        "GTK_DATA_PREFIX",
        "GTK_THEME",
        "GDK_BACKEND",
        "GSETTINGS_SCHEMA_DIR",
        "GTK_EXE_PREFIX",
        "GTK_IM_MODULE_FILE",
        "GDK_PIXBUF_MODULE_FILE",
        "GDK_PIXBUF_MODULEDIR",
        "GIO_EXTRA_MODULES",
    ] {
        restore_or_remove_env(cmd, key);
    }

    let appdir = std::env::var_os("APPDIR").map(std::path::PathBuf::from);
    for key in ["XDG_DATA_DIRS", "GTK_PATH"] {
        let original_key = format!("APPIMAGE_ORIGINAL_{key}");
        if let Some(original) = std::env::var_os(original_key) {
            cmd.env(key, original);
        } else if let (Some(value), Some(appdir)) = (std::env::var_os(key), appdir.as_deref()) {
            match without_appdir_paths(&value, appdir) {
                Some(clean) if !clean.is_empty() => {
                    cmd.env(key, clean);
                }
                _ => {
                    cmd.env_remove(key);
                }
            }
        } else {
            cmd.env_remove(key);
        }
    }

    for key in ["APPIMAGE", "APPDIR", "APPIMAGE_GTK_THEME"] {
        cmd.env_remove(key);
    }
}

#[cfg(target_os = "linux")]
fn restore_or_remove_env(cmd: &mut std::process::Command, key: &str) {
    if let Some(original) = std::env::var_os(format!("APPIMAGE_ORIGINAL_{key}")) {
        cmd.env(key, original);
    } else {
        cmd.env_remove(key);
    }
}

#[cfg(target_os = "linux")]
fn without_appdir_paths(
    value: &std::ffi::OsStr,
    appdir: &std::path::Path,
) -> Option<std::ffi::OsString> {
    std::env::join_paths(std::env::split_paths(value).filter(|entry| !entry.starts_with(appdir)))
        .ok()
}

#[cfg(target_os = "linux")]
fn wait_for_linux_launcher(mut child: std::process::Child, target: &str) -> Result<(), String> {
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_millis(750);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(format!("xdg-open failed for {target:?} with {status}"));
            }
            Err(e) => return Err(format!("could not monitor xdg-open for {target:?}: {e}")),
            Ok(None) if Instant::now() >= deadline => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_open_tests {
    use super::{wait_for_linux_launcher, without_appdir_paths};
    use std::ffi::OsStr;
    use std::path::Path;
    use std::process::Command;

    #[test]
    fn removes_only_appimage_entries_from_search_paths() {
        let clean = without_appdir_paths(
            OsStr::new("/tmp/.mount_Cove/usr/share:/usr/share:/home/user/.local/share"),
            Path::new("/tmp/.mount_Cove"),
        )
        .unwrap();

        assert_eq!(clean, OsStr::new("/usr/share:/home/user/.local/share"));
    }

    #[test]
    fn reports_an_immediate_launcher_failure() {
        let child = Command::new("sh").args(["-c", "exit 17"]).spawn().unwrap();

        let error = wait_for_linux_launcher(child, "/tmp/example").unwrap_err();
        assert!(error.contains("exit status: 17"), "{error}");
    }

    #[test]
    fn accepts_a_successful_launcher_handoff() {
        let child = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();

        assert!(wait_for_linux_launcher(child, "/tmp/example").is_ok());
    }
}

#[cfg(target_os = "macos")]
fn spawn_open(target: &str) -> Result<(), String> {
    use std::process::{Command, Stdio};
    Command::new("open")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("open failed for {target:?}: {e}"))
}

#[cfg(target_os = "windows")]
fn spawn_open(target: &str) -> Result<(), String> {
    // Avoid cmd.exe / `start`: cmd parses shell metacharacters (`&`, `|`,
    // `^`, etc.) in the command line even when args are passed as separate
    // argv elements, so a crafted filename could chain commands. Use the
    // Win32 shell API directly — it takes a single wide-string path and
    // never invokes a shell parser.
    use std::ffi::OsStr;
    use std::iter::once;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide_path: Vec<u16> = OsStr::new(target).encode_wide().chain(once(0)).collect();
    let verb: Vec<u16> = OsStr::new("open").encode_wide().chain(once(0)).collect();

    // SAFETY: pointers reference NUL-terminated wide strings owned by
    // local Vecs that outlive the call. ShellExecuteW does not retain them.
    let hinstance = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            verb.as_ptr(),
            wide_path.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    };

    // Per Win32 docs, ShellExecuteW returns > 32 on success; <= 32 encodes
    // an error code (e.g. SE_ERR_FNF, SE_ERR_NOASSOC).
    let code = hinstance as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW failed for {target:?}: code {code}"))
    }
}

/// Re-walk a single directory subtree from the live filesystem and return the
/// same DiskUsageInfo shape as the initial scan — recursive sizes, extensions,
/// and largest_files included. Used after mutations (copy/move/trash/rename)
/// so the table, treemap, and side panels can refresh without a full rescan.
#[command]
pub async fn rescan_disk_dir(
    path: String,
    state: State<'_, AppState>,
) -> Result<DiskUsageInfo, String> {
    use tokio_util::sync::CancellationToken;
    let dir = Path::new(&path);
    if !dir.is_dir() {
        return Err(format!("Not a directory: {}", path));
    }
    let excluded = state.settings.read().await.excluded_patterns.clone();
    let path_owned = path.clone();
    let tree = tokio::task::spawn_blocking(move || {
        let token = CancellationToken::new();
        diskusage::scan_disk_usage_inner(&path_owned, token, &excluded, |_| {})
            .map(|(tree, _, _)| tree)
    })
    .await
    .map_err(|e| format!("Rescan task failed: {e}"))??;
    tree.get_info(&path)
        .ok_or_else(|| format!("Path not found after rescan: {path}"))
}

// ---------- File operations (Disk Usage context menu) ----------

// Linux: the `trash` crate had mount-point detection issues, so use a custom
// freedesktop trash implementation. This path is unchanged from prior releases.
#[cfg(target_os = "linux")]
#[command]
pub async fn move_to_trash(paths: Vec<String>) -> Result<(), String> {
    for p in &paths {
        freedesktop_trash_fallback(Path::new(p))
            .map_err(|e| format!("Trash failed for '{}': {}", p, e))?;
    }
    Ok(())
}

// Windows/macOS: the `trash` crate calls the native Shell API (Windows Recycle
// Bin / macOS Trash) and works without the mount-point issues seen on Linux.
#[cfg(not(target_os = "linux"))]
#[command]
pub async fn move_to_trash(paths: Vec<String>) -> Result<(), String> {
    trash::delete_all(&paths).map_err(|e| format!("Trash failed: {}", e))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn freedesktop_trash_fallback(path: &Path) -> Result<(), String> {
    let trash_dir = dirs::data_dir()
        .ok_or("Cannot determine XDG data directory")?
        .join("Trash");
    let files_dir = trash_dir.join("files");
    let info_dir = trash_dir.join("info");
    std::fs::create_dir_all(&files_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&info_dir).map_err(|e| e.to_string())?;

    let name = path
        .file_name()
        .ok_or("No filename")?
        .to_string_lossy()
        .to_string();
    let abs = std::fs::canonicalize(path).map_err(|e| e.to_string())?;

    let mut dest_name = name.clone();
    let mut counter = 1u32;
    while files_dir.join(&dest_name).exists() {
        let stem = Path::new(&name)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        let ext = Path::new(&name).extension().map(|e| e.to_string_lossy().to_string());
        dest_name = match ext {
            Some(e) => format!("{}.{}.{}", stem, counter, e),
            None => format!("{}.{}", stem, counter),
        };
        counter += 1;
    }

    let deletion_date = format_trash_date();
    let info_content = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        abs.display(),
        deletion_date
    );
    std::fs::write(
        info_dir.join(format!("{}.trashinfo", dest_name)),
        info_content,
    )
    .map_err(|e| e.to_string())?;

    let dest = files_dir.join(&dest_name);
    if let Err(e) = std::fs::rename(path, &dest) {
        if e.raw_os_error() == Some(18) {
            // EXDEV: cross-device link - copy then remove
            if path.is_dir() {
                copy_dir_recursive(path, &dest)?;
                std::fs::remove_dir_all(path).map_err(|e| e.to_string())?;
            } else {
                std::fs::copy(path, &dest).map_err(|e| e.to_string())?;
                std::fs::remove_file(path).map_err(|e| e.to_string())?;
            }
        } else {
            return Err(e.to_string());
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn format_trash_date() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (secs / 86400) as i64;
    let day_secs = (secs % 86400) as i64;
    let h = day_secs / 3600;
    let m = (day_secs % 3600) / 60;
    let s = day_secs % 60;
    // Civil date from unix days (algorithm from Howard Hinnant)
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}")
}

#[command]
pub async fn delete_permanently(paths: Vec<String>) -> Result<(), String> {
    for p in &paths {
        let path = Path::new(p);
        if path.is_dir() {
            std::fs::remove_dir_all(path)
                .map_err(|e| format!("Delete failed for '{}': {}", p, e))?;
        } else {
            std::fs::remove_file(path)
                .map_err(|e| format!("Delete failed for '{}': {}", p, e))?;
        }
    }
    Ok(())
}

#[command]
pub async fn rename_path(from: String, to: String) -> Result<(), String> {
    let dest = Path::new(&to);
    if dest.exists() {
        return Err(format!("Destination already exists: {}", to));
    }
    std::fs::rename(&from, &to).map_err(|e| format!("Rename failed: {}", e))
}

#[command]
pub async fn copy_paths(srcs: Vec<String>, dest_dir: String) -> Result<(), String> {
    let dest = Path::new(&dest_dir);
    if !dest.is_dir() {
        return Err(format!("Destination is not a directory: {}", dest_dir));
    }
    for src in &srcs {
        let src_path = Path::new(src);
        let name = src_path
            .file_name()
            .ok_or_else(|| format!("Cannot determine filename for '{}'", src))?;
        let mut target = dest.join(name);
        target = resolve_collision(target);
        if src_path.is_dir() {
            copy_dir_recursive(src_path, &target)?;
        } else {
            std::fs::copy(src_path, &target)
                .map_err(|e| format!("Copy failed for '{}': {}", src, e))?;
        }
    }
    Ok(())
}

#[command]
pub async fn move_paths(srcs: Vec<String>, dest_dir: String) -> Result<(), String> {
    let dest = Path::new(&dest_dir);
    if !dest.is_dir() {
        return Err(format!("Destination is not a directory: {}", dest_dir));
    }
    let dest_canon = dunce::canonicalize(dest).ok();
    for src in &srcs {
        let src_path = Path::new(src);
        let name = src_path
            .file_name()
            .ok_or_else(|| format!("Cannot determine filename for '{}'", src))?;
        // Same-folder move is a no-op. Without this, the would-be target
        // already exists (it's the source itself), resolve_collision picks
        // "name (copy).ext", and the rename silently changes the filename.
        let src_parent_canon = src_path.parent().and_then(|p| dunce::canonicalize(p).ok());
        if dest_canon.is_some() && src_parent_canon == dest_canon {
            continue;
        }
        let mut target = dest.join(name);
        target = resolve_collision(target);
        match std::fs::rename(src_path, &target) {
            Ok(()) => {}
            Err(e) if is_cross_device(&e) => {
                if src_path.is_dir() {
                    copy_dir_recursive(src_path, &target)?;
                    std::fs::remove_dir_all(src_path)
                        .map_err(|e| format!("Remove after cross-device copy failed: {}", e))?;
                } else {
                    std::fs::copy(src_path, &target)
                        .map_err(|e| format!("Cross-device copy failed: {}", e))?;
                    std::fs::remove_file(src_path)
                        .map_err(|e| format!("Remove after cross-device copy failed: {}", e))?;
                }
            }
            Err(e) => return Err(format!("Move failed for '{}': {}", src, e)),
        }
    }
    Ok(())
}

fn is_cross_device(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    { e.raw_os_error() == Some(libc::EXDEV) }
    #[cfg(windows)]
    { e.raw_os_error() == Some(17) } // ERROR_NOT_SAME_DEVICE
    #[cfg(not(any(unix, windows)))]
    { false }
}

fn resolve_collision(mut target: std::path::PathBuf) -> std::path::PathBuf {
    if !target.exists() {
        return target;
    }
    let stem = target
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let ext = target
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let parent = target.parent().unwrap().to_path_buf();
    let first = parent.join(format!("{} (copy){}", stem, ext));
    if !first.exists() {
        return first;
    }
    for i in 2..1000 {
        let candidate = parent.join(format!("{} (copy {}){}", stem, i, ext));
        if !candidate.exists() {
            return candidate;
        }
    }
    target.set_file_name(format!("{}_dup{}", stem, ext));
    target
}

fn check_not_descendant(src: &Path, dst: &Path) -> Result<(), String> {
    let src_canon = dunce::canonicalize(src)
        .unwrap_or_else(|_| src.to_path_buf());
    let dst_canon = dunce::canonicalize(dst.parent().unwrap_or(dst))
        .unwrap_or_else(|_| dst.to_path_buf());
    if dst_canon.starts_with(&src_canon) {
        return Err(format!(
            "Cannot copy '{}' into itself or a subdirectory of itself",
            src.display()
        ));
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    check_not_descendant(src, dst)?;
    std::fs::create_dir_all(dst)
        .map_err(|e| format!("Cannot create dir '{}': {}", dst.display(), e))?;
    for entry in std::fs::read_dir(src)
        .map_err(|e| format!("Cannot read dir '{}': {}", src.display(), e))?
    {
        let entry = entry.map_err(|e| format!("Dir entry error: {}", e))?;
        let target = dst.join(entry.file_name());
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)
                .map_err(|e| format!("Copy failed: {}", e))?;
        }
    }
    Ok(())
}

// keep IndexRootRuntime referenced so the public type is exported.
#[allow(dead_code)]
fn _phantom(_x: IndexRootRuntime) {}
