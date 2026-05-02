use crate::index::{join_path, FileIndex, SearchHit};
use globset::{Glob, GlobMatcher};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};

#[derive(Debug, Deserialize, Default)]
pub struct SearchFilters {
    pub extensions: Option<Vec<String>>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub min_mtime: Option<i64>,
    pub max_mtime: Option<i64>,
    pub path_include: Option<String>,
    pub path_exclude: Option<String>,
    pub dirs_only: Option<bool>,
    pub files_only: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct SearchSort {
    pub field: Option<String>,
    pub ascending: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct SearchPage {
    pub items: Vec<SearchHit>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
}

fn is_wildcard(query: &str) -> bool {
    query.contains('*') || query.contains('?')
}

/// Hard upper bound on per-page result count accepted from IPC. Anything
/// larger is clamped to defend against accidental or malicious huge values
/// that would otherwise force the heap to materialize the full match set.
const MAX_PAGE_SIZE: usize = 500;
/// Hard upper bound on the cumulative top-K (page * page_size + page_size).
/// Keeps the bounded heap actually bounded even when `page` is large.
const MAX_TOP_K: usize = 50_000;

/// Build the full-path representation that matches what `FileIndex::get_hit`
/// returns. Used by both path sort and match-path search so sort order and
/// search hits agree on the same string.
fn full_path(parent: &str, name: &str) -> String {
    join_path(parent, name)
}

#[derive(Clone)]
enum SortKey {
    Size(u64),
    Mtime(i64),
    Path(String),
    Ext(String),
    Name(String),
}

impl PartialEq for SortKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for SortKey {}
impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (SortKey::Size(a), SortKey::Size(b)) => a.cmp(b),
            (SortKey::Mtime(a), SortKey::Mtime(b)) => a.cmp(b),
            (SortKey::Path(a), SortKey::Path(b)) => a.cmp(b),
            (SortKey::Ext(a), SortKey::Ext(b)) => a.cmp(b),
            (SortKey::Name(a), SortKey::Name(b)) => a.cmp(b),
            // Mismatched variants shouldn't happen — they'd indicate a
            // single-query mixing sort fields. Fall back to Equal so the
            // heap stays well-behaved.
            _ => Ordering::Equal,
        }
    }
}

fn sort_key(index: &FileIndex, i: usize, field: &str) -> SortKey {
    match field {
        "size" => SortKey::Size(index.size[i]),
        "mtime" => SortKey::Mtime(index.mtime[i]),
        "path" => {
            let parent = index.get_parent(i).unwrap_or("");
            let name = index.get_name(i).unwrap_or("");
            SortKey::Path(full_path(parent, name))
        }
        "ext" => {
            let n = index.get_name(i).unwrap_or("");
            let ext = n.rsplit('.').next().unwrap_or("").to_lowercase();
            SortKey::Ext(ext)
        }
        _ => {
            let n = index.get_name(i).unwrap_or("").to_lowercase();
            SortKey::Name(n)
        }
    }
}

/// Wrapper for the BinaryHeap so we can keep the K *smallest* by popping the
/// largest. Inverts ordering when ascending=false.
///
/// Comparison uses a total order: primary `key`, then `path` (full path,
/// matches the displayed identity), then `idx` (unique per entry). Without
/// the secondary keys, broad searches that produce many equal primary keys
/// (same size, same ext, same mtime, …) would let the heap eviction step
/// keep an arbitrary subset, and successive pages could include duplicates
/// or skip results. The path/idx tiebreakers guarantee that page 1 ∪ page 2
/// is identical to a deterministic full sort sliced into pages.
struct HeapEntry {
    key: SortKey,
    path: String,
    idx: usize,
    ascending: bool,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for HeapEntry {}
impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        let raw = self
            .key
            .cmp(&other.key)
            .then_with(|| self.path.cmp(&other.path))
            .then_with(|| self.idx.cmp(&other.idx));
        if self.ascending {
            raw
        } else {
            raw.reverse()
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn search(
    index: &FileIndex,
    query: &str,
    filters: &SearchFilters,
    sort: &SearchSort,
    page: usize,
    page_size: usize,
    case_sensitive: bool,
    match_path: bool,
    enabled_root_ids: Option<&HashSet<String>>,
) -> SearchPage {
    let prep_query: String = if case_sensitive {
        query.to_string()
    } else {
        query.to_lowercase()
    };
    let has_query = !query.is_empty();

    let glob_pattern = if has_query && is_wildcard(query) {
        prep_query.clone()
    } else {
        String::new()
    };

    let glob_matcher: Option<GlobMatcher> = if !glob_pattern.is_empty() {
        Glob::new(&glob_pattern).ok().map(|g| g.compile_matcher())
    } else {
        None
    };

    let path_include_glob: Option<GlobMatcher> = filters
        .path_include
        .as_ref()
        .and_then(|p| Glob::new(p).ok().map(|g| g.compile_matcher()));

    let path_exclude_glob: Option<GlobMatcher> = filters
        .path_exclude
        .as_ref()
        .and_then(|p| Glob::new(p).ok().map(|g| g.compile_matcher()));

    let ext_lower: Option<Vec<String>> = filters.extensions.as_ref().map(|exts| {
        exts.iter()
            .map(|e| e.to_lowercase().trim_start_matches('.').to_string())
            .collect()
    });

    let sort_field = sort.field.as_deref().unwrap_or("name");
    let ascending = sort.ascending.unwrap_or(true);

    // Clamp paging to defend the heap. page_size is forced into a sensible
    // range; page is capped so the start offset stays strictly below
    // MAX_TOP_K — otherwise the boundary page (start == MAX_TOP_K) is
    // guaranteed empty even when total reports more matches. Total match
    // count is computed over the full iteration so it stays accurate even
    // when paging is clamped.
    let page_size = page_size.clamp(1, MAX_PAGE_SIZE);
    let max_pages = if MAX_TOP_K >= page_size {
        (MAX_TOP_K - 1) / page_size
    } else {
        0
    };
    let page = page.min(max_pages);
    let target_k = page
        .checked_mul(page_size)
        .and_then(|n| n.checked_add(page_size))
        .unwrap_or(MAX_TOP_K)
        .min(MAX_TOP_K);

    // Single pass: count matches, maintain a bounded top-K heap of indices.
    // Heap holds at most `target_k` entries — the worst (largest) is popped
    // when capacity is exceeded so we end up with the K best.
    let mut total: usize = 0;
    let mut heap: BinaryHeap<HeapEntry> = BinaryHeap::with_capacity(target_k.min(1024));

    for i in 0..index.len() {
        // Root-ownership filter (active root scope). Entries owned by a
        // disabled / removed root are excluded from search results.
        if let Some(set) = enabled_root_ids {
            let rid = index.get_root_id(i).unwrap_or("");
            if rid.is_empty() || !set.contains(rid) {
                continue;
            }
        }

        if let Some(true) = filters.dirs_only {
            if !index.is_dir(i) {
                continue;
            }
        }
        if let Some(true) = filters.files_only {
            if index.is_dir(i) {
                continue;
            }
        }

        if let Some(min) = filters.min_size {
            if index.size[i] < min {
                continue;
            }
        }
        if let Some(max) = filters.max_size {
            if index.size[i] > max {
                continue;
            }
        }

        if let Some(min) = filters.min_mtime {
            if index.mtime[i] < min {
                continue;
            }
        }
        if let Some(max) = filters.max_mtime {
            if index.mtime[i] > max {
                continue;
            }
        }

        let name = match index.get_name(i) {
            Some(n) => n,
            None => continue,
        };
        let name_cmp: String = if case_sensitive {
            name.to_string()
        } else {
            name.to_lowercase()
        };

        if let Some(ref exts) = ext_lower {
            let file_ext = name.to_lowercase();
            let file_ext = file_ext.rsplit('.').next().unwrap_or("");
            if !exts.iter().any(|e| e == file_ext) {
                continue;
            }
        }

        if has_query {
            let name_matched = if let Some(ref gm) = glob_matcher {
                gm.is_match(&name_cmp)
            } else {
                name_cmp.contains(&prep_query)
            };

            let mut full_matched = false;
            if !name_matched && match_path {
                let parent = index.get_parent(i).unwrap_or("");
                let full = full_path(parent, name);
                let full_cmp: String = if case_sensitive {
                    full
                } else {
                    full.to_lowercase()
                };
                full_matched = if let Some(ref gm) = glob_matcher {
                    gm.is_match(&full_cmp)
                } else {
                    full_cmp.contains(&prep_query)
                };
            }
            if !name_matched && !full_matched {
                continue;
            }
        }

        if path_include_glob.is_some() || path_exclude_glob.is_some() {
            let parent = index.get_parent(i).unwrap_or("");
            if let Some(ref gm) = path_include_glob {
                if !gm.is_match(parent) {
                    continue;
                }
            }
            if let Some(ref gm) = path_exclude_glob {
                if gm.is_match(parent) {
                    continue;
                }
            }
        }

        total += 1;

        // Top-K maintenance. We don't keep matches we'll never display.
        // `path` is precomputed once for the heap-order tiebreaker so equal
        // primary keys (size/ext/mtime/name collisions) sort deterministically.
        let parent = index.get_parent(i).unwrap_or("");
        let entry = HeapEntry {
            key: sort_key(index, i, sort_field),
            path: full_path(parent, name),
            idx: i,
            ascending,
        };
        if heap.len() < target_k {
            heap.push(entry);
        } else if let Some(worst) = heap.peek() {
            // Heap is a max-heap on our `Ord` direction → peek is the worst
            // currently kept. Replace if this entry is better.
            if entry < *worst {
                heap.pop();
                heap.push(entry);
            }
        }
    }

    // Drain the heap into a sorted Vec.
    let mut top: Vec<HeapEntry> = heap.into_vec();
    top.sort();

    // page * page_size is bounded by MAX_TOP_K thanks to the clamp above,
    // but use checked math to keep the boundary explicit.
    let start = page
        .checked_mul(page_size)
        .unwrap_or(MAX_TOP_K)
        .min(MAX_TOP_K);
    let items: Vec<SearchHit> = top
        .iter()
        .skip(start)
        .take(page_size)
        .map(|e| index.get_hit(e.idx))
        .collect();

    SearchPage {
        items,
        total,
        page,
        page_size,
    }
}
