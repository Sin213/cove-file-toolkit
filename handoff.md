# Handoff: Windows compatibility + performance (4 commits)

## Summary
Cross-platform fixes and performance optimizations from Windows testing session. Removes hardcoded Linux paths, adds native trash on Windows/macOS, and gathers file metadata in jwalk's parallel read phase to avoid serial per-file handle opens (critical on Windows where Defender intercepts each `CreateFileW`).

## Commits reviewed
- `901126c` fix: use native trash crate on Windows/macOS, keep Linux fallback
- `8dfef47` fix: remove hardcoded /home fallback that broke Windows
- `be6d87d` perf: avoid per-file handle opens on Windows; fix corner resize + treemap labels
- `4ce64cf` perf: gather file metadata during jwalk's parallel read phase

## Changes

### Backend (Rust)
- `src-tauri/src/ipc.rs`: Added `default_root()` IPC command returning the user's home directory (via `dirs::home_dir()`). Split `move_to_trash` into platform-conditional versions: Linux keeps the custom freedesktop fallback, Windows/macOS uses the `trash` crate's native API. Added `#[cfg]` guards on `freedesktop_trash_fallback` and `format_trash_date` (Linux-only).
- `src-tauri/src/main.rs`: Registered `default_root` command.
- `src-tauri/src/walker.rs`: Added `entry_size_mtime()` to gather (size, mtime) from dir entries. On Windows uses `GetFileAttributesExW` (no file handle opened, avoids Defender overhead). Non-Windows falls back to `metadata()` (lstat). Updated `walk_directories` to use `WalkDirGeneric::<((), (u64, i64))>` with `process_read_dir` gathering metadata in parallel. Consumer loop now reads `entry.client_state` instead of calling `metadata()`.
- `src-tauri/src/diskusage.rs`: Same parallel metadata pattern as walker.rs. `process_read_dir` now calls `entry_size_mtime()` for files and prunes excluded dirs. Consumer loop reads `entry.client_state` instead of per-file `metadata()`.

### Frontend
- `src/App.svelte`: Added `defaultRoot` import from ipc. Resolves `homeRoot` on mount via `defaultRoot()`. Changed fallback from hardcoded `"/home"` to `homeRoot`. Widened corner resize handles from 4px to 6px, increased edge offsets from 8px to 16px.
- `src/views/Search.svelte`: Added `defaultRoot` import. Changed root fallback from hardcoded `"/home"` to async `defaultRoot()` call.
- `src/lib/ipc.ts`: Added `defaultRoot()` Tauri invoke wrapper.
- `src/views/DiskUsage.svelte`: Minor label adjustments (treemap).
- `src/lib/components/Treemap.svelte`: Label rendering adjustments.

## Verification
- `cargo build`: pass (Linux cross-compile; Windows-specific code behind `#[cfg(windows)]`)
- `cargo clippy --all-targets`: warnings only (all pre-existing, none from new code)
- `cargo test`: 33 passed, 0 failed
- `svelte-check`: 4 errors (all pre-existing type issues unrelated to these changes)
- `.gitignore`: added pnpm-lock.yaml and pnpm-workspace.yaml entries
