use crate::settings::{derive_display_name, IndexRoot};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

/// Per-root runtime status. Lives in AppState; not persisted.
#[derive(Clone, Debug)]
pub struct IndexRootRuntime {
    pub state: RootState,
    pub item_count: u64,
    pub last_indexed: i64,
    pub error: Option<String>,
}

impl Default for IndexRootRuntime {
    fn default() -> Self {
        Self {
            state: RootState::Idle,
            item_count: 0,
            last_indexed: 0,
            error: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootState {
    Idle,
    Pending,
    Indexing,
    Ready,
    Error,
    Disabled,
    Missing,
}

impl RootState {
    pub fn as_str(&self) -> &'static str {
        match self {
            RootState::Idle => "idle",
            RootState::Pending => "pending",
            RootState::Indexing => "indexing",
            RootState::Ready => "ready",
            RootState::Error => "error",
            RootState::Disabled => "disabled",
            RootState::Missing => "missing",
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct IndexRootView {
    pub id: String,
    pub path: String,
    pub display_name: String,
    pub enabled: bool,
    pub state: String,
    pub item_count: u64,
    pub last_indexed: i64,
    pub error: Option<String>,
}

pub fn make_view(
    root: &IndexRoot,
    runtime: &HashMap<String, IndexRootRuntime>,
) -> IndexRootView {
    let rt = runtime.get(&root.id).cloned().unwrap_or_default();
    let display_name = if root.display_name.is_empty() {
        derive_display_name(&root.path)
    } else {
        root.display_name.clone()
    };
    let state = if !root.enabled {
        RootState::Disabled
    } else {
        let p = Path::new(&root.path);
        if !p.exists() && rt.state == RootState::Idle {
            RootState::Missing
        } else {
            rt.state
        }
    };
    IndexRootView {
        id: root.id.clone(),
        path: root.path.clone(),
        display_name,
        enabled: root.enabled,
        state: state.as_str().to_string(),
        item_count: rt.item_count,
        last_indexed: rt.last_indexed,
        error: rt.error,
    }
}

/// Validate a candidate root path. Returns the canonical path on success.
pub fn validate_root_path(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Path cannot be empty".to_string());
    }
    let p = Path::new(trimmed);
    if !p.exists() {
        return Err(format!("Path does not exist: {trimmed}"));
    }
    if !p.is_dir() {
        return Err(format!("Not a directory: {trimmed}"));
    }
    let canonical = dunce::canonicalize(p)
        .map(|c| c.to_string_lossy().to_string())
        .unwrap_or_else(|_| trimmed.to_string());
    Ok(canonical)
}

/// Detect mounted drives the user is likely to want to index.
/// Excludes pseudo-filesystems and well-known system locations.
pub fn detect_drives() -> Vec<DetectedRoot> {
    let mut out = Vec::new();
    if let Some(home) = dirs::home_dir() {
        out.push(DetectedRoot {
            path: home.to_string_lossy().to_string(),
            display_name: "Home".to_string(),
            kind: "home".to_string(),
        });
    }
    detect_platform(&mut out);
    // Dedup by canonical path
    let mut seen = std::collections::HashSet::new();
    out.retain(|r| seen.insert(r.path.clone()));
    out
}

#[derive(Serialize, Clone, Debug)]
pub struct DetectedRoot {
    pub path: String,
    pub display_name: String,
    pub kind: String,
}

#[cfg(target_os = "linux")]
fn detect_platform(out: &mut Vec<DetectedRoot>) {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok();
    let mut bases: Vec<String> = vec!["/mnt".to_string(), "/media".to_string()];
    if let Some(u) = &user {
        bases.push(format!("/media/{u}"));
        bases.push(format!("/run/media/{u}"));
    }
    bases.push("/run/media".to_string());
    let mut seen_bases = std::collections::HashSet::new();
    for base in bases {
        if !seen_bases.insert(base.clone()) {
            continue;
        }
        scan_base_dir(&base, out);
    }
}

#[cfg(target_os = "macos")]
fn detect_platform(out: &mut Vec<DetectedRoot>) {
    scan_base_dir("/Volumes", out);
}

#[cfg(target_os = "windows")]
fn detect_platform(out: &mut Vec<DetectedRoot>) {
    // Enumerate drive letters that exist as directories.
    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        if Path::new(&root).is_dir() {
            out.push(DetectedRoot {
                path: root.clone(),
                display_name: format!("{}:", letter as char),
                kind: "drive".to_string(),
            });
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn detect_platform(_out: &mut Vec<DetectedRoot>) {}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn scan_base_dir(base: &str, out: &mut Vec<DetectedRoot>) {
    let entries = match std::fs::read_dir(base) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let path_str = p.to_string_lossy().to_string();
        if is_excluded_mount(&path_str) {
            continue;
        }
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path_str.clone());
        out.push(DetectedRoot {
            path: path_str,
            display_name: name,
            kind: "mount".to_string(),
        });
    }
}

fn is_excluded_mount(path: &str) -> bool {
    // Skip pseudo / system paths even if they show up under detection bases.
    const BAD_PREFIXES: &[&str] = &[
        "/proc", "/sys", "/dev", "/tmp", "/var/run", "/run/user",
        "/run/lock", "/run/systemd", "/run/snapd",
    ];
    for bad in BAD_PREFIXES {
        if path == *bad || path.starts_with(&format!("{bad}/")) {
            return true;
        }
    }
    false
}

/// Detect whether `candidate` is a child of any path in `existing`.
/// Returns the parent path if so. Used to warn about nested roots.
pub fn nested_under<'a>(candidate: &str, existing: &'a [String]) -> Option<&'a String> {
    let cand = Path::new(candidate);
    for ex in existing {
        let ex_path = Path::new(ex);
        if cand == ex_path {
            continue;
        }
        if cand.starts_with(ex_path) {
            return Some(ex);
        }
    }
    None
}

/// Reverse of `nested_under` — detect whether `candidate` is an ancestor of
/// any existing root. Returns the existing child path if so.
/// Adding a parent root after a child has already been configured would
/// otherwise duplicate-index the child's subtree.
pub fn nested_contains<'a>(candidate: &str, existing: &'a [String]) -> Option<&'a String> {
    let cand = Path::new(candidate);
    for ex in existing {
        let ex_path = Path::new(ex);
        if cand == ex_path {
            continue;
        }
        if ex_path.starts_with(cand) {
            return Some(ex);
        }
    }
    None
}
