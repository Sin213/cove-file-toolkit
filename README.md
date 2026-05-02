# Cove Toolkit v1.0.0

Dense desktop file utility built with Tauri 2.x, Svelte 5, and Rust. An
all-in-one tool for filename search, disk-usage analysis, and bulk renaming —
inspired by Everything, WizTree, and Bulk Rename Utility, in Cove style.

## Features

- **Search (Everything-style)** — Multi-root indexed file search with persistent
  cache, wildcards (`*` `?`), case-insensitive name+path matching, dense
  results table, double-click open, right-click context menu, force rebuild,
  status bar with live counters.
- **Disk Usage (WizTree-style)** — Tree-style folder/file table with %-of-parent
  bars, sortable columns, expand/collapse, breadcrumbs, side panels for file
  types and largest files, summary metrics.
- **Bulk Rename (BRU-style)** — 4-column rule grid (RegEx, Replace, Remove,
  Add, Case, Numbering, Extension, Filters), live preview with old → new
  comparison, conflict/error detection, apply locked when invalid, undo
  preserved.
- **Settings** — Indexed-roots manager, excluded folders, search behavior
  (case-sensitive, match path, auto-load cache), cache info and clear.

## Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [Node.js](https://nodejs.org/) >= 18
- Tauri system dependencies:
  - **Linux**: `sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev`
  - **Windows**: WebView2 (bundled with Windows 10/11)

## Development

```bash
npm install
npm run tauri dev
```

## Build

```bash
npm run tauri build
```

Build artifacts are placed in `src-tauri/target/release/bundle/`.

### Artifact matrix

| Platform | Artifact | Path | Status |
|----------|----------|------|--------|
| Linux | AppImage | `bundle/appimage/cove-file-toolkit_0.1.0_amd64.AppImage` | Ready |
| Linux | .deb | `bundle/deb/cove-file-toolkit_0.1.0_amd64.deb` | Ready |
| Windows | Setup.exe (NSIS) | `bundle/nsis/Cove File Toolkit_0.1.0_x64-setup.exe` | Configured — requires Windows host |
| Windows | Portable .exe | — | Not defined in this repo |

> **Windows notes:** The NSIS installer target is configured but there is no cross-compilation setup (no `cargo-xwin`, no CI). Build on a Windows machine with Rust + Node.js installed. A standalone portable `.exe` is not a defined packaging path — Tauri does not produce one by default.

### Checksums

Every release binary ships with a `.sha256` sidecar:

```bash
sha256sum <artifact> > <artifact>.sha256
```

## Runtime dependencies

- **Linux**: WebKitGTK 4.1 (`libwebkit2gtk-4.1`), GTK 3
- **Windows**: WebView2 (bundled with Windows 10/11)

## Known limitations

- No CI/CD pipeline — builds are local only
- No Windows cross-compilation from Linux
- No auto-update integration
- Index cache stores absolute paths — not portable across machines

## Architecture

- **Backend (Rust)**: SoA file index with string-interner, parallel jwalk scanning, paged search with globset, DiskUsage tree with bottom-up accumulation, rename engine with rollback, JSON cache persistence
- **Frontend (Svelte 5)**: Runes-based reactivity, virtual scrolling table, Tauri event listeners for scan progress, debounced search and rename preview
