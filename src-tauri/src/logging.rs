//! Persistent file logging.
//!
//! Launchers like Cove Nexus start child apps with stdin/stdout/stderr pointed
//! at /dev/null, so every `eprintln!` diagnostic in this app is discarded and a
//! user-reported bug leaves no trace behind. When stderr is not a terminal we
//! point fds 1 and 2 at a log file instead, which captures every existing
//! `eprintln!` site (and WebKitGTK's own warnings) without touching a single
//! call site. A terminal run is left alone so `tauri dev` still prints to the
//! console.

use std::path::PathBuf;

/// Roll the log over at roughly this size so a long-lived install cannot fill the
/// disk. One previous generation is kept as `<name>.1`.
///
/// This is a checkpoint, not a hard ceiling: the size is tested at startup and on
/// every `log_line` write, but output captured through the redirected fds (plain
/// `eprintln!`, WebKitGTK warnings) reaches the file without passing through that
/// check. A burst of those between two logged actions can overshoot the cap until
/// the next check rolls it over.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

pub fn log_dir() -> Option<PathBuf> {
    if crate::portable::is_portable() {
        Some(crate::portable::portable_data_dir("cove-file-toolkit").join("logs"))
    } else {
        dirs::data_dir().map(|d| d.join("cove-file-toolkit").join("logs"))
    }
}

pub fn log_path() -> Option<PathBuf> {
    log_dir().map(|d| d.join("cove-file-toolkit.log"))
}

fn epoch_millis() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Prepare the log file and, on a detached (non-terminal) launch, redirect the
/// process's stdout/stderr into it. Safe to call exactly once, early in `main`.
pub fn init() {
    let Some(path) = log_path() else { return };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
        // Owner-only: the log holds full filesystem paths, and a portable install
        // can sit in a directory other local users can reach.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).ok();
        }
    }
    rotate_if_large(&path);

    #[cfg(unix)]
    redirect_stdio(&path);

    log_line(
        "session",
        &format!(
            "start pid={} version={}",
            std::process::id(),
            env!("CARGO_PKG_VERSION")
        ),
    );
}

/// Roll the log over if it has outgrown the cap. Returns whether it rotated, so
/// callers can re-point the redirected descriptors at the new file.
fn rotate_if_large(path: &std::path::Path) -> bool {
    let too_big = std::fs::metadata(path)
        .map(|m| m.len() > MAX_LOG_BYTES)
        .unwrap_or(false);
    if too_big {
        let backup = path.with_extension("log.1");
        // Windows' rename fails if the destination exists, which would silently
        // wedge rotation after the first rollover and let the log grow unbounded.
        std::fs::remove_file(&backup).ok();
        return std::fs::rename(path, &backup).is_ok();
    }
    false
}

/// Open the log for appending, owner-only on unix.
fn open_log(path: &std::path::Path) -> Option<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path).ok()?;
    // `mode` only applies when the file is created, so tighten a pre-existing log
    // (or one left by an older build) as well.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600)).ok();
    }
    Some(file)
}

/// Point the standard descriptors at the log file. Only when stderr is not a
/// terminal: an interactive run should keep printing to the console.
#[cfg(unix)]
fn redirect_stdio(path: &std::path::Path) {
    use std::io::IsTerminal;
    use std::os::unix::io::AsRawFd;

    if std::io::stderr().is_terminal() {
        return;
    }
    let Some(file) = open_log(path) else {
        return;
    };

    // SAFETY: `fd` is a live descriptor owned by `file`. dup2 makes fds 1 and 2
    // refer to the same open file description, which stays alive after `file`
    // is dropped because the duplicates hold their own reference.
    unsafe {
        let fd = file.as_raw_fd();
        libc::dup2(fd, libc::STDOUT_FILENO);
        libc::dup2(fd, libc::STDERR_FILENO);
    }
}

/// Append one line to the log file.
///
/// This writes to the file directly rather than going through `eprintln!` so it
/// works on Windows too, where stdio is never redirected. Logging must never be
/// able to break the caller, so every failure here is swallowed.
pub fn log_line(source: &str, message: &str) {
    use std::io::Write;

    let Some(path) = log_path() else { return };

    // Enforce the cap at write time, not only at startup. Both this function and
    // the redirected stdout/stderr append for the whole life of the process, so a
    // long-lived session would otherwise sail past MAX_LOG_BYTES until the next
    // launch. After rotating, the redirected descriptors still point at the
    // renamed file, so they have to be re-pointed at the fresh one.
    if rotate_if_large(&path) {
        #[cfg(unix)]
        redirect_stdio(&path);
    }

    let Some(mut file) = open_log(&path) else {
        return;
    };
    writeln!(file, "[{}] [{source}] {message}", epoch_millis()).ok();
}
