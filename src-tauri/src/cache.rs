use crate::index::{FileIndex, RootMeta};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Bumped any time the on-disk shape of the cache changes. Loads from a
/// different version are rejected (caller decides whether to rebuild).
///
/// v3: `root_meta` is required for correctness — entries carry root_id and
/// the loader needs per-root metadata to populate FileIndex.roots and emit
/// SearchHits with non-empty `root_path`. Earlier v2 caches that pre-date
/// the multi-root patch may deserialize with an empty `root_meta` and would
/// produce inconsistent state, so we reject them and force a rebuild.
const CACHE_SCHEMA_VERSION: u32 = 3;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CachedRootMeta {
    pub id: String,
    pub path: String,
    pub canonical_path: String,
    #[serde(default)]
    pub item_count: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CacheInfo {
    /// Schema version of this cache file. Missing/older = legacy v1 cache.
    #[serde(default)]
    pub schema_version: u32,
    /// Legacy field — kept for tolerant deserialization. Prefer `roots`.
    #[serde(default)]
    pub root: String,
    pub timestamp: i64,
    pub entry_count: usize,
    /// Canonical paths covered by the cache. Surfaced to the UI.
    #[serde(default)]
    pub roots: Vec<String>,
    /// Stable per-root metadata so cache can be filtered by current
    /// configured/enabled root_ids on autoload.
    #[serde(default)]
    pub root_meta: Vec<CachedRootMeta>,
}

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    p: String,
    n: String,
    s: u64,
    m: i64,
    d: bool,
    /// Owning root id. Empty for legacy v1 entries.
    #[serde(default)]
    r: String,
}

fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("cove-file-toolkit"))
}

fn cache_data_path() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("index_cache.json"))
}

fn cache_info_path() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("index_cache_info.json"))
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Save the full FileIndex (entries + root metadata) to disk.
pub fn save_cache(index: &FileIndex) -> Result<(), String> {
    let data_path = cache_data_path().ok_or("Cannot determine cache directory")?;
    let info_path = cache_info_path().ok_or("Cannot determine cache directory")?;

    if let Some(parent) = data_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create cache dir: {e}"))?;
    }

    let entries: Vec<CacheEntry> = (0..index.len())
        .map(|i| CacheEntry {
            p: index.get_parent(i).unwrap_or("").to_string(),
            n: index.get_name(i).unwrap_or("").to_string(),
            s: index.size[i],
            m: index.mtime[i],
            d: index.is_dir(i),
            r: index.get_root_id(i).unwrap_or("").to_string(),
        })
        .collect();

    let json = serde_json::to_string(&entries).map_err(|e| format!("Serialize error: {e}"))?;
    std::fs::write(&data_path, json).map_err(|e| format!("Write cache: {e}"))?;

    // Per-root tallies for the info file.
    let mut counts: std::collections::HashMap<String, u64> =
        std::collections::HashMap::new();
    for e in &entries {
        *counts.entry(e.r.clone()).or_insert(0) += 1;
    }
    let root_meta: Vec<CachedRootMeta> = index
        .roots
        .values()
        .map(|m| CachedRootMeta {
            id: m.id.clone(),
            path: m.path.clone(),
            canonical_path: m.canonical_path.clone(),
            item_count: counts.get(&m.id).copied().unwrap_or(0),
        })
        .collect();

    let canonical_roots: Vec<String> =
        root_meta.iter().map(|r| r.canonical_path.clone()).collect();
    let primary = canonical_roots.first().cloned().unwrap_or_default();
    let info = CacheInfo {
        schema_version: CACHE_SCHEMA_VERSION,
        root: primary,
        timestamp: now_epoch(),
        entry_count: entries.len(),
        roots: canonical_roots,
        root_meta,
    };
    let info_json =
        serde_json::to_string_pretty(&info).map_err(|e| format!("Serialize info: {e}"))?;
    std::fs::write(&info_path, info_json).map_err(|e| format!("Write info: {e}"))?;

    Ok(())
}

/// Load and validate the cache. `enabled_root_ids`, when present, restricts
/// the loaded index to entries owned by currently-configured + enabled
/// roots — keeping disabled/removed roots out of active search results.
/// `None` loads everything (used by callers that don't have settings).
pub fn load_cache(
    enabled_root_ids: Option<&HashSet<String>>,
) -> Result<(CacheInfo, FileIndex), String> {
    let data_path = cache_data_path().ok_or("No cache directory")?;
    let info_path = cache_info_path().ok_or("No cache directory")?;

    let info_json =
        std::fs::read_to_string(&info_path).map_err(|_| "No cached index".to_string())?;
    let mut info: CacheInfo =
        serde_json::from_str(&info_json).map_err(|e| format!("Parse cache info: {e}"))?;
    // Future-version caches we genuinely cannot read are rejected. Older
    // versions are accepted and migrated below — losing usable cached
    // entries solely because `schema_version` is older than current would
    // strand the user at "No index loaded" after upgrades.
    if info.schema_version > CACHE_SCHEMA_VERSION {
        return Err(format!(
            "Cache schema version {} is newer than supported ({}). Rebuild required.",
            info.schema_version, CACHE_SCHEMA_VERSION
        ));
    }
    if info.roots.is_empty() && !info.root.is_empty() {
        info.roots = vec![info.root.clone()];
    }

    let data_json =
        std::fs::read_to_string(&data_path).map_err(|e| format!("Read cache data: {e}"))?;
    let entries: Vec<CacheEntry> =
        serde_json::from_str(&data_json).map_err(|e| format!("Parse cache data: {e}"))?;

    // Migration: pre-v3 caches (and any current-schema cache where
    // `root_meta` got lost) carry entries without per-root metadata. Rather
    // than hard-rejecting and dumping the user at "No index loaded", we try
    // to synthesize minimal RootMeta from the legacy `roots` / `root` fields
    // and re-attribute entries with empty `r` to the primary root. If we
    // can't synthesize anything (no roots field either) we fall through to
    // the load below — `index.len() == 0` will signal the caller to rebuild.
    if !entries.is_empty() && info.root_meta.is_empty() {
        if !info.roots.is_empty() {
            eprintln!(
                "[cache] migrating legacy cache (schema_version={}, {} entries, no root_meta)",
                info.schema_version,
                entries.len()
            );
            info.root_meta = info
                .roots
                .iter()
                .map(|p| CachedRootMeta {
                    id: crate::settings::stable_root_id(p),
                    path: p.clone(),
                    canonical_path: p.clone(),
                    item_count: 0,
                })
                .collect();
        } else {
            return Err(
                "Cache is missing per-root metadata and has no recoverable roots; rebuild required."
                    .to_string(),
            );
        }
    }

    let mut index = FileIndex::default();
    // Carry over root metadata that survives the filter — anything in
    // `enabled_root_ids` (or all of them when None).
    for m in &info.root_meta {
        if enabled_root_ids
            .map(|s| s.contains(&m.id))
            .unwrap_or(true)
        {
            index.upsert_root(RootMeta {
                id: m.id.clone(),
                path: m.path.clone(),
                canonical_path: m.canonical_path.clone(),
            });
        }
    }
    // Migration helper: legacy v1 entries persist with `r` empty. After
    // synthesizing root_meta from `info.roots`, attribute these entries to
    // the primary (first) root so they survive autoload. Multi-root v1
    // caches are rare in practice; the alternative — discarding them — is
    // worse than the rough attribution.
    let primary_root_id: String = info
        .root_meta
        .first()
        .map(|m| m.id.clone())
        .unwrap_or_default();

    let mut kept = 0usize;
    for e in &entries {
        let effective_r: &str = if e.r.is_empty() {
            primary_root_id.as_str()
        } else {
            e.r.as_str()
        };
        if effective_r.is_empty() {
            continue;
        }
        if let Some(set) = enabled_root_ids {
            if !set.contains(effective_r) {
                continue;
            }
        }
        // Drop any entry whose root_id can't be mapped back to a RootMeta
        // we just loaded — its SearchHit would otherwise carry an empty
        // `root_path`. Realistic only for hand-edited / corrupt caches,
        // since `save_cache` derives root_meta from index.roots; cheap
        // safety net.
        if !index.roots.contains_key(effective_r) {
            continue;
        }
        index.push(&e.p, &e.n, e.s, e.m, e.d, effective_r);
        kept += 1;
    }
    info.entry_count = kept;
    // Surface only the active (enabled) roots in the returned CacheInfo so
    // callers — and anything that displays `info.roots` / `info.root_meta`
    // — don't see disabled or removed roots after autoload.
    if let Some(set) = enabled_root_ids {
        info.root_meta.retain(|m| set.contains(&m.id));
        info.roots = info
            .root_meta
            .iter()
            .map(|m| m.canonical_path.clone())
            .collect();
    }

    Ok((info, index))
}

pub fn get_cache_info() -> Option<CacheInfo> {
    let path = cache_info_path()?;
    let json = std::fs::read_to_string(path).ok()?;
    let mut info: CacheInfo = serde_json::from_str(&json).ok()?;
    if info.roots.is_empty() && !info.root.is_empty() {
        info.roots = vec![info.root.clone()];
    }
    Some(info)
}

pub fn clear_cache() -> Result<(), String> {
    if let Some(p) = cache_data_path() {
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }
    if let Some(p) = cache_info_path() {
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }
    Ok(())
}
