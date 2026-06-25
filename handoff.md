# Handoff: Delete files feature + version bump to 1.2.4

## Summary
Add ability to delete selected files from Search and Disk Usage views with confirmation prompts. Supports trash (Delete key, context menu) and permanent delete (Shift+Delete, context menu). Version bumped to 1.2.4.

## Changes

### Backend (Rust)
- `src-tauri/src/ipc.rs`: Added `delete_permanently` command (handles files and directories). Added `freedesktop_trash_fallback` for Linux trash (bypasses `trash` crate due to mount-point detection issues). Added `format_trash_date` helper. Added cross-device rename fallback (copy+delete on EXDEV). Changed `move_to_trash` to use manual fallback on Linux.
- `src-tauri/src/main.rs`: Registered `delete_permanently` command.
- `src-tauri/Cargo.toml`: Version bump to 1.2.4.
- `src-tauri/Cargo.lock`: Updated lockfile for version bump.
- `src-tauri/tauri.conf.json`: Version bump to 1.2.4.

### Frontend
- `src/lib/ipc.ts`: Added `deletePermanently()` wrapper.
- `src/views/Search.svelte`: Added ConfirmDialog import, confirmDialog state, `handleTrash`/`handleDeletePermanently`/`getSelectedPaths` functions, Delete/Shift+Delete keyboard handler in `onKey`, trash + permanent delete context menu items, ConfirmDialog template.
- `src/views/DiskUsage.svelte`: Added `deletePermanently` import, `handleDeletePermanently` function, permanent delete context menu item, `svelte:window` keyboard handler for Delete/Shift+Delete.
- `package.json`: Version bump to 1.2.4.

## Verification
- `cargo check -p cove-file-toolkit`: pass
- `npx vite build`: pass
- Manual testing: trash works, permanent delete works, confirmation dialogs appear, keyboard shortcuts work
