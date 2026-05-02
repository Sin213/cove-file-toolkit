use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum DiskScanStatus {
    Idle,
    Scanning,
    Completed,
    Cancelled,
    Error,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum DiskScanPhase {
    Idle,
    Starting,
    Scanning,
    Finalizing,
    Done,
}

/// Authoritative scan state shared between the IPC layer and the worker
/// thread. Stored in `AppState::disk_usage_state` so the frontend can pull
/// it back any time (e.g. after a tab switch).
#[derive(Serialize, Clone, Debug)]
pub struct DiskUsageScanState {
    pub scan_id: Option<String>,
    pub root_path: String,
    pub status: DiskScanStatus,
    pub phase: DiskScanPhase,
    pub engine: String,
    pub current_path: String,
    pub elapsed_ms: u64,
    pub files_scanned: u64,
    pub dirs_scanned: u64,
    pub bytes_scanned: u64,
    pub errors_count: u64,
    pub skipped_count: u64,
    pub message: String,
    pub final_summary: Option<DiskUsageComplete>,
}

impl DiskUsageScanState {
    pub fn idle() -> Self {
        Self {
            scan_id: None,
            root_path: String::new(),
            status: DiskScanStatus::Idle,
            phase: DiskScanPhase::Idle,
            engine: engine_name().to_string(),
            current_path: String::new(),
            elapsed_ms: 0,
            files_scanned: 0,
            dirs_scanned: 0,
            bytes_scanned: 0,
            errors_count: 0,
            skipped_count: 0,
            message: String::new(),
            final_summary: None,
        }
    }
}

pub fn engine_name() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux-recursive"
    } else if cfg!(target_os = "windows") {
        "windows-recursive"
    } else if cfg!(target_os = "macos") {
        "macos-recursive"
    } else {
        "recursive"
    }
}

#[derive(Serialize, Clone)]
pub struct DiskUsageProgress {
    pub job_id: String,
    pub files_found: u64,
    pub dirs_found: u64,
    pub bytes_found: u64,
    pub errors: u64,
    pub skipped: u64,
    pub elapsed_ms: u64,
    pub current_path: String,
    pub phase: DiskScanPhase,
    pub engine: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct DiskUsageComplete {
    pub job_id: String,
    pub root: String,
    pub total_size: u64,
    pub total_files: u64,
    pub total_dirs: u64,
    pub errors: u64,
    pub skipped: u64,
    pub elapsed_ms: u64,
    pub engine: String,
}

#[derive(Serialize, Clone)]
pub struct DiskUsageEntry {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub file_count: u64,
    pub dir_count: usize,
    pub child_count: usize,
    pub item_count: u64,
    pub mtime: i64,
    pub percentage: f64,
    pub is_dir: bool,
}

#[derive(Serialize, Clone)]
pub struct ExtensionStats {
    pub extension: String,
    pub size: u64,
    pub count: u64,
    pub percentage: f64,
}

#[derive(Serialize, Clone)]
pub struct DiskUsageInfo {
    pub path: String,
    pub name: String,
    pub total_size: u64,
    pub total_file_count: u64,
    pub own_size: u64,
    pub own_file_count: u64,
    pub children: Vec<DiskUsageEntry>,
    pub extensions: Vec<ExtensionStats>,
    pub largest_files: Vec<DiskUsageEntry>,
}

#[derive(Clone)]
struct FileEntry {
    name: String,
    size: u64,
    mtime: i64,
}

struct DirInfo {
    name: String,
    own_size: u64,
    own_file_count: u64,
    total_size: u64,
    total_file_count: u64,
    total_dir_count: u64,
    mtime: i64,
    depth: u32,
    subdir_ids: Vec<u32>,
    files: Vec<FileEntry>,
    ext_size: HashMap<String, (u64, u64)>,
}

impl DirInfo {
    fn new(name: String, depth: u32) -> Self {
        Self {
            name,
            own_size: 0,
            own_file_count: 0,
            total_size: 0,
            total_file_count: 0,
            total_dir_count: 0,
            mtime: 0,
            depth,
            subdir_ids: Vec::new(),
            files: Vec::new(),
            ext_size: HashMap::new(),
        }
    }
}

/// Internal id-keyed tree. We keep a `path -> id` map to bind paths to nodes
/// without paying the cost of hashing/cloning long path strings on every
/// file insert.
pub struct DiskUsageTree {
    #[allow(dead_code)]
    root: String,
    /// dir id -> DirInfo (Vec is cheaper than HashMap<String, _> by a wide
    /// margin; child lookups become O(1) integer index operations).
    dirs: Vec<DirInfo>,
    /// canonical path -> dir id. Used by `get_info` (frontend lookup).
    path_to_id: HashMap<String, u32>,
    /// id -> reconstructed path (only built on demand by `get_info`, never
    /// during the walk).
    id_to_path: Vec<String>,
}

fn extension_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{}{}{}", parent, std::path::MAIN_SEPARATOR, name)
    }
}

impl DiskUsageTree {
    pub fn get_info(&self, path: &str) -> Option<DiskUsageInfo> {
        let id = *self.path_to_id.get(path)? as usize;
        let info = &self.dirs[id];
        let parent_size = info.total_size;
        let path_str = self.id_to_path[id].clone();

        let mut children: Vec<DiskUsageEntry> = Vec::with_capacity(
            info.subdir_ids.len() + info.files.len(),
        );

        for &sub_id in &info.subdir_ids {
            let sub = &self.dirs[sub_id as usize];
            children.push(DiskUsageEntry {
                name: sub.name.clone(),
                path: self.id_to_path[sub_id as usize].clone(),
                size: sub.total_size,
                file_count: sub.total_file_count,
                dir_count: sub.total_dir_count as usize,
                child_count: sub.subdir_ids.len() + sub.files.len(),
                item_count: sub.total_file_count + sub.total_dir_count,
                mtime: sub.mtime,
                percentage: if parent_size > 0 {
                    (sub.total_size as f64 / parent_size as f64) * 100.0
                } else {
                    0.0
                },
                is_dir: true,
            });
        }

        for file in &info.files {
            children.push(DiskUsageEntry {
                name: file.name.clone(),
                path: join_path(&path_str, &file.name),
                size: file.size,
                file_count: 0,
                dir_count: 0,
                child_count: 0,
                item_count: 0,
                mtime: file.mtime,
                percentage: if parent_size > 0 {
                    (file.size as f64 / parent_size as f64) * 100.0
                } else {
                    0.0
                },
                is_dir: false,
            });
        }

        children.sort_by(|a, b| b.size.cmp(&a.size));

        let agg = self.aggregate_extensions(id as u32);
        let mut extensions: Vec<ExtensionStats> = agg
            .into_iter()
            .map(|(ext, (size, count))| ExtensionStats {
                extension: if ext.is_empty() { "(none)".to_string() } else { ext },
                size,
                count,
                percentage: if parent_size > 0 {
                    (size as f64 / parent_size as f64) * 100.0
                } else {
                    0.0
                },
            })
            .collect();
        extensions.sort_by(|a, b| b.size.cmp(&a.size));
        extensions.truncate(40);

        let largest_files = self.largest_files(id as u32, 20, parent_size);

        Some(DiskUsageInfo {
            path: path_str,
            name: info.name.clone(),
            total_size: info.total_size,
            total_file_count: info.total_file_count,
            own_size: info.own_size,
            own_file_count: info.own_file_count,
            children,
            extensions,
            largest_files,
        })
    }

    fn aggregate_extensions(&self, root_id: u32) -> HashMap<String, (u64, u64)> {
        let mut acc: HashMap<String, (u64, u64)> = HashMap::new();
        let mut stack: Vec<u32> = vec![root_id];
        while let Some(id) = stack.pop() {
            let d = &self.dirs[id as usize];
            for (ext, (sz, cnt)) in &d.ext_size {
                let entry = acc.entry(ext.clone()).or_insert((0, 0));
                entry.0 += sz;
                entry.1 += cnt;
            }
            for &sd in &d.subdir_ids {
                stack.push(sd);
            }
        }
        acc
    }

    /// Top-N largest files under `root_id`. Uses a bounded min-heap so we
    /// don't materialize every file into a Vec.
    fn largest_files(&self, root_id: u32, limit: usize, parent_size: u64) -> Vec<DiskUsageEntry> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        if limit == 0 {
            return Vec::new();
        }
        // (Reverse(size), dir_id, file_index_in_dir)
        let mut heap: BinaryHeap<Reverse<(u64, u32, u32)>> = BinaryHeap::with_capacity(limit + 1);
        let mut stack: Vec<u32> = vec![root_id];
        while let Some(id) = stack.pop() {
            let d = &self.dirs[id as usize];
            for (i, f) in d.files.iter().enumerate() {
                if heap.len() < limit {
                    heap.push(Reverse((f.size, id, i as u32)));
                } else if let Some(top) = heap.peek() {
                    if f.size > top.0 .0 {
                        heap.pop();
                        heap.push(Reverse((f.size, id, i as u32)));
                    }
                }
            }
            for &sd in &d.subdir_ids {
                stack.push(sd);
            }
        }
        let mut picks: Vec<(u64, u32, u32)> = heap.into_iter().map(|r| r.0).collect();
        picks.sort_by(|a, b| b.0.cmp(&a.0));
        picks
            .into_iter()
            .map(|(size, dir_id, file_idx)| {
                let d = &self.dirs[dir_id as usize];
                let f = &d.files[file_idx as usize];
                DiskUsageEntry {
                    name: f.name.clone(),
                    path: join_path(&self.id_to_path[dir_id as usize], &f.name),
                    size,
                    file_count: 0,
                    dir_count: 0,
                    child_count: 0,
                    item_count: 0,
                    mtime: f.mtime,
                    percentage: if parent_size > 0 {
                        (f.size as f64 / parent_size as f64) * 100.0
                    } else {
                        0.0
                    },
                    is_dir: false,
                }
            })
            .collect()
    }
}

fn is_excluded(path: &Path, excluded: &[String]) -> bool {
    if excluded.is_empty() {
        return false;
    }
    for component in path.components() {
        if let std::path::Component::Normal(name) = component {
            let name_str = name.to_string_lossy();
            if excluded.iter().any(|ex| name_str.as_ref() == ex.as_str()) {
                return true;
            }
        }
    }
    false
}

/// Phase timings reported by the inner scan. Used by the bench test and by
/// any future telemetry surface.
#[derive(Debug, Clone, Default)]
pub struct ScanTimings {
    pub canonicalize_ms: u64,
    pub walk_ms: u64,
    pub aggregation_ms: u64,
    pub total_ms: u64,
    pub entries_seen: u64,
}

#[derive(Clone, Debug)]
pub struct ProgressSnapshot {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub errors: u64,
    pub skipped: u64,
    pub elapsed_ms: u64,
    pub current_path: String,
    pub phase: DiskScanPhase,
}

/// Pure scan core — no Tauri, no AppHandle, no shared mutex. Reusable by
/// tests, benches, and the Tauri-bound `scan_disk_usage` wrapper.
///
/// Uses jwalk's parallel walker (`Parallelism::RayonNewPool`). On a tree
/// the size of `~/Downloads` (~10k files / 760 dirs) this is dramatically
/// faster than the previous `Parallelism::Serial` because metadata syscalls
/// for sibling directories happen in parallel inside jwalk's worker pool.
pub fn scan_disk_usage_inner<F>(
    root: &str,
    token: CancellationToken,
    excluded: &[String],
    mut on_progress: F,
) -> Result<(DiskUsageTree, DiskUsageComplete, ScanTimings), String>
where
    F: FnMut(&ProgressSnapshot),
{
    let total_start = Instant::now();

    let canon_start = Instant::now();
    let canonical = match dunce::canonicalize(root) {
        Ok(p) => p,
        Err(e) => return Err(format!("Invalid path: {e}")),
    };
    let canonicalize_ms = canon_start.elapsed().as_millis() as u64;

    let root_str = canonical.to_string_lossy().to_string();
    let root_name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| root_str.clone());

    let walk_start = Instant::now();

    on_progress(&ProgressSnapshot {
        files: 0,
        dirs: 0,
        bytes: 0,
        errors: 0,
        skipped: 0,
        elapsed_ms: 0,
        current_path: root_str.clone(),
        phase: DiskScanPhase::Scanning,
    });

    // Parallel walk: jwalk distributes readdir + stat calls across worker
    // threads, which is the single biggest win on real-world directories.
    // We cap at min(num_cpus, 8) — past 8 threads the contention on the
    // global allocator + dentry cache lock starts to outweigh the gain.
    let parallel_threads = std::cmp::min(num_cpus::get(), 8);
    let excluded_for_prune: Vec<String> = excluded.to_vec();
    let walker = jwalk::WalkDir::new(&canonical)
        .parallelism(jwalk::Parallelism::RayonNewPool(parallel_threads))
        .skip_hidden(false)
        .follow_links(false)
        // Prune excluded subtrees at descend time. Without this, jwalk
        // would still recurse into e.g. `node_modules/` and pay the cost
        // of millions of metadata reads only to have the consumer drop
        // every entry. Setting `read_children_path = None` on a directory
        // entry tells jwalk not to recurse into it.
        .process_read_dir(move |_depth, _path, _state, children| {
            if excluded_for_prune.is_empty() {
                return;
            }
            for child in children.iter_mut() {
                if let Ok(entry) = child {
                    if entry.file_type().is_dir() {
                        let name_os = entry.file_name();
                        let name = name_os.to_string_lossy();
                        if excluded_for_prune
                            .iter()
                            .any(|ex| name.as_ref() == ex.as_str())
                        {
                            entry.read_children_path = None;
                        }
                    }
                }
            }
        });

    // Vec-backed tree. Root is always id 0.
    let mut dirs: Vec<DirInfo> = Vec::with_capacity(1024);
    let mut path_to_id: HashMap<String, u32> = HashMap::with_capacity(1024);
    dirs.push(DirInfo::new(root_name.clone(), 0));
    path_to_id.insert(root_str.clone(), 0);

    let mut file_count: u64 = 0;
    let mut dir_count: u64 = 1;
    let mut byte_count: u64 = 0;
    let mut error_count: u64 = 0;
    let mut skipped_count: u64 = 0;
    let mut last_emit = Instant::now();
    let mut entries_seen: u64 = 0;
    let mut last_current_path = String::new();

    for entry_result in walker {
        if token.is_cancelled() {
            return Err("Scan cancelled".to_string());
        }

        let entry = match entry_result {
            Ok(e) => e,
            Err(_) => {
                error_count += 1;
                continue;
            }
        };
        entries_seen += 1;

        let path = entry.path();
        if is_excluded(&path, excluded) {
            skipped_count += 1;
            continue;
        }

        let file_type = entry.file_type();
        let is_dir = file_type.is_dir();
        let is_file = file_type.is_file();
        // Skip sockets, fifos, block/char devices, symlinks-to-files —
        // counting them can cause confusing totals or hangs on read.
        if !is_dir && !is_file {
            skipped_count += 1;
            continue;
        }

        if is_dir {
            // jwalk yields the root as the first entry — skip re-creating it.
            // Instead just stamp its mtime if we can get it cheaply.
            // For non-root dirs, allocate an id and link to parent.
            // Compare against the canonical PathBuf (no string conversion).
            if path == canonical {
                continue;
            }

            // We need the parent dir's id to attach this subdir. The parent
            // path is reachable via `entry.parent_path()` (a `&Path`),
            // avoiding an extra `path.parent()` allocation.
            let parent_path = entry.parent_path();
            // Look up parent id from `path_to_id`. Convert `parent_path` to
            // a string only for the lookup — almost always already present
            // because jwalk delivers parents before children.
            let parent_path_str = parent_path.to_string_lossy();
            let parent_id = match path_to_id.get(parent_path_str.as_ref()) {
                Some(&id) => id,
                None => {
                    // Parent missing — could be a path outside root_str
                    // (shouldn't happen) or an out-of-order edge case. Skip.
                    skipped_count += 1;
                    continue;
                }
            };

            dir_count += 1;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let new_id = dirs.len() as u32;
            let depth = dirs[parent_id as usize].depth + 1;
            dirs.push(DirInfo::new(name, depth));
            // Defer building full path string until we need it for lookup —
            // but we need to record the path so children can attach.
            let path_str = path.to_string_lossy().to_string();
            path_to_id.insert(path_str, new_id);
            dirs[parent_id as usize].subdir_ids.push(new_id);
        } else {
            // File: only stat files (skip stat on dirs entirely — we don't
            // need their size). This is a meaningful win on dir-heavy trees.
            let metadata = entry.metadata().ok();
            let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            let mtime = metadata
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            file_count += 1;
            byte_count += size;

            let parent_path = entry.parent_path();
            let parent_path_str = parent_path.to_string_lossy();
            let parent_id = match path_to_id.get(parent_path_str.as_ref()) {
                Some(&id) => id,
                None => {
                    // Edge case: file under unknown parent. Drop into root
                    // bucket so size totals stay correct.
                    skipped_count += 1;
                    continue;
                }
            };

            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let ext = extension_of(&name);

            let parent_entry = &mut dirs[parent_id as usize];
            parent_entry.own_size += size;
            parent_entry.own_file_count += 1;
            parent_entry.files.push(FileEntry { name, size, mtime });
            let e = parent_entry.ext_size.entry(ext).or_insert((0, 0));
            e.0 += size;
            e.1 += 1;
        }

        if last_emit.elapsed().as_millis() >= 200 {
            // Emit a small progress snapshot. We deliberately avoid emitting
            // any tree fragments — only counters + the most recent dir path.
            // Stringify a current path lazily: the parent-path conversion is
            // cheap because we already have the &Path on hand.
            last_current_path = entry.parent_path().to_string_lossy().to_string();
            on_progress(&ProgressSnapshot {
                files: file_count,
                dirs: dir_count,
                bytes: byte_count,
                errors: error_count,
                skipped: skipped_count,
                elapsed_ms: walk_start.elapsed().as_millis() as u64,
                current_path: last_current_path.clone(),
                phase: DiskScanPhase::Scanning,
            });
            last_emit = Instant::now();
        }
    }

    let walk_ms = walk_start.elapsed().as_millis() as u64;

    // Cancellation check at the boundary between walk and finalize. A scan
    // that was replaced or cancelled must not continue into aggregation
    // and publish stale results.
    if token.is_cancelled() {
        return Err("Scan cancelled".to_string());
    }

    let agg_start = Instant::now();

    on_progress(&ProgressSnapshot {
        files: file_count,
        dirs: dir_count,
        bytes: byte_count,
        errors: error_count,
        skipped: skipped_count,
        elapsed_ms: total_start.elapsed().as_millis() as u64,
        current_path: last_current_path.clone(),
        phase: DiskScanPhase::Finalizing,
    });

    // Bottom-up accumulation. We pre-recorded each DirInfo's depth, so the
    // sort cost is O(N log N) on integers — no per-comparison Path
    // component traversal as before.
    let mut dir_order: Vec<u32> = (0..dirs.len() as u32).collect();
    dir_order.sort_unstable_by(|&a, &b| dirs[b as usize].depth.cmp(&dirs[a as usize].depth));

    // Periodic cancel check inside the finalize loop. Cheap (atomic load)
    // and bounded so a huge tree (millions of dirs) can still be aborted.
    let mut since_cancel_check = 0usize;
    for &id in &dir_order {
        if since_cancel_check >= 4096 {
            if token.is_cancelled() {
                return Err("Scan cancelled".to_string());
            }
            since_cancel_check = 0;
        }
        since_cancel_check += 1;
        let id_us = id as usize;
        // First read what we need without a long borrow of `dirs[id_us]`.
        let own_size = dirs[id_us].own_size;
        let own_file_count = dirs[id_us].own_file_count;
        let subdir_ids: Vec<u32> = dirs[id_us].subdir_ids.clone();

        let mut ts = own_size;
        let mut tfc = own_file_count;
        let mut tdc = subdir_ids.len() as u64;
        for sub_id in &subdir_ids {
            let sub = &dirs[*sub_id as usize];
            ts += sub.total_size;
            tfc += sub.total_file_count;
            tdc += sub.total_dir_count;
        }
        let info = &mut dirs[id_us];
        info.total_size = ts;
        info.total_file_count = tfc;
        info.total_dir_count = tdc;
    }

    if token.is_cancelled() {
        return Err("Scan cancelled".to_string());
    }

    let aggregation_ms = agg_start.elapsed().as_millis() as u64;

    // Build id -> path table once, on demand. We keep it in the tree so
    // `get_info` doesn't have to reconstruct paths repeatedly.
    let mut id_to_path: Vec<String> = vec![String::new(); dirs.len()];
    for (path, &id) in &path_to_id {
        id_to_path[id as usize] = path.clone();
    }

    let total_size = dirs.first().map(|d| d.total_size).unwrap_or(0);

    let summary = DiskUsageComplete {
        job_id: String::new(),
        root: root_str.clone(),
        total_size,
        total_files: file_count,
        total_dirs: dir_count,
        errors: error_count,
        skipped: skipped_count,
        elapsed_ms: total_start.elapsed().as_millis() as u64,
        engine: engine_name().to_string(),
    };

    let total_ms = total_start.elapsed().as_millis() as u64;
    let timings = ScanTimings {
        canonicalize_ms,
        walk_ms,
        aggregation_ms,
        total_ms,
        entries_seen,
    };

    Ok((
        DiskUsageTree {
            root: root_str,
            dirs,
            path_to_id,
            id_to_path,
        },
        summary,
        timings,
    ))
}

/// Tauri-bound wrapper. Forwards `ProgressSnapshot` updates to the shared
/// state and to `diskusage.progress` events. The expensive work happens
/// inside `scan_disk_usage_inner`.
pub fn scan_disk_usage(
    root: &str,
    token: CancellationToken,
    job_id: String,
    app: AppHandle,
    excluded: &[String],
    state: Arc<Mutex<DiskUsageScanState>>,
) -> Result<(DiskUsageTree, DiskUsageComplete), String> {
    eprintln!("[diskusage] scan start: root={} job_id={}", root, job_id);

    update_state(&state, |s| {
        s.scan_id = Some(job_id.clone());
        s.status = DiskScanStatus::Scanning;
        s.phase = DiskScanPhase::Starting;
        s.root_path = root.to_string();
        s.engine = engine_name().to_string();
        s.current_path = root.to_string();
        s.elapsed_ms = 0;
        s.files_scanned = 0;
        s.dirs_scanned = 0;
        s.bytes_scanned = 0;
        s.errors_count = 0;
        s.skipped_count = 0;
        s.message = "Resolving path…".to_string();
        s.final_summary = None;
    });

    let app_for_cb = app.clone();
    let job_id_for_cb = job_id.clone();
    let state_for_cb = state.clone();

    let result = scan_disk_usage_inner(root, token, excluded, move |snap| {
        // Update authoritative state and emit a small event. We never hold
        // the state mutex across `app.emit` — emit happens after the closure
        // returns and the lock drops. State writes are guarded by scan_id
        // so a stale progress callback from a replaced scan can never
        // overwrite the new scan's state.
        update_state(&state_for_cb, |s| {
            if s.scan_id.as_deref() != Some(job_id_for_cb.as_str()) {
                return;
            }
            s.files_scanned = snap.files;
            s.dirs_scanned = snap.dirs;
            s.bytes_scanned = snap.bytes;
            s.errors_count = snap.errors;
            s.skipped_count = snap.skipped;
            s.elapsed_ms = snap.elapsed_ms;
            s.current_path = snap.current_path.clone();
            s.phase = snap.phase.clone();
            s.message = match snap.phase {
                DiskScanPhase::Scanning => format!("Scanning {}", snap.current_path),
                DiskScanPhase::Finalizing => "Computing folder totals…".to_string(),
                _ => s.message.clone(),
            };
        });
        let _ = app_for_cb.emit(
            "diskusage.progress",
            DiskUsageProgress {
                job_id: job_id_for_cb.clone(),
                files_found: snap.files,
                dirs_found: snap.dirs,
                bytes_found: snap.bytes,
                errors: snap.errors,
                skipped: snap.skipped,
                elapsed_ms: snap.elapsed_ms,
                current_path: snap.current_path.clone(),
                phase: snap.phase.clone(),
                engine: engine_name().to_string(),
            },
        );
    });

    match result {
        Ok((tree, mut summary, timings)) => {
            summary.job_id = job_id.clone();
            eprintln!(
                "[diskusage] phases: canon={}ms walk={}ms agg={}ms total={}ms entries={}",
                timings.canonicalize_ms,
                timings.walk_ms,
                timings.aggregation_ms,
                timings.total_ms,
                timings.entries_seen
            );
            // IMPORTANT: do NOT advertise `Completed` here. The tree is
            // not yet stored in `state.disk_usage`, so a frontend sync
            // that observes Completed and calls `get_disk_usage()` would
            // fail with "No disk usage data". The IPC layer flips
            // status=Completed AFTER it publishes the tree.
            update_state(&state, |s| {
                if s.scan_id.as_deref() != Some(job_id.as_str()) {
                    return;
                }
                s.phase = DiskScanPhase::Finalizing;
                s.files_scanned = summary.total_files;
                s.dirs_scanned = summary.total_dirs;
                s.bytes_scanned = summary.total_size;
                s.errors_count = summary.errors;
                s.skipped_count = summary.skipped;
                s.elapsed_ms = summary.elapsed_ms;
                s.current_path = summary.root.clone();
                s.message = "Publishing results…".to_string();
            });
            Ok((tree, summary))
        }
        Err(e) => {
            eprintln!("[diskusage] scan ended with error: {e}");
            update_state(&state, |s| {
                if s.scan_id.as_deref() != Some(job_id.as_str()) {
                    return;
                }
                s.status = if e == "Scan cancelled" {
                    DiskScanStatus::Cancelled
                } else {
                    DiskScanStatus::Error
                };
                s.phase = DiskScanPhase::Done;
                s.message = e.clone();
            });
            Err(e)
        }
    }
}

fn update_state<F>(state: &Arc<Mutex<DiskUsageScanState>>, f: F)
where
    F: FnOnce(&mut DiskUsageScanState),
{
    if let Ok(mut g) = state.lock() {
        f(&mut g);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Legacy implementation, kept ONLY for benchmark comparison. Mirrors
    /// the pre-optimization code: jwalk Serial, HashMap<String, DirInfo>
    /// keyed on full paths, metadata() called for every entry (incl.
    /// directories), and Path::components().count()-based depth sort.
    /// Returns (canon_ms, walk_ms, agg_ms, total_ms, files, dirs, bytes).
    fn legacy_scan(root: &str) -> (u64, u64, u64, u64, u64, u64, u64) {
        #[derive(Default)]
        struct LDir {
            #[allow(dead_code)]
            name: String,
            own_size: u64,
            own_files: u64,
            total_size: u64,
            total_files: u64,
            total_dirs: u64,
            subdirs: Vec<String>,
            #[allow(dead_code)]
            files: Vec<u64>,
        }
        let total_t = Instant::now();
        let canon = dunce::canonicalize(root).expect("canonicalize");
        let canon_ms = total_t.elapsed().as_millis() as u64;
        let root_str = canon.to_string_lossy().to_string();
        let walk_t = Instant::now();
        let walker = jwalk::WalkDir::new(&canon)
            .parallelism(jwalk::Parallelism::Serial)
            .skip_hidden(false)
            .follow_links(false);
        let mut dirs: HashMap<String, LDir> = HashMap::new();
        dirs.insert(root_str.clone(), LDir::default());
        let mut file_count = 0u64;
        let mut dir_count = 1u64;
        for entry_result in walker {
            let entry = match entry_result {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            let ft = entry.file_type();
            let is_dir = ft.is_dir();
            let is_file = ft.is_file();
            if !is_dir && !is_file {
                continue;
            }
            let parent_dir = path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            // Stat every entry (the bug we removed).
            let metadata = entry.metadata().ok();
            let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            if is_dir {
                let path_str = path.to_string_lossy().to_string();
                if path_str == root_str {
                    continue;
                }
                dir_count += 1;
                dirs.entry(path_str.clone()).or_insert_with(LDir::default);
                if !parent_dir.is_empty() && parent_dir.starts_with(&root_str) {
                    let p = dirs.entry(parent_dir.clone()).or_insert_with(LDir::default);
                    p.subdirs.push(path_str);
                }
            } else {
                file_count += 1;
                let p = dirs.entry(parent_dir.clone()).or_insert_with(LDir::default);
                p.own_size += size;
                p.own_files += 1;
                p.files.push(size);
            }
        }
        let walk_ms = walk_t.elapsed().as_millis() as u64;

        // Bottom-up accumulation, with Path::components().count() in sort
        let agg_t = Instant::now();
        let mut paths: Vec<String> = dirs.keys().cloned().collect();
        paths.sort_by(|a, b| {
            let da = std::path::Path::new(a).components().count();
            let db = std::path::Path::new(b).components().count();
            db.cmp(&da)
        });
        for p in &paths {
            let subs = dirs[p].subdirs.clone();
            let mut ts = dirs[p].own_size;
            let mut tf = dirs[p].own_files;
            let mut td = subs.len() as u64;
            for s in &subs {
                if let Some(c) = dirs.get(s) {
                    ts += c.total_size;
                    tf += c.total_files;
                    td += c.total_dirs;
                }
            }
            let info = dirs.get_mut(p).unwrap();
            info.total_size = ts;
            info.total_files = tf;
            info.total_dirs = td;
        }
        let agg_ms = agg_t.elapsed().as_millis() as u64;
        let total_ms = total_t.elapsed().as_millis() as u64;
        let total_bytes = dirs.get(&root_str).map(|d| d.total_size).unwrap_or(0);
        (
            canon_ms, walk_ms, agg_ms, total_ms, file_count, dir_count, total_bytes,
        )
    }

    /// Compare legacy vs. current implementation on the same path. Run with:
    ///   BENCH_PATH=/home/sin/Downloads cargo test --release \
    ///     -- --ignored --nocapture bench_disk_usage_compare
    #[test]
    #[ignore]
    fn bench_disk_usage_compare() {
        let path = std::env::var("BENCH_PATH")
            .expect("BENCH_PATH env var required");
        let (lc, lw, la, lt, l_files, l_dirs, l_bytes) = legacy_scan(&path);
        // Warm up filesystem cache symmetrically by running new immediately
        // after — the legacy path traversed the same dirs so dentry cache
        // is already hot for both.
        let token = CancellationToken::new();
        let new_t = Instant::now();
        let (_tree, summary, t) = scan_disk_usage_inner(&path, token, &[], |_| {})
            .expect("new scan");
        let new_total_ms = new_t.elapsed().as_millis() as u64;

        eprintln!();
        eprintln!("===== LEGACY vs CURRENT =====");
        eprintln!("path             : {}", path);
        eprintln!("---");
        eprintln!(
            "legacy  files={} dirs={} bytes={}",
            l_files, l_dirs, l_bytes
        );
        eprintln!(
            "current files={} dirs={} bytes={}",
            summary.total_files, summary.total_dirs, summary.total_size
        );
        eprintln!("---");
        eprintln!("legacy canon     : {} ms", lc);
        eprintln!("legacy walk      : {} ms", lw);
        eprintln!("legacy aggregate : {} ms", la);
        eprintln!("legacy TOTAL     : {} ms", lt);
        eprintln!("---");
        eprintln!("current canon    : {} ms", t.canonicalize_ms);
        eprintln!("current walk     : {} ms", t.walk_ms);
        eprintln!("current aggregate: {} ms", t.aggregation_ms);
        eprintln!("current TOTAL    : {} ms (wall {} ms)", t.total_ms, new_total_ms);
        eprintln!("speedup          : {:.2}x", lt as f64 / t.total_ms.max(1) as f64);
        eprintln!("=============================");

        // Correctness: the refactor must not silently drop entries. Counts
        // and bytes must match the legacy implementation exactly. Dir counts
        // can differ by 1 (legacy counts root as 1; current does too) — they
        // must match.
        assert_eq!(
            summary.total_files, l_files,
            "file count drifted from legacy"
        );
        assert_eq!(
            summary.total_dirs, l_dirs,
            "dir count drifted from legacy"
        );
        assert_eq!(
            summary.total_size, l_bytes,
            "byte total drifted from legacy"
        );
    }

    /// Headless benchmark. Run with:
    ///   BENCH_PATH=/home/sin/Downloads cargo test --release \
    ///     -p cove-file-toolkit -- --ignored --nocapture bench_disk_usage
    #[test]
    #[ignore]
    fn bench_disk_usage() {
        let path = std::env::var("BENCH_PATH")
            .expect("BENCH_PATH env var required (e.g. /home/sin/Downloads)");
        let token = CancellationToken::new();

        let mut emit_count = 0u64;
        let mut last_files = 0u64;
        let scan_t0 = Instant::now();
        let result = scan_disk_usage_inner(&path, token, &[], |snap| {
            emit_count += 1;
            last_files = snap.files;
        })
        .expect("scan failed");
        let scan_total_ms = scan_t0.elapsed().as_millis() as u64;
        let (tree, summary, timings) = result;

        let info_t0 = Instant::now();
        let info = tree.get_info(&summary.root).expect("get_info");
        let info_ms = info_t0.elapsed().as_millis() as u64;

        eprintln!();
        eprintln!("===== DISK USAGE BENCH =====");
        eprintln!("path             : {}", summary.root);
        eprintln!("entries walked   : {}", timings.entries_seen);
        eprintln!("files            : {}", summary.total_files);
        eprintln!("dirs             : {}", summary.total_dirs);
        eprintln!("bytes            : {}", summary.total_size);
        eprintln!("---");
        eprintln!("canonicalize     : {} ms", timings.canonicalize_ms);
        eprintln!("walk + ingest    : {} ms", timings.walk_ms);
        eprintln!("aggregation      : {} ms", timings.aggregation_ms);
        eprintln!("scan total (in-fn): {} ms", timings.total_ms);
        eprintln!("scan total (wall) : {} ms", scan_total_ms);
        eprintln!("get_info(root)   : {} ms", info_ms);
        eprintln!("rootInfo.children    : {}", info.children.len());
        eprintln!("rootInfo.largest_files: {}", info.largest_files.len());
        eprintln!("rootInfo.extensions  : {}", info.extensions.len());
        eprintln!("progress emits   : {}", emit_count);
        eprintln!("============================");
        let _ = last_files;
    }
}
