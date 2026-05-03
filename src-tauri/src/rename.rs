use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum RenameRule {
    #[serde(rename = "prefix")]
    Prefix { text: String },
    #[serde(rename = "suffix")]
    Suffix { text: String },
    #[serde(rename = "remove")]
    Remove {
        text: String,
        case_sensitive: bool,
        #[serde(default)]
        stem_only: bool,
    },
    #[serde(rename = "remove_range")]
    RemoveRange { start: i32, end: i32 },
    #[serde(rename = "replace")]
    Replace {
        from: String,
        to: String,
        case_sensitive: bool,
        #[serde(default)]
        stem_only: bool,
    },
    #[serde(rename = "regex_replace")]
    RegexReplace {
        pattern: String,
        replacement: String,
        #[serde(default)]
        stem_only: bool,
    },
    #[serde(rename = "numbering")]
    Numbering {
        start: u32,
        step: u32,
        padding: u32,
        position: String,
        #[serde(default)]
        separator: Option<String>,
    },
    #[serde(rename = "case_change")]
    CaseChange { mode: String },
    #[serde(rename = "ext_change")]
    ExtChange { new_ext: String },
    #[serde(rename = "ext_case")]
    ExtCase { mode: String },
    #[serde(rename = "remove_ends")]
    RemoveEnds { first: u32, last: u32 },
    #[serde(rename = "insert_at")]
    InsertAt { text: String, position: i32 },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RenamePreviewItem {
    pub original_path: String,
    pub original_name: String,
    pub new_name: String,
    pub new_path: String,
    pub status: String,
    pub message: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RenameRecord {
    pub original: String,
    pub renamed: String,
}

fn split_stem_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(pos) if pos > 0 => (&name[..pos], &name[pos..]),
        _ => (name, ""),
    }
}

fn apply_to_stem<F: Fn(&str) -> String>(name: &str, f: F) -> String {
    let (stem, ext) = split_stem_ext(name);
    format!("{}{}", f(stem), ext)
}

fn apply_rule(name: &str, rule: &RenameRule, index: usize) -> Result<String, String> {
    let (stem, ext) = split_stem_ext(name);

    match rule {
        RenameRule::Prefix { text } => Ok(format!("{}{}{}", text, stem, ext)),
        RenameRule::Suffix { text } => Ok(format!("{}{}{}", stem, text, ext)),
        RenameRule::Remove {
            text,
            case_sensitive,
            stem_only,
        } => {
            let f = |s: &str| -> String {
                if *case_sensitive {
                    s.replace(text.as_str(), "")
                } else {
                    case_insensitive_replace(s, text, "")
                }
            };
            if *stem_only {
                Ok(apply_to_stem(name, f))
            } else {
                Ok(f(name))
            }
        }
        RenameRule::RemoveRange { start, end } => {
            let chars: Vec<char> = stem.chars().collect();
            let len = chars.len() as i32;
            let s = normalize_index(*start, len);
            let e = normalize_index(*end, len);
            if e <= s || s >= len {
                return Ok(name.to_string());
            }
            let new_stem: String = chars
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    let i = *i as i32;
                    i < s || i >= e
                })
                .map(|(_, c)| *c)
                .collect();
            Ok(format!("{}{}", new_stem, ext))
        }
        RenameRule::Replace {
            from,
            to,
            case_sensitive,
            stem_only,
        } => {
            let f = |s: &str| -> String {
                if *case_sensitive {
                    s.replace(from.as_str(), to.as_str())
                } else {
                    case_insensitive_replace(s, from, to)
                }
            };
            if *stem_only {
                Ok(apply_to_stem(name, f))
            } else {
                Ok(f(name))
            }
        }
        RenameRule::RegexReplace {
            pattern,
            replacement,
            stem_only,
        } => {
            let re = Regex::new(pattern).map_err(|e| format!("Invalid regex: {e}"))?;
            if *stem_only {
                Ok(apply_to_stem(name, |s| {
                    re.replace_all(s, replacement.as_str()).to_string()
                }))
            } else {
                Ok(re.replace_all(name, replacement.as_str()).to_string())
            }
        }
        RenameRule::Numbering {
            start,
            step,
            padding,
            position,
            separator,
        } => {
            let number = start + (index as u32 * step);
            let formatted = format!("{:0>width$}", number, width = *padding as usize);
            let sep = separator.clone().unwrap_or_else(|| "_".to_string());
            match position.as_str() {
                "prefix" => Ok(format!("{}{}{}{}", formatted, sep, stem, ext)),
                _ => Ok(format!("{}{}{}{}", stem, sep, formatted, ext)),
            }
        }
        RenameRule::CaseChange { mode } => {
            let new_stem = match mode.as_str() {
                "upper" => stem.to_uppercase(),
                "lower" => stem.to_lowercase(),
                "title" => title_case(stem),
                "sentence" => sentence_case(stem),
                _ => stem.to_string(),
            };
            Ok(format!("{}{}", new_stem, ext))
        }
        RenameRule::ExtCase { mode } => {
            let new_ext = match mode.as_str() {
                "upper" => ext.to_uppercase(),
                "lower" => ext.to_lowercase(),
                _ => ext.to_string(),
            };
            Ok(format!("{}{}", stem, new_ext))
        }
        RenameRule::ExtChange { new_ext } => {
            if new_ext.is_empty() {
                Ok(stem.to_string())
            } else if new_ext.starts_with('.') {
                Ok(format!("{}{}", stem, new_ext))
            } else {
                Ok(format!("{}.{}", stem, new_ext))
            }
        }
        RenameRule::RemoveEnds { first, last } => {
            let chars: Vec<char> = stem.chars().collect();
            let len = chars.len() as i32;
            let start = *first as i32;
            let end = (len - *last as i32).max(start);
            if start >= len || start >= end {
                return Ok(name.to_string());
            }
            let new_stem: String = chars[start as usize..end as usize].iter().collect();
            Ok(format!("{}{}", new_stem, ext))
        }
        RenameRule::InsertAt { text, position } => {
            if text.is_empty() {
                return Ok(name.to_string());
            }
            let chars: Vec<char> = stem.chars().collect();
            let len = chars.len() as i32;
            // Symmetric indexing: 0 = start of stem, -1 = end of stem
            // (i.e. just before extension), -2 = before last stem char, ...
            // Positive overflow clamps to end; negative overflow clamps to 0.
            let mut idx = if *position < 0 {
                len + 1 + *position
            } else {
                *position
            };
            if idx < 0 {
                idx = 0;
            }
            if idx > len {
                idx = len;
            }
            let mut new_stem: String = chars[..idx as usize].iter().collect();
            new_stem.push_str(text);
            new_stem.extend(chars[idx as usize..].iter());
            Ok(format!("{}{}", new_stem, ext))
        }
    }
}

fn normalize_index(idx: i32, len: i32) -> i32 {
    if idx < 0 {
        (len + idx).max(0)
    } else {
        idx.min(len)
    }
}

fn case_insensitive_replace(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let lower_haystack = haystack.to_lowercase();
    let lower_needle = needle.to_lowercase();
    let mut result = String::new();
    let mut last = 0;
    for (idx, _) in lower_haystack.match_indices(&lower_needle) {
        result.push_str(&haystack[last..idx]);
        result.push_str(replacement);
        last = idx + needle.len();
    }
    result.push_str(&haystack[last..]);
    result
}

fn title_case(s: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = true;
    for c in s.chars() {
        if c.is_whitespace() || c == '_' || c == '-' {
            result.push(c);
            capitalize_next = true;
        } else if capitalize_next {
            result.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            result.extend(c.to_lowercase());
            capitalize_next = false;
        }
    }
    result
}

fn sentence_case(s: &str) -> String {
    let mut result = String::new();
    let mut capitalize = true;
    for c in s.chars() {
        if capitalize && c.is_alphabetic() {
            result.extend(c.to_uppercase());
            capitalize = false;
        } else {
            result.extend(c.to_lowercase());
        }
    }
    result
}

pub fn preview_rename(paths: &[String], rules: &[RenameRule]) -> Vec<RenamePreviewItem> {
    let mut previews: Vec<RenamePreviewItem> = Vec::with_capacity(paths.len());

    for (index, path) in paths.iter().enumerate() {
        let p = Path::new(path);
        let original_name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let parent = p
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let mut name = original_name.clone();
        let mut error: Option<String> = None;

        for rule in rules {
            match apply_rule(&name, rule, index) {
                Ok(new_name) => name = new_name,
                Err(e) => {
                    error = Some(e);
                    break;
                }
            }
        }

        if error.is_none() {
            if let Some(msg) = validate_filename(&name) {
                error = Some(msg);
            }
        }

        let new_path = if parent.is_empty() {
            name.clone()
        } else {
            format!("{}{}{}", parent, std::path::MAIN_SEPARATOR, name)
        };

        let status = if error.is_some() {
            "error".to_string()
        } else if name == original_name {
            "unchanged".to_string()
        } else {
            "ok".to_string()
        };

        previews.push(RenamePreviewItem {
            original_path: path.clone(),
            original_name,
            new_name: name,
            new_path,
            status,
            message: error,
        });
    }

    detect_conflicts(&mut previews, paths);
    previews
}

/// Approximation of the FS's case-folding rule for collision detection.
/// Windows NTFS and macOS HFS+/APFS default volumes fold ASCII case; Linux
/// ext4/btrfs/etc. do not. We use this only for in-memory comparison —
/// `Path::exists()` already handles filesystem-side folding correctly.
fn normalize_path_for_collision(p: &str) -> String {
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        p.to_lowercase()
    } else {
        p.to_string()
    }
}

/// Cross-platform filename validation. Rejects names that would be
/// invalid on any major OS so that renames produced on Linux/macOS don't
/// silently break when files later land on a Windows share, OneDrive
/// sync, or Windows guest. Returns the user-facing error message when
/// the name is invalid, `None` when it's fine.
fn validate_filename(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("Name would be empty".to_string());
    }
    if name.contains('/') || name.contains('\\') {
        return Some("Name contains path separators".to_string());
    }
    if name.contains('\0') {
        return Some("Name contains null bytes".to_string());
    }
    for c in name.chars() {
        let cp = c as u32;
        if cp < 0x20 || cp == 0x7f {
            return Some("Name contains control characters".to_string());
        }
    }
    const WIN_INVALID: &[char] = &['<', '>', ':', '"', '|', '?', '*'];
    for c in name.chars() {
        if WIN_INVALID.contains(&c) {
            return Some(format!("Name contains invalid character: {c}"));
        }
    }
    if name.ends_with(' ') || name.ends_with('.') {
        return Some("Name ends with a space or dot".to_string());
    }
    let stem = match name.rfind('.') {
        Some(pos) if pos > 0 => &name[..pos],
        _ => name,
    };
    let upper = stem.to_ascii_uppercase();
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&upper.as_str()) {
        return Some(format!("Name is a reserved device name: {stem}"));
    }
    None
}

fn detect_conflicts(previews: &mut [RenamePreviewItem], original_paths: &[String]) {
    // Duplicate-target detection uses the platform-aware normalized form so
    // foo.txt and FOO.txt are caught as conflicts on case-insensitive
    // filesystems even though their raw strings differ.
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut conflict_indices: Vec<(usize, usize)> = Vec::new();

    for (i, preview) in previews.iter().enumerate() {
        if preview.status != "ok" {
            continue;
        }
        let key = normalize_path_for_collision(&preview.new_path);
        if let Some(&first) = seen.get(&key) {
            conflict_indices.push((first, i));
        } else {
            seen.insert(key, i);
        }
    }

    for (a, b) in conflict_indices {
        previews[a].status = "conflict".to_string();
        previews[a].message = Some("Duplicate target name".to_string());
        previews[b].status = "conflict".to_string();
        previews[b].message = Some("Duplicate target name".to_string());
    }

    // External collision: target normalizes to an existing filesystem path
    // that is NOT another original being renamed in this same batch.
    // Within-batch collisions (swap A↔B, cycle A→B→C→A, case-only flip
    // on a case-insensitive FS) are resolved by the staging in
    // `apply_rename` and must not be flagged here.
    let original_set: HashSet<String> = original_paths
        .iter()
        .map(|p| normalize_path_for_collision(p))
        .collect();
    for preview in previews.iter_mut() {
        if preview.status != "ok" {
            continue;
        }
        let new_norm = normalize_path_for_collision(&preview.new_path);
        let orig_norm = normalize_path_for_collision(&preview.original_path);
        if new_norm == orig_norm {
            // No-op or pure case-only rename — staging handles it.
            continue;
        }
        if original_set.contains(&new_norm) {
            // In-batch swap/cycle — staging handles it.
            continue;
        }
        if Path::new(&preview.new_path).exists() {
            preview.status = "conflict".to_string();
            preview.message = Some("Would overwrite existing file".to_string());
        }
    }
}

struct StagedRename {
    original: String,
    temp: String,
    final_path: String,
    original_name: String,
}

/// A directory reserved exclusively by this rename operation. Created with
/// `fs::create_dir`, which fails atomically when the path already exists.
/// Once the reservation succeeds, the OS guarantees no other process can
/// have allocated the same path, so any temp filenames placed inside are
/// protected from sibling collisions.
struct ReservedTempDir {
    path: PathBuf,
}

impl ReservedTempDir {
    /// Reserve a hidden, unique-named temp directory beside `parent`. Same
    /// filesystem as `parent`, so subsequent `std::fs::rename` moves into
    /// the directory remain atomic. Retries on `AlreadyExists`; bubbles
    /// other errors to the caller.
    fn create_in(parent: &Path) -> Result<Self, String> {
        Self::create_in_with(parent, || uuid::Uuid::new_v4().to_string())
    }

    fn create_in_with<F: FnMut() -> String>(
        parent: &Path,
        mut gen_token: F,
    ) -> Result<Self, String> {
        for _ in 0..32 {
            let token = gen_token();
            let name = format!(".__cove_rename_{}", token);
            let path = parent.join(name);
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(format!(
                        "Cannot reserve temp directory in '{}': {}",
                        parent.display(),
                        e
                    ));
                }
            }
        }
        Err(format!(
            "Could not reserve a unique temp directory in '{}'",
            parent.display()
        ))
    }

    fn child(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Remove the reserved directory if (and only if) it is empty. Returns
    /// `true` when removed (or already gone) and `false` when files remain
    /// inside — those must be preserved for manual recovery.
    fn try_cleanup(&self) -> bool {
        match std::fs::remove_dir(&self.path) {
            Ok(()) => true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => true,
            Err(_) => false,
        }
    }
}

/// One reserved temp directory per source parent. Renames in this app stay
/// within their parent, but a batch can span multiple parents, so we keep a
/// dir per parent and reuse it for both forward staging temps and rollback
/// temps. The map is also the cleanup roster.
struct TempRegistry {
    dirs: HashMap<PathBuf, ReservedTempDir>,
}

impl TempRegistry {
    fn new() -> Self {
        Self {
            dirs: HashMap::new(),
        }
    }

    fn reserve_for(&mut self, parent: &Path) -> Result<&ReservedTempDir, String> {
        if !self.dirs.contains_key(parent) {
            let reserved = ReservedTempDir::create_in(parent)?;
            self.dirs.insert(parent.to_path_buf(), reserved);
        }
        Ok(self.dirs.get(parent).expect("just inserted"))
    }

    /// Reserve (or reuse) the temp dir for `host_path`'s parent and return
    /// `<reserved_dir>/<child_name>`. Within a parent, callers pass distinct
    /// `child_name`s (e.g. `s_<orig>` for staging vs `r_<final>` for
    /// rollback) to avoid intra-batch collisions.
    fn child_path(&mut self, host_path: &str, child_name: &str) -> Result<PathBuf, String> {
        let p = Path::new(host_path);
        let parent = p
            .parent()
            .ok_or_else(|| format!("'{}' has no parent directory", host_path))?;
        let dir = self.reserve_for(parent)?;
        Ok(dir.child(child_name))
    }

    /// Remove every reserved dir that is now empty. Non-empty dirs are kept
    /// (they hold files for manual recovery) and their paths are returned.
    fn cleanup_empty(&self) -> Vec<PathBuf> {
        let mut leftovers: Vec<PathBuf> = Vec::new();
        for d in self.dirs.values() {
            if !d.try_cleanup() {
                leftovers.push(d.path.clone());
            }
        }
        leftovers
    }
}

/// Why an atomic no-overwrite rename refused or failed.
#[derive(Debug)]
enum AtomicRenameError {
    /// `dst` already exists. The destination existence check and the move
    /// were performed atomically by the kernel; `dst` was NOT touched and
    /// `src` is untouched.
    DestinationExists,
    /// The host platform does not provide a directory-capable atomic
    /// no-replace move primitive (or the kernel/filesystem rejected it).
    /// `src` and `dst` were not modified.
    Unsupported,
    /// Filesystem error other than destination-exists or unsupported
    /// (permissions, I/O, cross-device, etc.). `src` and `dst` were not
    /// modified.
    Io(io::Error),
}

impl std::fmt::Display for AtomicRenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AtomicRenameError::DestinationExists => {
                write!(f, "destination already exists")
            }
            AtomicRenameError::Unsupported => write!(
                f,
                "platform does not support an atomic directory-capable no-replace rename"
            ),
            AtomicRenameError::Io(e) => write!(f, "{}", e),
        }
    }
}

/// Atomically move `src` to `dst` WITHOUT ever overwriting `dst`. Supports
/// both regular files and directories on platforms that expose a true
/// no-replace move primitive.
///
/// `src` and `dst` must live on the same filesystem. In this module that
/// invariant always holds because rollback temps live in a reserved
/// subdirectory of the original's parent, created on the same volume.
///
/// Platform implementations:
/// - Linux: `renameat2(AT_FDCWD, src, AT_FDCWD, dst, RENAME_NOREPLACE)`.
///   Kernel-atomic and supported for both files and directories. If the
///   syscall is unavailable (`ENOSYS`) or rejected (`EINVAL` from older
///   kernels / filesystems that do not support the flag), we surface
///   `Unsupported` instead of falling back to an overwriting rename.
/// - Windows: `MoveFileW` (no flags). Without `MOVEFILE_REPLACE_EXISTING`
///   it refuses to clobber an existing destination and natively supports
///   directories on the same volume.
/// - macOS: `renamex_np` with `RENAME_EXCL`. Same kernel-atomic guarantees
///   as `renameat2(RENAME_NOREPLACE)` and supports directories.
/// - Other platforms (BSDs, etc.): `Unsupported`. The caller leaves the
///   temp/current path in place and reports the recovery details rather
///   than risk an overwriting rename.
#[cfg(target_os = "linux")]
fn atomic_rename_no_replace(src: &Path, dst: &Path) -> Result<(), AtomicRenameError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let src_c = CString::new(src.as_os_str().as_bytes())
        .map_err(|e| AtomicRenameError::Io(io::Error::new(io::ErrorKind::InvalidInput, e)))?;
    let dst_c = CString::new(dst.as_os_str().as_bytes())
        .map_err(|e| AtomicRenameError::Io(io::Error::new(io::ErrorKind::InvalidInput, e)))?;

    // RENAME_NOREPLACE is defined in <linux/fs.h>; libc may not expose it
    // on older targets, so define it locally.
    const RENAME_NOREPLACE: libc::c_uint = 1;

    let ret = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            src_c.as_ptr(),
            libc::AT_FDCWD,
            dst_c.as_ptr(),
            RENAME_NOREPLACE,
        )
    };

    if ret == 0 {
        return Ok(());
    }

    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::EEXIST) | Some(libc::ENOTEMPTY) => Err(AtomicRenameError::DestinationExists),
        // ENOSYS: kernel too old. EINVAL: filesystem rejects the flag
        // (e.g. some older overlay/network filesystems). EOPNOTSUPP /
        // ENOTSUP: same intent. Treat all as "no safe primitive here" so
        // the caller falls back to preserving the temp path.
        Some(libc::ENOSYS) | Some(libc::EINVAL) | Some(libc::EOPNOTSUPP) => {
            Err(AtomicRenameError::Unsupported)
        }
        _ => Err(AtomicRenameError::Io(err)),
    }
}

#[cfg(target_os = "macos")]
fn atomic_rename_no_replace(src: &Path, dst: &Path) -> Result<(), AtomicRenameError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let src_c = CString::new(src.as_os_str().as_bytes())
        .map_err(|e| AtomicRenameError::Io(io::Error::new(io::ErrorKind::InvalidInput, e)))?;
    let dst_c = CString::new(dst.as_os_str().as_bytes())
        .map_err(|e| AtomicRenameError::Io(io::Error::new(io::ErrorKind::InvalidInput, e)))?;

    // RENAME_EXCL = 0x4 — fail if dst exists. Atomic, file + directory
    // capable on APFS/HFS+.
    const RENAME_EXCL: libc::c_uint = 0x0000_0004;

    extern "C" {
        fn renamex_np(
            from: *const libc::c_char,
            to: *const libc::c_char,
            flags: libc::c_uint,
        ) -> libc::c_int;
    }

    let ret = unsafe { renamex_np(src_c.as_ptr(), dst_c.as_ptr(), RENAME_EXCL) };
    if ret == 0 {
        return Ok(());
    }

    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::EEXIST) | Some(libc::ENOTEMPTY) => Err(AtomicRenameError::DestinationExists),
        Some(libc::ENOSYS) | Some(libc::ENOTSUP) | Some(libc::EOPNOTSUPP)
        | Some(libc::EINVAL) => Err(AtomicRenameError::Unsupported),
        _ => Err(AtomicRenameError::Io(err)),
    }
}

#[cfg(target_os = "windows")]
fn atomic_rename_no_replace(src: &Path, dst: &Path) -> Result<(), AtomicRenameError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, GetLastError,
    };
    use windows_sys::Win32::Storage::FileSystem::MoveFileW;

    fn to_wide(p: &Path) -> Vec<u16> {
        let mut v: Vec<u16> = p.as_os_str().encode_wide().collect();
        v.push(0);
        v
    }

    let src_w = to_wide(src);
    let dst_w = to_wide(dst);

    let ok = unsafe { MoveFileW(src_w.as_ptr(), dst_w.as_ptr()) };
    if ok != 0 {
        return Ok(());
    }

    let code = unsafe { GetLastError() };
    match code {
        ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS => Err(AtomicRenameError::DestinationExists),
        _ => Err(AtomicRenameError::Io(io::Error::from_raw_os_error(code as i32))),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn atomic_rename_no_replace(_src: &Path, _dst: &Path) -> Result<(), AtomicRenameError> {
    Err(AtomicRenameError::Unsupported)
}

/// Move `temp` back to `original` using an atomic no-overwrite primitive.
/// Returns `Err(reason)` when the original is occupied, the platform lacks
/// a safe directory-capable no-replace primitive, or the move fails for
/// any other reason. In every error case the temp path is left in place
/// for manual recovery and the reserved temp directory is preserved by
/// the registry's `cleanup_empty`. The destination existence check and
/// the move are performed atomically by the kernel.
fn restore_no_overwrite(temp: &Path, original: &Path) -> Result<(), String> {
    match atomic_rename_no_replace(temp, original) {
        Ok(()) => Ok(()),
        Err(AtomicRenameError::DestinationExists) => {
            Err("original path is occupied by another file".to_string())
        }
        Err(AtomicRenameError::Unsupported) => Err(
            "atomic directory-capable no-replace rename is not supported on this platform"
                .to_string(),
        ),
        Err(AtomicRenameError::Io(e)) => Err(format!(
            "atomic no-overwrite restore '{}' -> '{}' failed: {}",
            temp.display(),
            original.display(),
            e
        )),
    }
}

/// Roll back a partially-finalized batch without ever overwriting a path
/// that might be occupied. Phase 1 parks finalized files into the reserved
/// temp dir; phase 2 restores those temps to originals only if the original
/// is empty; phase 3 does the same for staged-but-not-finalized items.
/// Occupied originals are left alone and reported with full recovery
/// information so the user can finish the restore by hand.
fn rollback_full(
    registry: &mut TempRegistry,
    records: &[RenameRecord],
    staged: &[StagedRename],
    failed_idx: usize,
) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();

    // Phase 1: park each finalized file at a unique rollback temp inside
    // its parent's reserved directory. The reserved dir is exclusive to
    // this operation, so no external process can hijack the temp path.
    let mut rollback_temps: Vec<(PathBuf, String, String)> = Vec::with_capacity(records.len());
    for r in records {
        let renamed_path = Path::new(&r.renamed);
        let renamed_name = match renamed_path.file_name() {
            Some(n) => n.to_string_lossy().to_string(),
            None => {
                errors.push(format!(
                    "Rollback could not derive basename for FINALIZED file '{}' (original '{}'). \
                     Manual restore required.",
                    r.renamed, r.original
                ));
                continue;
            }
        };
        let temp_path = match registry.child_path(&r.renamed, &format!("r_{}", renamed_name)) {
            Ok(p) => p,
            Err(e) => {
                errors.push(format!(
                    "Rollback could not reserve temp for FINALIZED file '{}' (original '{}'): {}. \
                     Manual restore required.",
                    r.renamed, r.original, e
                ));
                continue;
            }
        };
        if let Err(e) = std::fs::rename(&r.renamed, &temp_path) {
            errors.push(format!(
                "Rollback could not park FINALIZED file '{}' at temp '{}' (original '{}'): {}. \
                 Manual restore required.",
                r.renamed,
                temp_path.display(),
                r.original,
                e
            ));
            continue;
        }
        rollback_temps.push((temp_path, r.original.clone(), r.renamed.clone()));
    }

    // Phase 2: move each rollback temp back to its original path. If the
    // original was recreated by another process, leave the temp in place
    // and report rather than overwrite.
    for (temp_path, original, renamed) in &rollback_temps {
        let orig_path = Path::new(original);
        if let Err(reason) = restore_no_overwrite(temp_path, orig_path) {
            errors.push(format!(
                "Rollback skipped restore of FINALIZED file: original '{}' (was renamed to '{}'); \
                 file preserved at '{}'; reason: {}. Move it back manually if appropriate.",
                original,
                renamed,
                temp_path.display(),
                reason
            ));
        }
    }

    // Phase 3: restore staged-but-not-finalized files. Their originals were
    // vacated in stage 1 of the forward pass, but a third party could have
    // re-created them in the meantime, so the same no-overwrite check
    // applies.
    for s in staged[failed_idx..].iter().rev() {
        let temp_path = Path::new(&s.temp);
        let orig_path = Path::new(&s.original);
        if let Err(reason) = restore_no_overwrite(temp_path, orig_path) {
            errors.push(format!(
                "Rollback skipped restore of STAGED file '{}': original '{}'; \
                 file preserved at '{}'; reason: {}. Move it back manually if appropriate.",
                s.original_name, s.original, s.temp, reason
            ));
        }
    }

    errors
}

fn format_rollback_error(
    primary: String,
    rollback_errors: &[String],
    leftover_dirs: &[PathBuf],
) -> String {
    let mut out = primary;
    if !rollback_errors.is_empty() {
        out.push_str(&format!(
            "\nRollback reported {} issue(s): {}",
            rollback_errors.len(),
            rollback_errors.join(" | ")
        ));
    }
    if !leftover_dirs.is_empty() {
        let paths: Vec<String> = leftover_dirs
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        out.push_str(&format!(
            "\nFiles preserved for manual recovery in: {}",
            paths.join(", ")
        ));
    }
    out
}

pub fn apply_rename(previews: &[RenamePreviewItem]) -> Result<Vec<RenameRecord>, String> {
    for p in previews {
        if p.status == "error" || p.status == "conflict" {
            return Err(format!(
                "Cannot apply: '{}' has status '{}'",
                p.original_name, p.status
            ));
        }
    }

    // Only operations that actually change the path need to move; "unchanged"
    // rows are skipped, and rows where the raw string differs but the
    // normalized form matches the original (some pathological no-ops) also
    // skip — staging would just shuffle them through a temp for nothing.
    let ops: Vec<&RenamePreviewItem> = previews
        .iter()
        .filter(|p| p.status == "ok" && p.original_path != p.new_path)
        .collect();

    if ops.is_empty() {
        return Ok(Vec::new());
    }

    let mut registry = TempRegistry::new();

    // Stage 1: rename every source into its parent's reserved temp dir.
    // The reserved dir is created with `fs::create_dir`, which is atomic
    // wrt other processes, so paths inside it are immune to the
    // check-then-rename race the previous helper had. Staging breaks
    // in-batch cycles/swaps and moves case-only renames out of the way
    // on case-insensitive filesystems before the final name is reused.
    let mut staged: Vec<StagedRename> = Vec::with_capacity(ops.len());
    for p in &ops {
        let temp_path =
            match registry.child_path(&p.original_path, &format!("s_{}", p.original_name)) {
                Ok(t) => t,
                Err(e) => {
                    let rb = rollback_full(&mut registry, &[], &staged, 0);
                    let leftovers = registry.cleanup_empty();
                    return Err(format_rollback_error(
                        format!("Cannot reserve temp for '{}': {}", p.original_name, e),
                        &rb,
                        &leftovers,
                    ));
                }
            };
        if let Err(e) = std::fs::rename(&p.original_path, &temp_path) {
            let rb = rollback_full(&mut registry, &[], &staged, 0);
            let leftovers = registry.cleanup_empty();
            return Err(format_rollback_error(
                format!("Failed to stage '{}': {}", p.original_name, e),
                &rb,
                &leftovers,
            ));
        }
        staged.push(StagedRename {
            original: p.original_path.clone(),
            temp: temp_path.to_string_lossy().to_string(),
            final_path: p.new_path.clone(),
            original_name: p.original_name.clone(),
        });
    }

    // Stage 2: temp -> final. Defensive existence check guards against any
    // collision the preview missed (e.g. an unrelated file appeared on disk
    // between preview and apply).
    let mut records: Vec<RenameRecord> = Vec::with_capacity(staged.len());
    for (i, s) in staged.iter().enumerate() {
        if Path::new(&s.final_path).exists() {
            let rb = rollback_full(&mut registry, &records, &staged, i);
            let leftovers = registry.cleanup_empty();
            return Err(format_rollback_error(
                format!("Refusing to overwrite existing file: {}", s.final_path),
                &rb,
                &leftovers,
            ));
        }
        if let Err(e) = std::fs::rename(&s.temp, &s.final_path) {
            let rb = rollback_full(&mut registry, &records, &staged, i);
            let leftovers = registry.cleanup_empty();
            return Err(format_rollback_error(
                format!("Failed to rename '{}': {}", s.original_name, e),
                &rb,
                &leftovers,
            ));
        }
        records.push(RenameRecord {
            original: s.original.clone(),
            renamed: s.final_path.clone(),
        });
    }

    // Success: every reserved temp dir is now empty. Remove them. If any
    // are non-empty (defensive — should not happen on success), preserve
    // them so the user can investigate manually.
    let _ = registry.cleanup_empty();

    Ok(records)
}

pub fn undo_rename(records: &[RenameRecord]) -> Result<(), String> {
    for record in records.iter().rev() {
        std::fs::rename(&record.renamed, &record.original)
            .map_err(|e| format!("Failed to undo '{}': {}", record.renamed, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp_dir(label: &str) -> PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!(
            "cove_rename_test_{}_{}",
            label,
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(p: &PathBuf, contents: &str) {
        fs::write(p, contents).unwrap();
    }

    #[test]
    fn validate_filename_rejects_windows_invalid() {
        assert!(validate_filename("").is_some());
        assert!(validate_filename("foo/bar").is_some());
        assert!(validate_filename("foo\\bar").is_some());
        assert!(validate_filename("foo\0bar").is_some());
        assert!(validate_filename("foo\x07bar").is_some()); // bell
        assert!(validate_filename("foo\x7fbar").is_some()); // DEL
        for c in ['<', '>', ':', '"', '|', '?', '*'] {
            assert!(
                validate_filename(&format!("name{c}txt")).is_some(),
                "expected reject for {c}"
            );
        }
        assert!(validate_filename("trailing ").is_some());
        assert!(validate_filename("trailing.").is_some());
        for r in ["CON", "con", "PRN", "Aux", "NUL", "COM1", "lpt9"] {
            assert!(
                validate_filename(r).is_some(),
                "expected reject for reserved {r}"
            );
        }
        assert!(validate_filename("CON.txt").is_some());
        // Sane names accepted.
        assert!(validate_filename("ok.txt").is_none());
        assert!(validate_filename("space inside.txt").is_none());
        assert!(validate_filename("dot.in.middle.txt").is_none());
    }

    #[test]
    fn preview_marks_invalid_windows_names() {
        let dir = tmp_dir("invalid_names");
        let src = dir.join("src.txt");
        touch(&src, "x");
        let previews = preview_rename(
            &[src.to_string_lossy().to_string()],
            &[RenameRule::Replace {
                from: "src".to_string(),
                to: "CON".to_string(),
                case_sensitive: true,
                stem_only: true,
            }],
        );
        assert_eq!(previews[0].status, "error");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_flags_duplicate_targets() {
        let dir = tmp_dir("dup_targets");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        touch(&a, "1");
        touch(&b, "2");
        let paths = vec![
            a.to_string_lossy().to_string(),
            b.to_string_lossy().to_string(),
        ];
        // Replace stem with constant — both end up the same.
        let previews = preview_rename(
            &paths,
            &[RenameRule::RegexReplace {
                pattern: "^.*$".to_string(),
                replacement: "same".to_string(),
                stem_only: true,
            }],
        );
        assert!(previews.iter().all(|p| p.status == "conflict"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_rename_handles_swap_safely() {
        let dir = tmp_dir("swap");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        touch(&a, "A");
        touch(&b, "B");
        let previews = vec![
            RenamePreviewItem {
                original_path: a.to_string_lossy().to_string(),
                original_name: "a.txt".to_string(),
                new_name: "b.txt".to_string(),
                new_path: b.to_string_lossy().to_string(),
                status: "ok".to_string(),
                message: None,
            },
            RenamePreviewItem {
                original_path: b.to_string_lossy().to_string(),
                original_name: "b.txt".to_string(),
                new_name: "a.txt".to_string(),
                new_path: a.to_string_lossy().to_string(),
                status: "ok".to_string(),
                message: None,
            },
        ];
        let records = apply_rename(&previews).expect("swap should succeed");
        assert_eq!(records.len(), 2);
        // Contents should be swapped: a.txt now holds B, b.txt now holds A.
        assert_eq!(fs::read_to_string(&a).unwrap(), "B");
        assert_eq!(fs::read_to_string(&b).unwrap(), "A");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_rename_handles_three_cycle() {
        let dir = tmp_dir("cycle");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        let c = dir.join("c.txt");
        touch(&a, "A");
        touch(&b, "B");
        touch(&c, "C");
        // A -> B, B -> C, C -> A
        let previews = vec![
            RenamePreviewItem {
                original_path: a.to_string_lossy().to_string(),
                original_name: "a.txt".to_string(),
                new_name: "b.txt".to_string(),
                new_path: b.to_string_lossy().to_string(),
                status: "ok".to_string(),
                message: None,
            },
            RenamePreviewItem {
                original_path: b.to_string_lossy().to_string(),
                original_name: "b.txt".to_string(),
                new_name: "c.txt".to_string(),
                new_path: c.to_string_lossy().to_string(),
                status: "ok".to_string(),
                message: None,
            },
            RenamePreviewItem {
                original_path: c.to_string_lossy().to_string(),
                original_name: "c.txt".to_string(),
                new_name: "a.txt".to_string(),
                new_path: a.to_string_lossy().to_string(),
                status: "ok".to_string(),
                message: None,
            },
        ];
        let records = apply_rename(&previews).expect("cycle should succeed");
        assert_eq!(records.len(), 3);
        assert_eq!(fs::read_to_string(&a).unwrap(), "C");
        assert_eq!(fs::read_to_string(&b).unwrap(), "A");
        assert_eq!(fs::read_to_string(&c).unwrap(), "B");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_rename_refuses_external_overwrite() {
        let dir = tmp_dir("ext_collision");
        let src = dir.join("src.txt");
        let dst = dir.join("dst.txt");
        touch(&src, "S");
        touch(&dst, "D");
        // The preview should flag this as a conflict.
        let previews = preview_rename(
            &[src.to_string_lossy().to_string()],
            &[RenameRule::Replace {
                from: "src".to_string(),
                to: "dst".to_string(),
                case_sensitive: true,
                stem_only: true,
            }],
        );
        assert_eq!(previews[0].status, "conflict");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cycle_rollback_preserves_all_contents() {
        // A → B, B → C, C → D where D pre-exists. Stage 1 parks all three at
        // unique temps; stage 2 finalizes A→B and B→C, then trips the
        // existence check on D and triggers rollback. The pre-fix code
        // unwound finalized renames in reverse with direct std::fs::rename,
        // overwriting B's content during the C→B step. Cycle-safe rollback
        // must restore A=A, B=B, C=C with no data loss.
        let dir = tmp_dir("cycle_rollback");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        let c = dir.join("c.txt");
        let d = dir.join("d.txt");
        touch(&a, "A");
        touch(&b, "B");
        touch(&c, "C");
        touch(&d, "D"); // blocker — final step will refuse to overwrite

        let previews = vec![
            RenamePreviewItem {
                original_path: a.to_string_lossy().to_string(),
                original_name: "a.txt".to_string(),
                new_name: "b.txt".to_string(),
                new_path: b.to_string_lossy().to_string(),
                status: "ok".to_string(),
                message: None,
            },
            RenamePreviewItem {
                original_path: b.to_string_lossy().to_string(),
                original_name: "b.txt".to_string(),
                new_name: "c.txt".to_string(),
                new_path: c.to_string_lossy().to_string(),
                status: "ok".to_string(),
                message: None,
            },
            RenamePreviewItem {
                original_path: c.to_string_lossy().to_string(),
                original_name: "c.txt".to_string(),
                new_name: "d.txt".to_string(),
                new_path: d.to_string_lossy().to_string(),
                status: "ok".to_string(),
                message: None,
            },
        ];

        let result = apply_rename(&previews);
        assert!(result.is_err(), "apply must fail on blocker");

        // Originals fully restored, blocker untouched, no stray temps left.
        assert_eq!(fs::read_to_string(&a).unwrap(), "A");
        assert_eq!(fs::read_to_string(&b).unwrap(), "B");
        assert_eq!(fs::read_to_string(&c).unwrap(), "C");
        assert_eq!(fs::read_to_string(&d).unwrap(), "D");
        let leftover: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".__cove_rename_")
            })
            .collect();
        assert!(leftover.is_empty(), "no temp artifacts should remain");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reserved_temp_dir_collision_retries_safely() {
        // Pre-create a directory matching the first generated token so
        // `fs::create_dir` returns AlreadyExists; verify the loop retries
        // with the next token and reserves a fresh, free directory.
        let dir = tmp_dir("reserved_collision");
        let first = "fixed-token-aaaa";
        let second = "fixed-token-bbbb";
        let blocker = dir.join(format!(".__cove_rename_{}", first));
        fs::create_dir(&blocker).unwrap();

        let mut tokens = vec![second.to_string(), first.to_string()];
        let r =
            ReservedTempDir::create_in_with(&dir, || tokens.pop().unwrap()).unwrap();

        assert_ne!(r.path, blocker, "must not return the colliding path");
        assert!(r.path.is_dir(), "reserved dir must exist");
        assert!(
            r.path.to_string_lossy().contains(second),
            "should have advanced past the collision to the next token"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reserved_temp_dir_is_unique_per_creation() {
        // Real-UUID path: two reservations in the same parent must produce
        // distinct, non-overlapping directories.
        let dir = tmp_dir("reserved_unique");
        let r1 = ReservedTempDir::create_in(&dir).unwrap();
        let r2 = ReservedTempDir::create_in(&dir).unwrap();
        assert_ne!(r1.path, r2.path);
        assert!(r1.path.is_dir());
        assert!(r2.path.is_dir());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn successful_apply_cleans_up_temp_dirs() {
        // After a successful batch, no `.__cove_rename_*` directory should
        // remain in the source parent.
        let dir = tmp_dir("apply_cleanup");
        let a = dir.join("a.txt");
        touch(&a, "A");
        let previews = vec![RenamePreviewItem {
            original_path: a.to_string_lossy().to_string(),
            original_name: "a.txt".to_string(),
            new_name: "renamed.txt".to_string(),
            new_path: dir.join("renamed.txt").to_string_lossy().to_string(),
            status: "ok".to_string(),
            message: None,
        }];
        apply_rename(&previews).expect("rename should succeed");
        let leftover: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".__cove_rename_")
            })
            .collect();
        assert!(
            leftover.is_empty(),
            "reserved temp dirs must be removed after a clean apply"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_rename_no_replace_refuses_occupied_destination() {
        // The atomic primitive must surface `DestinationExists` without
        // touching the destination's contents and without removing the
        // source — leaving the temp file fully recoverable.
        let dir = tmp_dir("atomic_dst_exists");
        let src = dir.join("src.txt");
        let dst = dir.join("dst.txt");
        touch(&src, "SRC");
        touch(&dst, "DST_PRESERVED");

        let err = atomic_rename_no_replace(&src, &dst)
            .expect_err("must refuse to overwrite an occupied destination");
        assert!(
            matches!(err, AtomicRenameError::DestinationExists),
            "expected DestinationExists, got {:?}",
            err
        );
        assert_eq!(
            fs::read_to_string(&dst).unwrap(),
            "DST_PRESERVED",
            "destination contents must not be touched"
        );
        assert_eq!(
            fs::read_to_string(&src).unwrap(),
            "SRC",
            "source must remain available for manual recovery"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_rename_no_replace_moves_to_free_destination() {
        // Sanity check: when the destination is free, the helper must move
        // the data and unlink the source.
        let dir = tmp_dir("atomic_dst_free");
        let src = dir.join("src.txt");
        let dst = dir.join("dst.txt");
        touch(&src, "PAYLOAD");

        atomic_rename_no_replace(&src, &dst).expect("must succeed when dst is free");
        assert!(!src.exists(), "source must be unlinked after success");
        assert_eq!(fs::read_to_string(&dst).unwrap(), "PAYLOAD");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_rename_no_replace_moves_directory_to_free_destination() {
        // Directories must be restored as well as regular files. The
        // hard_link-based implementation could not satisfy this; the
        // platform no-replace primitive must.
        let dir = tmp_dir("atomic_dir_free");
        let src = dir.join("src_dir");
        let dst = dir.join("dst_dir");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("inside.txt"), "PAYLOAD").unwrap();

        match atomic_rename_no_replace(&src, &dst) {
            Ok(()) => {
                assert!(!src.exists(), "source dir must be gone after success");
                assert!(dst.is_dir(), "dst dir must exist after success");
                assert_eq!(
                    fs::read_to_string(dst.join("inside.txt")).unwrap(),
                    "PAYLOAD",
                    "directory contents must be preserved verbatim"
                );
            }
            Err(AtomicRenameError::Unsupported) => {
                // Acceptable on platforms without a directory-capable
                // no-replace primitive — the helper must still leave src
                // intact instead of clobbering anything.
                assert!(src.is_dir(), "src dir must be untouched on Unsupported");
                assert!(!dst.exists(), "dst must not be created on Unsupported");
            }
            Err(other) => panic!("unexpected error moving directory: {:?}", other),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_rename_no_replace_refuses_occupied_directory_destination() {
        // An occupied directory destination must surface DestinationExists
        // and leave both the source directory and the destination contents
        // intact.
        let dir = tmp_dir("atomic_dir_exists");
        let src = dir.join("src_dir");
        let dst = dir.join("dst_dir");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("inside.txt"), "SRC_INSIDE").unwrap();
        fs::create_dir(&dst).unwrap();
        fs::write(dst.join("keep.txt"), "DST_PRESERVED").unwrap();

        match atomic_rename_no_replace(&src, &dst) {
            Err(AtomicRenameError::DestinationExists) => {}
            Err(AtomicRenameError::Unsupported) => {
                // Acceptable on unsupported platforms; src/dst must both
                // remain untouched.
            }
            other => panic!("expected DestinationExists or Unsupported, got {:?}", other),
        }
        assert!(src.is_dir(), "source directory must remain available");
        assert_eq!(
            fs::read_to_string(src.join("inside.txt")).unwrap(),
            "SRC_INSIDE"
        );
        assert!(dst.is_dir(), "destination directory must remain in place");
        assert_eq!(
            fs::read_to_string(dst.join("keep.txt")).unwrap(),
            "DST_PRESERVED",
            "destination contents must not be touched"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollback_restores_finalized_directory_when_original_is_free() {
        // Directories arriving from Search/Disk Usage flow must round-trip
        // through rollback the same way regular files do. Before this fix
        // the hard_link-based restore failed for directories and left the
        // original path missing.
        let dir = tmp_dir("rollback_dir_finalized");
        let mut registry = TempRegistry::new();

        let original = dir.join("orig_dir");
        let renamed = dir.join("renamed_dir");
        fs::create_dir(&renamed).unwrap();
        fs::write(renamed.join("payload.txt"), "DIR_PAYLOAD").unwrap();

        let records = vec![RenameRecord {
            original: original.to_string_lossy().to_string(),
            renamed: renamed.to_string_lossy().to_string(),
        }];

        let errors = rollback_full(&mut registry, &records, &[], 0);

        if errors.is_empty() {
            assert!(
                original.is_dir(),
                "original directory must be restored when free"
            );
            assert_eq!(
                fs::read_to_string(original.join("payload.txt")).unwrap(),
                "DIR_PAYLOAD",
                "directory payload must survive the rollback round-trip"
            );
            assert!(!renamed.exists(), "renamed path must be vacated");
            let leftovers = registry.cleanup_empty();
            assert!(
                leftovers.is_empty(),
                "reserved dir must be cleaned up after a successful directory restore"
            );
        } else {
            // Platforms without a directory-capable no-replace primitive
            // must fail safely: original stays missing, payload is parked
            // in the reserved temp dir, and the report names the temp
            // path so the user can recover by hand.
            assert!(!original.exists(), "original must remain untouched on unsupported platforms");
            assert_eq!(errors.len(), 1, "exactly one safe-fail report expected");
            assert!(
                errors[0].contains("FINALIZED") && errors[0].contains("preserved at"),
                "expected recovery report, got: {}",
                errors[0]
            );
            let leftovers = registry.cleanup_empty();
            assert_eq!(
                leftovers.len(),
                1,
                "reserved dir with unrestored content must be preserved"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollback_does_not_overwrite_recreated_finalized_original() {
        // Simulate the finalized-then-recreated case: A was renamed to B
        // and the rename finalized; before rollback runs, another process
        // recreates A. Rollback must leave the recreated A alone and
        // preserve A's original content in the reserved temp dir.
        let dir = tmp_dir("rollback_recreated_finalized");
        let mut registry = TempRegistry::new();

        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        touch(&b, "A_CONTENT_NOW_AT_B");
        touch(&a, "RECREATED_BY_OTHER_PROCESS");

        let records = vec![RenameRecord {
            original: a.to_string_lossy().to_string(),
            renamed: b.to_string_lossy().to_string(),
        }];

        let errors = rollback_full(&mut registry, &records, &[], 0);

        // Recreated A must be untouched.
        assert_eq!(fs::read_to_string(&a).unwrap(), "RECREATED_BY_OTHER_PROCESS");
        // B is now empty (its content was parked in temp).
        assert!(!b.exists());
        // Exactly one error reporting the skipped FINALIZED restore.
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].contains("FINALIZED") && errors[0].contains("occupied"),
            "expected occupied-original report, got: {}",
            errors[0]
        );

        // Cleanup leaves the reserved dir in place because it still holds
        // the unrestored content.
        let leftovers = registry.cleanup_empty();
        assert_eq!(
            leftovers.len(),
            1,
            "reserved dir with unrestored content must be reported"
        );
        let recovered: Vec<String> = fs::read_dir(&leftovers[0])
            .unwrap()
            .filter_map(|e| e.ok())
            .filter_map(|e| fs::read_to_string(e.path()).ok())
            .collect();
        assert!(
            recovered.iter().any(|c| c == "A_CONTENT_NOW_AT_B"),
            "original content must be preserved in the reserved temp dir"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollback_does_not_overwrite_recreated_staged_original() {
        // Simulate the staged-but-not-finalized case: source was moved
        // into a forward staging temp, another process recreated the
        // original, rollback must not overwrite it.
        let dir = tmp_dir("rollback_recreated_staged");
        let mut registry = TempRegistry::new();

        // Reserve the dir up front so we can plant a controlled temp file.
        let reserved_path = registry.reserve_for(&dir).unwrap().path.clone();
        let temp_file = reserved_path.join("s_a.txt");
        touch(&temp_file, "ORIGINAL_A_CONTENT");

        let a = dir.join("a.txt");
        touch(&a, "RECREATED_A");

        let staged = vec![StagedRename {
            original: a.to_string_lossy().to_string(),
            temp: temp_file.to_string_lossy().to_string(),
            final_path: dir.join("b.txt").to_string_lossy().to_string(),
            original_name: "a.txt".to_string(),
        }];

        let errors = rollback_full(&mut registry, &[], &staged, 0);

        assert_eq!(fs::read_to_string(&a).unwrap(), "RECREATED_A");
        assert!(temp_file.exists(), "staged temp must remain for recovery");
        assert_eq!(fs::read_to_string(&temp_file).unwrap(), "ORIGINAL_A_CONTENT");
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].contains("STAGED"),
            "expected STAGED report, got: {}",
            errors[0]
        );

        let leftovers = registry.cleanup_empty();
        assert_eq!(leftovers.len(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollback_restores_safe_items_when_some_originals_occupied() {
        // Two finalized records: one original is recreated, the other isn't.
        // Rollback must restore the safe one and report only the occupied one.
        let dir = tmp_dir("rollback_partial");
        let mut registry = TempRegistry::new();

        let a_orig = dir.join("a.txt");
        let a_renamed = dir.join("a_renamed.txt");
        let b_orig = dir.join("b.txt");
        let b_renamed = dir.join("b_renamed.txt");

        touch(&a_renamed, "A_CONTENT");
        touch(&b_renamed, "B_CONTENT");
        touch(&a_orig, "RECREATED_A"); // occupied — must NOT be overwritten

        let records = vec![
            RenameRecord {
                original: a_orig.to_string_lossy().to_string(),
                renamed: a_renamed.to_string_lossy().to_string(),
            },
            RenameRecord {
                original: b_orig.to_string_lossy().to_string(),
                renamed: b_renamed.to_string_lossy().to_string(),
            },
        ];

        let errors = rollback_full(&mut registry, &records, &[], 0);

        assert_eq!(fs::read_to_string(&a_orig).unwrap(), "RECREATED_A");
        assert_eq!(
            fs::read_to_string(&b_orig).unwrap(),
            "B_CONTENT",
            "B's original was free, so its content should have been restored"
        );
        assert!(!a_renamed.exists());
        assert!(!b_renamed.exists());

        assert_eq!(
            errors.len(),
            1,
            "only A should be reported (B succeeded), got: {:?}",
            errors
        );

        let leftovers = registry.cleanup_empty();
        assert_eq!(leftovers.len(), 1, "reserved dir holds A's content");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_rollback_preserves_temp_dir_for_recovery() {
        // When rollback cannot restore some files because their originals
        // are occupied, the reserved temp dir must NOT be removed — the
        // files inside are the only remaining copy.
        let dir = tmp_dir("rollback_preserve_dir");
        let mut registry = TempRegistry::new();

        let a_orig = dir.join("a.txt");
        let a_renamed = dir.join("a_renamed.txt");
        touch(&a_renamed, "A_CONTENT");
        touch(&a_orig, "RECREATED");

        let records = vec![RenameRecord {
            original: a_orig.to_string_lossy().to_string(),
            renamed: a_renamed.to_string_lossy().to_string(),
        }];

        let _ = rollback_full(&mut registry, &records, &[], 0);
        let leftovers = registry.cleanup_empty();
        assert_eq!(leftovers.len(), 1);

        // Calling cleanup again must not remove the still-non-empty dir.
        assert!(
            leftovers[0].is_dir(),
            "non-empty reserved dir must remain on disk"
        );
        let entries: Vec<_> = fs::read_dir(&leftovers[0])
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1, "the unrestored file is still in the dir");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn temp_paths_live_inside_reserved_directory() {
        // The forward staging temp returned by the registry must live
        // inside the reserved directory, not as a sibling of the source.
        let dir = tmp_dir("temp_inside_reserved");
        let mut registry = TempRegistry::new();
        let host = dir.join("file.txt").to_string_lossy().to_string();
        let temp = registry.child_path(&host, "s_file.txt").unwrap();
        let temp_parent = temp.parent().unwrap();
        assert_ne!(temp_parent, dir.as_path(), "must not be a sibling of source");
        assert_eq!(
            temp_parent.parent().unwrap(),
            dir.as_path(),
            "reserved dir must live inside the source's parent"
        );
        assert!(
            temp_parent
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".__cove_rename_"),
            "reserved dir must use the cove rename prefix"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_ends_trims_both_sides() {
        let rule = RenameRule::RemoveEnds { first: 6, last: 6 };
        assert_eq!(apply_rule("prefix_name_suffix.txt", &rule, 0).unwrap(), "_name_.txt");
    }

    #[test]
    fn remove_ends_first_only() {
        let rule = RenameRule::RemoveEnds { first: 3, last: 0 };
        assert_eq!(apply_rule("abcHello.txt", &rule, 0).unwrap(), "Hello.txt");
    }

    #[test]
    fn remove_ends_last_only() {
        let rule = RenameRule::RemoveEnds { first: 0, last: 4 };
        assert_eq!(apply_rule("Hello_end.txt", &rule, 0).unwrap(), "Hello.txt");
    }

    #[test]
    fn remove_ends_exceeds_length_returns_original() {
        let rule = RenameRule::RemoveEnds { first: 50, last: 50 };
        assert_eq!(apply_rule("short.txt", &rule, 0).unwrap(), "short.txt");
    }

    #[test]
    fn insert_at_zero_acts_like_prefix() {
        let rule = RenameRule::InsertAt { text: "X".to_string(), position: 0 };
        assert_eq!(apply_rule("name.txt", &rule, 0).unwrap(), "Xname.txt");
    }

    #[test]
    fn insert_at_negative_one_inserts_before_ext() {
        let rule = RenameRule::InsertAt { text: "_v2".to_string(), position: -1 };
        assert_eq!(apply_rule("name.txt", &rule, 0).unwrap(), "name_v2.txt");
    }

    #[test]
    fn insert_at_negative_two_before_last_stem_char() {
        let rule = RenameRule::InsertAt { text: "X".to_string(), position: -2 };
        assert_eq!(apply_rule("abcd.txt", &rule, 0).unwrap(), "abcXd.txt");
    }

    #[test]
    fn insert_at_stem_len_inserts_before_ext() {
        let rule = RenameRule::InsertAt { text: "_v2".to_string(), position: 4 };
        assert_eq!(apply_rule("name.txt", &rule, 0).unwrap(), "name_v2.txt");
    }

    #[test]
    fn insert_at_middle() {
        let rule = RenameRule::InsertAt { text: "MID".to_string(), position: 2 };
        assert_eq!(apply_rule("abcd.txt", &rule, 0).unwrap(), "abMIDcd.txt");
    }

    #[test]
    fn insert_at_oob_clamps_to_end_of_stem() {
        let rule = RenameRule::InsertAt { text: "Z".to_string(), position: 99 };
        assert_eq!(apply_rule("abcd.txt", &rule, 0).unwrap(), "abcdZ.txt");
    }

    #[test]
    fn insert_at_empty_text_noop() {
        let rule = RenameRule::InsertAt { text: "".to_string(), position: 0 };
        assert_eq!(apply_rule("abcd.txt", &rule, 0).unwrap(), "abcd.txt");
    }

    #[test]
    fn insert_at_no_extension() {
        let rule = RenameRule::InsertAt { text: "_X".to_string(), position: -1 };
        assert_eq!(apply_rule("README", &rule, 0).unwrap(), "README_X");
    }

    #[test]
    fn case_insensitive_collision_in_batch() {
        // On Linux this is a real collision (different bytes); on macOS/Win
        // the FS folds them together. Both code paths flag it as conflict.
        let previews_two = vec![
            RenamePreviewItem {
                original_path: "/tmp/cove_test/a.txt".to_string(),
                original_name: "a.txt".to_string(),
                new_name: "FOO.txt".to_string(),
                new_path: "/tmp/cove_test/FOO.txt".to_string(),
                status: "ok".to_string(),
                message: None,
            },
            RenamePreviewItem {
                original_path: "/tmp/cove_test/b.txt".to_string(),
                original_name: "b.txt".to_string(),
                new_name: "foo.txt".to_string(),
                new_path: "/tmp/cove_test/foo.txt".to_string(),
                status: "ok".to_string(),
                message: None,
            },
        ];
        if cfg!(any(target_os = "windows", target_os = "macos")) {
            let mut p = previews_two.clone();
            detect_conflicts(&mut p, &[
                "/tmp/cove_test/a.txt".to_string(),
                "/tmp/cove_test/b.txt".to_string(),
            ]);
            assert!(p.iter().all(|x| x.status == "conflict"));
        }
    }
}
