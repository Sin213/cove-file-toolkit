# Handoff: v1.2.8 - diagnostic file logging + local Linux release build

## Release
Version bumped 1.2.7 -> 1.2.8 in `package.json`, `src-tauri/Cargo.toml`,
`src-tauri/tauri.conf.json`, `src-tauri/Cargo.lock`.

## Packaging change
`scripts/build-release-linux.sh` (new) builds the AppImage and .deb locally,
deletes the crashing `libgiognutls.so` from the AppDir, repacks, smoke tests that
the AppImage actually launches (refusing to stage a build that exits early), and
stages to `release/` with the same artifact names the CI job used, one `.sha256`
sidecar per artifact. Upload is opt-in via `--upload`.

The `linux` job was removed from `.github/workflows/release.yml`; the `windows`
job stays, since Setup.exe/Portable.exe cannot be built on the Linux host. The
Linux job was not just redundant but harmful: it would upload an unrepaired
AppImage over a working one. The published 1.2.7 AppImage was already a local
build (it requires GLIBC_2.39, which ubuntu-22.04 cannot produce).

## Summary
`open_path` and `reveal_in_folder` intermittently stop working: every entry point
(double-click, Enter, right-click Open, right-click Open containing folder) dies at
once while the rest of the app keeps working, and only an app restart clears it. The
root cause is NOT fixed by this patch and is not yet known. This patch adds the
instrumentation needed to find it, because the app's existing diagnostics were being
discarded: Cove Nexus launches child apps with stdin/stdout/stderr pointed at
/dev/null, so every `eprintln!` in the app was lost and the webview console was
unreachable in a packaged build.

## Evidence that shaped the patch
Gathered live against a wedged instance:
- `search` works end-to-end while opens are dead, so the IPC transport is fine.
- During activation attempts the main process burns ZERO CPU and forks no `xdg-open`
  (confirmed by CPU sampling and 10ms polling of `/proc/<pid>/task/*/children`).
- No error is returned: the "Could not open" banner renders correctly
  (`Search.svelte:946`), so a backend `Err` would be visible. Nothing appears.
- Clicks reach the webview (rows highlight, context menu opens, "Copy path" works).
- tokio's blocking pool is idle, not saturated; no threads blocked in a syscall.

Ruled out with evidence: the open code itself, environment differences (the AppImage
was run under Nexus's exact 45-var env and worked), `xdg-open`, blocking filesystem
calls, blocking-pool exhaustion, a stuck full-screen overlay, disabled menu items, the
async runtime, and the IPC transport.

## Changes

### Backend (Rust)
- `src-tauri/src/logging.rs` (new): log file at
  `~/.local/share/cove-file-toolkit/logs/cove-file-toolkit.log` (portable installs use
  `cove-app-data/cove-file-toolkit/logs/`). When stderr is not a terminal, `dup2`s fds
  1 and 2 onto the log file so every existing `eprintln!` call site is captured with no
  call-site edits; a terminal run is left alone so `tauri dev` still prints to the
  console. Rotates at 5 MB keeping one `.1` generation. `log_line()` appends directly to
  the file so it also works on Windows, where no redirect happens.
- `src-tauri/src/main.rs`: `mod logging;`, `logging::init()` as the first statement in
  `main`, and registration of the `log_frontend` command.
- `src-tauri/src/ipc.rs`: `log_frontend` command so the webview can write into the same
  file.

### Frontend
- `src/lib/ipc.ts`: `logToFile()` helper plus `dispatch`/`resolved`/`rejected` logging
  inside the `openPath` and `revealInFolder` wrappers, so every caller (double-click,
  Enter, context menu, Disk Usage) is covered by one edit. The log calls are
  deliberately NOT awaited: if the IPC channel is the thing misbehaving, awaiting a log
  call would stall the operation under observation.
- `src/views/Search.svelte`: one `logToFile` line in `activateItem` recording that the
  activation handler ran at all, which distinguishes "handler never fired" from
  "command never arrived".

## How to read the log
| Last line present | Meaning |
|---|---|
| nothing | UI handler never fired |
| `[ui:search:activate]` | handler ran, `openPath` never called |
| `[ui:openPath] dispatch` | command never reached Rust (IPC) |
| `[open_path] requested` | reached the backend; following lines say why it stopped |
| `spawn err` / `rejected` | failed properly and the UI is hiding the error |

## Verification
- `cargo build`: pass
- `cargo clippy --all-targets`: 0 errors; warnings are pre-existing (diskusage.rs,
  walker.rs, index.rs), none in the new code
- `cargo test`: 38 passed, 0 failed, 2 ignored
- `npx tsc --noEmit`: 2 errors, both pre-existing in `src/main.ts` (confirmed identical
  with these changes stashed); none in the changed files
- `npm run build` (Vite, compiles Svelte): pass
- Runtime check on a real build: the log captured the full chain end-to-end -
  `[ui:search:activate]` -> `[ui:openPath] dispatch` -> `[open_path] requested` ->
  `[open_path] spawn ok` -> `[ui:openPath] resolved`, including a path containing `!`.

## Deliberate decisions
- **The log records full file paths on purpose.** Which path failed is the diagnostic
  signal itself - the first captured trace was for a path containing `!`, exactly the
  kind of detail a redacted log would have destroyed. This also does not add any new
  disclosure: `open_path`/`reveal_in_folder` already printed these paths via `eprintln!`,
  so the patch changes the sink, not the content, and the file lives in the user's own
  data directory on a single-user machine. If sharing logs for support ever becomes
  routine, the right answer is a diagnostic-mode toggle, not blanket redaction that
  would defeat the instrument.
- **Rotation is a checkpoint, not a hard ceiling.** The size is tested at startup and on
  every `log_line` write (which re-points the redirected descriptors after rolling
  over), but output captured through the redirected fds reaches the file without
  passing that check, so a burst between two logged actions can overshoot until the
  next check. Documented as such rather than adding a rotation thread to a diagnostic
  tool; revisit if a log is ever observed growing.

## Not in this patch
- The root cause of the wedge. This patch is the instrument for finding it; the next
  occurrence should be diagnosed from the log.

## Additional verification for the packaging change
- `scripts/build-release-linux.sh` run end to end: build -> module removal -> repack ->
  smoke test passed (still running after 20s) -> staged 4 files with sidecars.
- Staged AppImage re-extracted and inspected: `usr/lib/gio/modules` is empty and the
  logging strings are present in the packaged binary.
- `cargo build`, `cargo test` (38 passed), `cargo clippy` (0 errors) re-run after the
  version bump.
