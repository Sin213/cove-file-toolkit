use serde::{Deserialize, Deserializer, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IndexRoot {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Hardcoded UUIDv5 namespace used to derive stable per-root IDs from the
/// stored configured path. Must never change — changing it would invalidate
/// every existing cache entry whose owning root_id was derived from this
/// namespace.
const ROOT_NAMESPACE: uuid::Uuid = uuid::Uuid::from_bytes([
    0x7b, 0x6c, 0x2c, 0x4e, 0x5e, 0x3a, 0x4d, 0x4c,
    0x8b, 0x3e, 0x1f, 0x8e, 0x2c, 0x0a, 0x1d, 0x3b,
]);

/// Deterministic root id derived from the stored path string. We deliberately
/// do NOT canonicalize here: canonicalization depends on whether the path is
/// currently mounted/resolvable, so a removable drive or symlinked root would
/// otherwise hash to different ids depending on availability — and that would
/// orphan all of its cached entries on restart while unplugged. Hashing the
/// stored path verbatim keeps the id stable across mount/unmount cycles.
/// Canonical paths are still tracked separately as scan metadata
/// (RootMeta::canonical_path), they just aren't part of the identity.
pub fn stable_root_id(path: &str) -> String {
    uuid::Uuid::new_v5(&ROOT_NAMESPACE, path.as_bytes()).to_string()
}

impl IndexRoot {
    pub fn from_path(path: String) -> Self {
        let display_name = derive_display_name(&path);
        Self {
            id: stable_root_id(&path),
            path,
            display_name,
            enabled: true,
        }
    }
}

pub fn derive_display_name(path: &str) -> String {
    let p = std::path::Path::new(path);
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if path.is_empty() {
                "(unnamed)".to_string()
            } else {
                path.to_string()
            }
        })
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    pub default_root: String,
    pub excluded_patterns: Vec<String>,
    /// Indexed roots. On disk this may be either a list of IndexRoot objects
    /// (current format) or a list of plain strings (legacy v1.0 format).
    /// `deserialize_index_roots` accepts both for backward compat.
    #[serde(default, deserialize_with = "deserialize_index_roots")]
    pub indexed_roots: Vec<IndexRoot>,
    #[serde(default = "default_case_sensitive")]
    pub case_sensitive: bool,
    #[serde(default = "default_match_path")]
    pub match_path: bool,
    #[serde(default = "default_auto_load_cache")]
    pub auto_load_cache: bool,
    #[serde(default = "default_close_to_tray")]
    pub close_to_tray: bool,
}

fn default_case_sensitive() -> bool {
    false
}
fn default_match_path() -> bool {
    true
}
fn default_auto_load_cache() -> bool {
    true
}
fn default_close_to_tray() -> bool {
    true
}

fn deserialize_index_roots<'de, D>(d: D) -> Result<Vec<IndexRoot>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;
    let value = serde_json::Value::deserialize(d)?;
    let arr = match value.as_array() {
        Some(a) => a,
        None => return Err(Error::custom("indexed_roots must be an array")),
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        if let Some(s) = item.as_str() {
            // Legacy: plain string path → wrap as enabled IndexRoot with a fresh id.
            out.push(IndexRoot::from_path(s.to_string()));
        } else {
            let r: IndexRoot =
                serde_json::from_value(item.clone()).map_err(Error::custom)?;
            out.push(r);
        }
    }
    Ok(out)
}

impl Default for Settings {
    fn default() -> Self {
        let home = dirs::home_dir()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());
        Self {
            default_root: home.clone(),
            excluded_patterns: vec![
                "node_modules".to_string(),
                ".git".to_string(),
                "target".to_string(),
                "__pycache__".to_string(),
                ".cache".to_string(),
                ".npm".to_string(),
            ],
            indexed_roots: vec![IndexRoot::from_path(home)],
            case_sensitive: default_case_sensitive(),
            match_path: default_match_path(),
            auto_load_cache: default_auto_load_cache(),
            close_to_tray: default_close_to_tray(),
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("cove-file-toolkit").join("settings.json"))
}

pub fn load_settings() -> Settings {
    let mut loaded: Settings = settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if loaded.indexed_roots.is_empty() && !loaded.default_root.is_empty() {
        loaded.indexed_roots = vec![IndexRoot::from_path(loaded.default_root.clone())];
    }
    loaded
}

pub fn save_settings(settings: &Settings) -> Result<(), String> {
    let path = settings_path().ok_or("Cannot determine config directory")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create config dir: {e}"))?;
    }
    let json =
        serde_json::to_string_pretty(settings).map_err(|e| format!("Serialize error: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("Write error: {e}"))?;
    Ok(())
}
