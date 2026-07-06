//! In-app auto-updater backed by the GitHub releases API.
//!
//! Fleet semantics (same as the Python UpdateController and
//! electron-updater used by the other Cove apps): poll releases/latest,
//! and for AppImage installs download the new release asset, verify it
//! against its `.sha256` sidecar, install it NEXT TO the running
//! AppImage under its own versioned filename, remove the old file, and
//! relaunch. Keeping the release asset's filename keeps the on-disk name
//! truthful - external launchers like Cove Nexus derive the installed
//! version from it. Non-AppImage builds only report the release URL so
//! the frontend can open the release page.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Duration;

const REPO: &str = "Sin213/cove-file-toolkit";
const UA: &str = "cove-file-toolkit-updater";

#[derive(Serialize, Clone)]
pub struct UpdateInfo {
    pub latest_version: String,
    pub release_url: String,
    pub asset_name: Option<String>,
    pub asset_url: Option<String>,
    pub sha256_url: Option<String>,
    pub can_auto_install: bool,
}

fn parse_version(v: &str) -> (u64, u64, u64) {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let mut nums = v.split('.').map(|part| {
        part.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u64>()
            .unwrap_or(0)
    });
    (
        nums.next().unwrap_or(0),
        nums.next().unwrap_or(0),
        nums.next().unwrap_or(0),
    )
}

fn version_newer(latest: &str, current: &str) -> bool {
    parse_version(latest) > parse_version(current)
}

fn appimage_path() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE").map(PathBuf::from)
}

fn fetch_latest_release() -> Result<serde_json::Value, String> {
    let resp = ureq::get(&format!(
        "https://api.github.com/repos/{REPO}/releases/latest"
    ))
    .set("Accept", "application/vnd.github+json")
    .set("User-Agent", UA)
    .timeout(Duration::from_secs(8))
    .call()
    .map_err(|e| e.to_string())?;
    resp.into_json().map_err(|e| e.to_string())
}

fn check_inner() -> Result<Option<UpdateInfo>, String> {
    let data = fetch_latest_release()?;
    let tag = data["tag_name"].as_str().unwrap_or("");
    let latest = tag.trim_start_matches(['v', 'V']).to_string();
    if latest.is_empty() || !version_newer(&latest, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let release_url = data["html_url"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://github.com/{REPO}/releases/tag/{tag}"));

    let mut asset_name = None;
    let mut asset_url = None;
    let mut sha256_url = None;
    if let Some(assets) = data["assets"].as_array() {
        let appimage = assets.iter().find(|a| {
            let name = a["name"].as_str().unwrap_or("").to_lowercase();
            name.ends_with(".appimage")
        });
        if let Some(asset) = appimage {
            let name = asset["name"].as_str().unwrap_or("").to_string();
            let sidecar_name = format!("{name}.sha256").to_lowercase();
            sha256_url = assets
                .iter()
                .find(|a| a["name"].as_str().unwrap_or("").to_lowercase() == sidecar_name)
                .and_then(|a| a["browser_download_url"].as_str())
                .map(str::to_string);
            asset_url = asset["browser_download_url"].as_str().map(str::to_string);
            asset_name = Some(name);
        }
    }

    let can_auto_install = appimage_path().is_some()
        && asset_url.is_some()
        && sha256_url.is_some();
    Ok(Some(UpdateInfo {
        latest_version: latest,
        release_url,
        asset_name,
        asset_url,
        sha256_url,
        can_auto_install,
    }))
}

fn download_to(url: &str, dest: &std::path::Path) -> Result<(), String> {
    let resp = ureq::get(url)
        .set("User-Agent", UA)
        .timeout(Duration::from_secs(600))
        .call()
        .map_err(|e| e.to_string())?;
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    std::io::copy(&mut reader, &mut file).map_err(|e| {
        let _ = std::fs::remove_file(dest);
        e.to_string()
    })?;
    Ok(())
}

fn fetch_sidecar_hash(url: &str) -> Result<String, String> {
    let resp = ureq::get(url)
        .set("User-Agent", UA)
        .timeout(Duration::from_secs(20))
        .call()
        .map_err(|e| e.to_string())?;
    use std::io::Read;
    let mut text = String::new();
    // Sidecars are tiny; cap the read so a hostile redirect can't dump
    // unbounded bytes.
    resp.into_reader()
        .take(4096)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let token = line.split_whitespace().next().unwrap_or("");
        if token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(token.to_lowercase());
        }
        return Err(format!("unrecognized sidecar contents: {line:?}"));
    }
    Err("empty sidecar".into())
}

fn sha256_of_file(path: &std::path::Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Spawn `path` detached with the AppImage loader env restored/scrubbed
/// so the new binary starts with a clean environment (same rationale as
/// `spawn_open` in ipc.rs).
fn relaunch(path: &std::path::Path) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(path);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        for key in [
            "LD_LIBRARY_PATH",
            "LD_PRELOAD",
            "GDK_PIXBUF_MODULE_FILE",
            "GDK_PIXBUF_MODULEDIR",
            "PYTHONPATH",
        ] {
            if let Ok(orig) = std::env::var(format!("APPIMAGE_ORIGINAL_{key}")) {
                cmd.env(key, orig);
            } else if std::env::var("APPIMAGE").is_ok() {
                cmd.env_remove(key);
            }
        }
        cmd.env_remove("APPIMAGE");
        cmd.env_remove("APPDIR");
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

fn install_inner(asset_name: &str, asset_url: &str, sha256_url: &str) -> Result<(), String> {
    let old = appimage_path().ok_or("APPIMAGE env var not set - not an AppImage install")?;
    let old = std::fs::canonicalize(&old).unwrap_or(old);
    let dir = old
        .parent()
        .ok_or("running AppImage has no parent directory")?;

    let cache = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("cove-file-toolkit");
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let downloaded = cache.join(asset_name);
    download_to(asset_url, &downloaded)?;

    let expected = fetch_sidecar_hash(sha256_url).map_err(|e| {
        let _ = std::fs::remove_file(&downloaded);
        format!("could not fetch sidecar: {e}")
    })?;
    let actual = sha256_of_file(&downloaded).inspect_err(|_| {
        let _ = std::fs::remove_file(&downloaded);
    })?;
    if actual != expected {
        let _ = std::fs::remove_file(&downloaded);
        return Err(format!("sha256 mismatch: expected {expected}, got {actual}"));
    }

    // Install under the new versioned filename next to the running file.
    let target = dir.join(asset_name);
    let tmp = dir.join(format!(".{asset_name}.part"));
    // fs::rename fails across filesystems (cache is usually on the same
    // one, but not guaranteed); copy + remove is always safe.
    std::fs::copy(&downloaded, &tmp).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&downloaded);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
    if target != old {
        // Unlinking the running file is fine on Linux; the kernel keeps
        // the mmap alive until we exit.
        let _ = std::fs::remove_file(&old);
    }
    relaunch(&target)
}

#[tauri::command]
pub async fn updater_check() -> Result<Option<UpdateInfo>, String> {
    tauri::async_runtime::spawn_blocking(check_inner)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn updater_install(
    app: tauri::AppHandle,
    asset_name: String,
    asset_url: String,
    sha256_url: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        install_inner(&asset_name, &asset_url, &sha256_url)
    })
    .await
    .map_err(|e| e.to_string())??;
    // The new version is already relaunching; hand over.
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(version_newer("1.3.0", "1.2.9"));
        assert!(version_newer("2.0.0", "1.9.9"));
        assert!(!version_newer("1.2.0", "1.2.0"));
        assert!(!version_newer("1.2", "1.2.1"));
        assert!(version_newer("v1.3.0", "1.2.5"));
    }
}
