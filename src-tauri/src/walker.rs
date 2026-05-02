use crate::index::{FileIndex, RootMeta};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;

#[derive(serde::Serialize, Clone)]
pub struct ScanProgress {
    pub job_id: String,
    pub files_found: u64,
    pub dirs_found: u64,
    pub elapsed_ms: u64,
    pub current_path: String,
    pub current_root: String,
    pub current_root_id: String,
    pub roots_done: usize,
    pub roots_total: usize,
}

#[derive(serde::Serialize, Clone)]
pub struct RootResult {
    pub root_id: String,
    pub path: String,
    pub canonical_path: String,
    pub item_count: u64,
    pub files: u64,
    pub dirs: u64,
    pub error: Option<String>,
    /// Set when this root was skipped because an ancestor root in the same
    /// scan already covers it. `error` stays None so the IPC layer treats it
    /// as a successful zero-item scan and prunes any stale entries that this
    /// root_id used to own — the parent now indexes that subtree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covered_by: Option<String>,
}

#[derive(serde::Serialize, Clone)]
pub struct ScanComplete {
    pub job_id: String,
    pub total_files: u64,
    pub total_dirs: u64,
    pub elapsed_ms: u64,
    pub roots: Vec<String>,
    pub per_root: Vec<RootResult>,
}

/// Input shape for the walker — pairs each requested root path with the
/// stable id we want reflected in events and per-root results.
#[derive(Clone, Debug)]
pub struct WalkRoot {
    pub id: String,
    pub path: String,
}

fn system_time_to_epoch(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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

/// True if `child` is the same as `parent` or sits under it. Uses path
/// component comparison, so `/foo/barbaz` is NOT considered under `/foo/bar`.
fn is_under(child: &Path, parent: &Path) -> bool {
    if child == parent {
        return true;
    }
    child.starts_with(parent)
}

pub fn walk_directories(
    roots: &[WalkRoot],
    token: CancellationToken,
    job_id: String,
    app: AppHandle,
    excluded: &[String],
) -> Result<(FileIndex, ScanComplete), String> {
    if roots.is_empty() {
        return Err("No indexed roots configured".to_string());
    }

    let parallelism = num_cpus::get();
    let mut index = FileIndex::default();
    let file_count = Arc::new(AtomicU64::new(0));
    let dir_count = Arc::new(AtomicU64::new(0));
    let mut last_emit = Instant::now();
    let start = Instant::now();
    let roots_total = roots.len();
    let mut per_root: Vec<RootResult> = Vec::with_capacity(roots_total);

    // ----- Phase 1: pre-canonicalize and partition -----
    // Roots that canonicalize successfully are sorted parents-first so the
    // ancestor wins overlap dedup (covers strictly more, never silently
    // losing files outside a child's subtree). Roots that fail to
    // canonicalize are kept aside and reported as errors after the walk so
    // the IPC merge step preserves any prior cached entries they owned
    // (missing removable drives, etc.).
    struct CanonRoot<'a> {
        walk_root: &'a WalkRoot,
        canonical: PathBuf,
        canonical_str: String,
    }
    let mut canon: Vec<CanonRoot> = Vec::with_capacity(roots_total);
    let mut failed: Vec<RootResult> = Vec::new();

    for walk_root in roots {
        match dunce::canonicalize(&walk_root.path) {
            Ok(p) => {
                let s = p.to_string_lossy().to_string();
                canon.push(CanonRoot {
                    walk_root,
                    canonical: p,
                    canonical_str: s,
                });
            }
            Err(e) => {
                failed.push(RootResult {
                    root_id: walk_root.id.clone(),
                    path: walk_root.path.clone(),
                    canonical_path: walk_root.path.clone(),
                    item_count: 0,
                    files: 0,
                    dirs: 0,
                    error: Some(format!("Cannot resolve path: {e}")),
                    covered_by: None,
                });
            }
        }
    }

    // Shallow paths first, then lexicographic for determinism. This makes
    // ancestor roots win over descendants regardless of input order.
    canon.sort_by(|a, b| {
        let da = a.canonical.components().count();
        let db = b.canonical.components().count();
        da.cmp(&db).then_with(|| a.canonical.cmp(&b.canonical))
    });

    let mut walked_canonical: Vec<PathBuf> = Vec::new();
    let mut roots_done: usize = 0;

    for cr in &canon {
        if token.is_cancelled() {
            return Err("Scan cancelled".to_string());
        }

        // Because we processed parents first, any prior walked path that is
        // an ancestor of (or equal to) this one means this root is already
        // covered. Mark it covered (success-with-zero) so the IPC layer
        // treats it as a successful rescan and prunes any stale entries
        // this root_id used to own — the parent now indexes that subtree.
        let mut covered_by: Option<String> = None;
        for prior in &walked_canonical {
            if &cr.canonical == prior || is_under(&cr.canonical, prior) {
                covered_by = Some(prior.to_string_lossy().to_string());
                break;
            }
        }
        if let Some(parent) = covered_by {
            per_root.push(RootResult {
                root_id: cr.walk_root.id.clone(),
                path: cr.walk_root.path.clone(),
                canonical_path: cr.canonical_str.clone(),
                item_count: 0,
                files: 0,
                dirs: 0,
                error: None,
                covered_by: Some(parent),
            });
            continue;
        }

        if !cr.canonical.exists() {
            per_root.push(RootResult {
                root_id: cr.walk_root.id.clone(),
                path: cr.walk_root.path.clone(),
                canonical_path: cr.canonical_str.clone(),
                item_count: 0,
                files: 0,
                dirs: 0,
                error: Some("Path not accessible".to_string()),
                covered_by: None,
            });
            continue;
        }
        if !cr.canonical.is_dir() {
            per_root.push(RootResult {
                root_id: cr.walk_root.id.clone(),
                path: cr.walk_root.path.clone(),
                canonical_path: cr.canonical_str.clone(),
                item_count: 0,
                files: 0,
                dirs: 0,
                error: Some("Not a directory".to_string()),
                covered_by: None,
            });
            continue;
        }

        // Register root metadata so SearchHits / cache writes can resolve
        // the configured root_path from the entry's root_id.
        index.upsert_root(RootMeta {
            id: cr.walk_root.id.clone(),
            path: cr.walk_root.path.clone(),
            canonical_path: cr.canonical_str.clone(),
        });

        let walker = jwalk::WalkDir::new(&cr.canonical)
            .parallelism(jwalk::Parallelism::RayonNewPool(parallelism))
            .skip_hidden(false)
            .follow_links(false);

        let root_files_start = file_count.load(Ordering::Relaxed);
        let root_dirs_start = dir_count.load(Ordering::Relaxed);
        let mut root_cancelled = false;

        for entry_result in walker {
            if token.is_cancelled() {
                root_cancelled = true;
                break;
            }

            let entry = match entry_result {
                Ok(e) => e,
                Err(_) => continue, // skip permission errors etc.
            };

            let path = entry.path();

            if is_excluded(&path, excluded) {
                continue;
            }

            let is_dir = entry.file_type().is_dir();
            let (size, mtime) = entry
                .metadata()
                .map(|m| (m.len(), system_time_to_epoch(m.modified().unwrap_or(UNIX_EPOCH))))
                .unwrap_or((0, 0));

            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let parent = path
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();

            index.push(&parent, &name, size, mtime, is_dir, &cr.walk_root.id);

            if is_dir {
                dir_count.fetch_add(1, Ordering::Relaxed);
            } else {
                file_count.fetch_add(1, Ordering::Relaxed);
            }

            if last_emit.elapsed().as_millis() >= 100 {
                let _ = app.emit(
                    "index.progress",
                    ScanProgress {
                        job_id: job_id.clone(),
                        files_found: file_count.load(Ordering::Relaxed),
                        dirs_found: dir_count.load(Ordering::Relaxed),
                        elapsed_ms: start.elapsed().as_millis() as u64,
                        current_path: parent.clone(),
                        current_root: cr.canonical_str.clone(),
                        current_root_id: cr.walk_root.id.clone(),
                        roots_done,
                        roots_total,
                    },
                );
                last_emit = Instant::now();
            }
        }

        let root_files = file_count.load(Ordering::Relaxed) - root_files_start;
        let root_dirs = dir_count.load(Ordering::Relaxed) - root_dirs_start;

        per_root.push(RootResult {
            root_id: cr.walk_root.id.clone(),
            path: cr.walk_root.path.clone(),
            canonical_path: cr.canonical_str.clone(),
            item_count: root_files + root_dirs,
            files: root_files,
            dirs: root_dirs,
            error: None,
            covered_by: None,
        });

        walked_canonical.push(cr.canonical.clone());
        roots_done += 1;

        if root_cancelled {
            return Err("Scan cancelled".to_string());
        }
    }

    // Surface the canonicalize-failure roots last so they're visible in
    // per_root but don't perturb overlap ordering.
    per_root.extend(failed);

    let total_files = file_count.load(Ordering::Relaxed);
    let total_dirs = dir_count.load(Ordering::Relaxed);

    // Only roots that actually walked successfully (not failed, not covered)
    // contribute to the active root set.
    let canonical_roots: Vec<String> = per_root
        .iter()
        .filter(|r| r.error.is_none() && r.covered_by.is_none())
        .map(|r| r.canonical_path.clone())
        .collect();

    let summary = ScanComplete {
        job_id: job_id.clone(),
        total_files,
        total_dirs,
        elapsed_ms: start.elapsed().as_millis() as u64,
        roots: canonical_roots,
        per_root,
    };

    Ok((index, summary))
}
