<script lang="ts">
  import "./app.css";
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { listen } from "@tauri-apps/api/event";
  import { getVersion } from "@tauri-apps/api/app";
  import Search from "./views/Search.svelte";
  import DiskUsage from "./views/DiskUsage.svelte";
  import Rename from "./views/Rename.svelte";
  import Settings from "./lib/components/Settings.svelte";
  import coveIcon from "./lib/assets/cove_icon.png";
  import {
    getSettings,
    getCacheInfo,
    loadCachedIndex,
    getIndexStats,
    getIndexRoots,
    getIndexScanState,
    defaultRoot,
    type AppSettings,
    type CacheInfo,
  } from "./lib/ipc";
  import {
    initScanListeners,
    refreshIndexStats,
    getIndexState,
    getDiskState,
    syncDiskScanState,
    syncIndexScanState,
    startDiskWatchdog,
  } from "./lib/scanStore.svelte";

  type TabId = "search" | "disk" | "rename";
  const tabs: { id: TabId; label: string; key: string; icon: string }[] = [
    { id: "search", label: "Search", key: "1", icon: "search" },
    { id: "disk", label: "Disk Usage", key: "2", icon: "disk" },
    { id: "rename", label: "Rename", key: "3", icon: "rename" },
  ];

  let activeTab = $state<TabId>("search");
  let showSettings = $state(false);
  let settings = $state<AppSettings | null>(null);
  // OS-correct fallback root (home dir), resolved from the backend on startup.
  let homeRoot = $state("");
  let cacheInfo = $state<CacheInfo | null>(null);
  let isMaximized = $state(false);
  let appVersion = $state("");

  let renameFiles = $state<Array<{ name: string; path: string }>>([]);

  let globalShortcut = $state("");

  // One-time per-session toast hint when the X button hides to tray, so the
  // first close doesn't feel like a freeze. Shown once per app launch.
  let trayHintVisible = $state(false);
  let trayHintShown = false;
  let trayHintTimer: ReturnType<typeof setTimeout> | null = null;
  function showTrayHintOnce() {
    if (trayHintShown) return;
    trayHintShown = true;
    trayHintVisible = true;
    if (trayHintTimer) clearTimeout(trayHintTimer);
    trayHintTimer = setTimeout(() => (trayHintVisible = false), 6000);
  }

  async function winMinimize() {
    try {
      await getCurrentWindow().minimize();
    } catch (e) {
      console.error("minimize failed:", e);
    }
  }
  async function winToggleMaximize() {
    try {
      const w = getCurrentWindow();
      await w.toggleMaximize();
      isMaximized = await w.isMaximized();
    } catch (e) {
      console.error("toggleMaximize failed:", e);
    }
  }
  async function winClose() {
    try {
      await getCurrentWindow().close();
    } catch (e) {
      console.error("close failed:", e);
    }
  }
  async function refreshMaxState() {
    try {
      isMaximized = await getCurrentWindow().isMaximized();
    } catch {
      /* ignore */
    }
  }

  type ResizeDir =
    | "East"
    | "West"
    | "North"
    | "South"
    | "NorthEast"
    | "NorthWest"
    | "SouthEast"
    | "SouthWest";

  async function onResize(e: MouseEvent, dir: ResizeDir) {
    if (isMaximized) return;
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    try {
      await getCurrentWindow().startResizeDragging(dir);
    } catch (err) {
      console.error("startResizeDragging failed:", err);
    }
  }

  function sendToRename(files: Array<{ name: string; path: string }>) {
    renameFiles = files;
    activeTab = "rename";
  }

  async function refreshSettings() {
    try {
      settings = await getSettings();
    } catch {
      settings = null;
    }
    try {
      cacheInfo = await getCacheInfo();
    } catch {
      cacheInfo = null;
    }
    // Sync into the global scan store so its banner can reflect cache info.
    const idx = getIndexState();
    idx.cacheInfo = cacheInfo;
  }

  function handleKey(e: KeyboardEvent) {
    if (e.ctrlKey && (e.key === "f" || e.key === "F")) {
      e.preventDefault();
      activeTab = "search";
      globalShortcut = "focus-search";
      setTimeout(() => (globalShortcut = ""), 50);
      return;
    }
    if (e.ctrlKey && (e.key === "r" || e.key === "R")) {
      e.preventDefault();
      globalShortcut = "refresh";
      setTimeout(() => (globalShortcut = ""), 50);
      return;
    }
    if (e.ctrlKey && (e.key === ",")) {
      e.preventDefault();
      showSettings = true;
      return;
    }
    if (e.altKey && /^[1-3]$/.test(e.key)) {
      e.preventDefault();
      const t = tabs.find((t) => t.key === e.key);
      if (t) activeTab = t.id;
    }
  }

  async function autoLoadCacheOnStartup(): Promise<void> {
    const idx = getIndexState();
    // Frontend gate: don't even ask the backend if we already know we're
    // indexing here.
    if (idx.status === "indexing") {
      console.info("[cove] cache auto-load: skipped, indexing already in progress");
      await refreshIndexStats();
      return;
    }
    // Already ready (e.g., HMR re-mount): nothing to do.
    if ((idx.stats?.total ?? 0) > 0) {
      console.info("[cove] cache auto-load: skipped, index already populated");
      return;
    }

    // Authoritative backend probe: on hard reload the frontend can be idle
    // while a backend index job is still running. Skip auto-load in that
    // case so we never clobber an active scan's eventual merge.
    try {
      const scanState = await getIndexScanState();
      if (scanState.is_running) {
        console.info(
          "[cove] cache auto-load: skipped, backend index job is active",
        );
        await syncIndexScanState();
        return;
      }
    } catch (e) {
      console.warn("[cove] cache auto-load: get_index_scan_state failed:", e);
    }

    // Fresh probe straight from the backend — independent of any reactive
    // variable on the frontend.
    let probed: CacheInfo | null = null;
    try {
      probed = await getCacheInfo();
    } catch (e) {
      console.warn("[cove] cache auto-load: get_cache_info IPC failed:", e);
      await refreshIndexStats();
      return;
    }
    if (!probed) {
      console.info("[cove] cache auto-load: no cache file present");
      await refreshIndexStats();
      return;
    }

    // Cache file exists. Try to load it.
    idx.status = "loading-cache";
    try {
      const resp = await loadCachedIndex();
      if (resp.status === "skipped_indexing") {
        // Backend rejected the load because a scan is in flight. Don't
        // surface a scary error; pull authoritative state so the UI
        // continues to reflect indexing.
        console.info(
          "[cove] cache auto-load: backend skipped (indexing active)",
        );
        idx.status = "idle";
        idx.error = "";
        await syncIndexScanState();
        return;
      }
      if (resp.status === "skipped_empty") {
        // Cache existed but the enabled-root filter dropped every entry.
        // Treat the same as "no usable cache" — leave Search at idle so the
        // user gets the "No index loaded" affordance instead of an empty
        // ready state.
        console.info(
          "[cove] cache auto-load: cache had zero entries after root filter",
        );
        idx.status = "idle";
        idx.error = "";
        await refreshIndexStats();
        return;
      }
      idx.stats = await getIndexStats();
      idx.cacheInfo = await getCacheInfo();
      idx.rootViews = await getIndexRoots();
      // Defensive double-check: if stats came back empty for any reason,
      // don't mark Search ready over a phantom index.
      if ((idx.stats?.total ?? 0) === 0) {
        console.info(
          "[cove] cache auto-load: post-load stats show zero entries; staying idle",
        );
        idx.status = "idle";
        idx.error = "";
        return;
      }
      idx.status = "ready";
      idx.error = "";
      console.info(
        `[cove] cache auto-load: ready, ${idx.stats?.total ?? 0} entries from ${
          idx.cacheInfo?.roots?.length ?? 0
        } root(s)`,
      );
    } catch (e) {
      // Schema mismatch, missing data file, etc. Surface the reason in the
      // console but keep the UI in a clean "No index loaded" state — the
      // user can rebuild from the button.
      console.warn("[cove] cache auto-load: load_cached_index failed:", e);
      idx.status = "idle";
      idx.error = "";
      await refreshIndexStats();
    }
  }

  onMount(async () => {
    appVersion = await getVersion();
    // Resolve the OS-correct default root (home dir) for use as a fallback.
    try {
      homeRoot = await defaultRoot();
    } catch {
      homeRoot = "";
    }
    // 1. Subscribe to backend events ONCE — survives tab switches.
    await initScanListeners();
    // 2. Load settings + cache info.
    await refreshSettings();
    // 3. Auto-load cache on startup (Everything-style). The manual Load
    //    Cache button in Search remains available either way.
    //
    //    Gate is "not explicitly disabled" rather than "===true" so a
    //    transient `getSettings()` failure (settings === null) or a legacy
    //    settings file missing the field still attempts the load. The
    //    backend default for `auto_load_cache` is true; matching that here
    //    eliminates the silent-skip failure mode where startup looked idle
    //    even though a perfectly good cache existed on disk.
    if (settings?.auto_load_cache !== false) {
      await autoLoadCacheOnStartup();
    } else {
      console.info(
        "[cove] cache auto-load: skipped, auto_load_cache setting is off",
      );
    }
    // 4. Recover authoritative scan state from the backend (handles
    //    missed completion events, tab-switch races, fast scans finishing
    //    before the listener was wired).
    await syncIndexScanState();
    await syncDiskScanState();
    // If a disk scan was already running when the frontend booted, start
    // the defensive watchdog so the UI never gets stuck in scanning state.
    if (getDiskState().status === "scanning") startDiskWatchdog();
    // 5. Track window maximize state for the toggle icon.
    await refreshMaxState();
    // Backend emits this when the X button is intercepted and the window is
    // hidden to tray. Show a one-time toast so users don't think the app froze.
    try {
      await listen("cove://close-to-tray", () => showTrayHintOnce());
    } catch (e) {
      console.warn("[cove] could not subscribe to close-to-tray event:", e);
    }
    try {
      const unlistenResize = await getCurrentWindow().onResized(() =>
        refreshMaxState(),
      );
      window.addEventListener("keydown", handleKey);
      return () => {
        window.removeEventListener("keydown", handleKey);
        unlistenResize();
      };
    } catch {
      window.addEventListener("keydown", handleKey);
      return () => window.removeEventListener("keydown", handleKey);
    }
  });
</script>

<main class="app">
  <header class="topbar" data-tauri-drag-region>
    <div class="brand" data-tauri-drag-region>
      <img class="brand-icon" src={coveIcon} alt="Cove" data-tauri-drag-region />
      <div class="brand-text" data-tauri-drag-region>
        <div class="brand-name" data-tauri-drag-region>Cove Toolkit</div>
        <div class="brand-version" data-tauri-drag-region>v{appVersion}</div>
      </div>
    </div>

    <nav class="tabs" aria-label="Main views" data-tauri-drag-region>
      {#each tabs as tab}
        <button
          class="tab"
          class:active={activeTab === tab.id}
          onclick={() => (activeTab = tab.id)}
          title={`${tab.label} (Alt+${tab.key})`}
        >
          <span class="tab-icon" aria-hidden="true">
            {#if tab.icon === "search"}
              <svg viewBox="0 0 16 16" width="14" height="14"
                ><path
                  d="M11.5 7.5a4 4 0 1 1-8 0 4 4 0 0 1 8 0Zm-1.1 3.4a5.5 5.5 0 1 1 1.06-1.06l3.34 3.34a.75.75 0 0 1-1.06 1.06l-3.34-3.34Z"
                  fill="currentColor"
                /></svg
              >
            {:else if tab.icon === "disk"}
              <svg viewBox="0 0 16 16" width="14" height="14"
                ><path
                  d="M8 1.5a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13Zm0 1.5a5 5 0 0 1 4.9 4H8.75A.75.75 0 0 0 8 7.75V12a5 5 0 1 1 0-9Z"
                  fill="currentColor"
                /></svg
              >
            {:else}
              <svg viewBox="0 0 16 16" width="14" height="14"
                ><path
                  d="M3.5 2A1.5 1.5 0 0 0 2 3.5v9A1.5 1.5 0 0 0 3.5 14h9a1.5 1.5 0 0 0 1.5-1.5V8.207a.75.75 0 1 0-1.5 0V12.5h-9v-9h4.293a.75.75 0 1 0 0-1.5H3.5Zm9.78 1.97a.75.75 0 0 1 1.06 1.06l-7.5 7.5a.75.75 0 0 1-.32.19l-2.5.75a.75.75 0 0 1-.93-.93l.75-2.5a.75.75 0 0 1 .19-.32l7.5-7.5Z"
                  fill="currentColor"
                /></svg
              >
            {/if}
          </span>
          {tab.label}
          <span class="kbd">⌥{tab.key}</span>
        </button>
      {/each}
    </nav>

    <div class="topbar-actions" data-tauri-drag-region>
      <button
        class="action-btn"
        title="Settings (Ctrl+,)"
        onclick={() => (showSettings = true)}
      >
        <svg viewBox="0 0 16 16" width="14" height="14"
          ><path
            d="M9.4 1c.4 0 .74.27.84.65l.27 1.06c.5.18.97.46 1.36.8l1.06-.27a.86.86 0 0 1 .96.4l1.4 2.42a.86.86 0 0 1-.18 1.06l-.83.78c.05.27.07.55.07.83s-.02.56-.07.83l.83.78c.31.29.39.74.18 1.06l-1.4 2.42a.86.86 0 0 1-.96.4l-1.06-.27c-.4.34-.86.62-1.36.8l-.27 1.06a.86.86 0 0 1-.84.65H6.6a.86.86 0 0 1-.84-.65l-.27-1.06c-.5-.18-.97-.46-1.36-.8l-1.06.27a.86.86 0 0 1-.96-.4l-1.4-2.42a.86.86 0 0 1 .18-1.06l.83-.78A6 6 0 0 1 1.65 8c0-.28.02-.56.07-.83l-.83-.78a.86.86 0 0 1-.18-1.06l1.4-2.42a.86.86 0 0 1 .96-.4l1.06.27c.4-.34.86-.62 1.36-.8l.27-1.06A.86.86 0 0 1 6.6 1h2.8ZM8 5.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5Z"
            fill="currentColor"
          /></svg
        >
        <span>Settings</span>
      </button>
    </div>

    <div class="window-controls" data-tauri-drag-region>
      <button
        class="wc-btn"
        title="Minimize"
        aria-label="Minimize"
        onclick={winMinimize}
      >
        <svg viewBox="0 0 12 12" width="12" height="12">
          <path d="M2 6h8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
        </svg>
      </button>
      <button
        class="wc-btn"
        title={isMaximized ? "Restore" : "Maximize"}
        aria-label={isMaximized ? "Restore" : "Maximize"}
        onclick={winToggleMaximize}
      >
        {#if isMaximized}
          <svg viewBox="0 0 12 12" width="12" height="12">
            <path
              d="M3.5 2.5h6v6h-6z M2 4h1.5v6h6V11.5h-7.5z"
              stroke="currentColor"
              stroke-width="1.1"
              fill="none"
              stroke-linejoin="round"
            />
          </svg>
        {:else}
          <svg viewBox="0 0 12 12" width="12" height="12">
            <rect
              x="2.5"
              y="2.5"
              width="7"
              height="7"
              fill="none"
              stroke="currentColor"
              stroke-width="1.2"
              rx="0.5"
            />
          </svg>
        {/if}
      </button>
      <button
        class="wc-btn close"
        title="Close"
        aria-label="Close"
        onclick={winClose}
      >
        <svg viewBox="0 0 12 12" width="12" height="12">
          <path
            d="M3 3l6 6 M9 3l-6 6"
            stroke="currentColor"
            stroke-width="1.4"
            stroke-linecap="round"
          />
        </svg>
      </button>
    </div>
  </header>

  <section class="view">
    {#if activeTab === "search"}
      <Search
        {sendToRename}
        {settings}
        focusSignal={globalShortcut === "focus-search"}
        refreshSignal={globalShortcut === "refresh"}
        onCacheChange={() => refreshSettings()}
      />
    {:else if activeTab === "disk"}
      <DiskUsage
        {sendToRename}
        defaultRoot={settings?.default_root ?? homeRoot}
        refreshSignal={globalShortcut === "refresh"}
      />
    {:else}
      <Rename files={renameFiles} />
    {/if}
  </section>
</main>

{#if showSettings}
  <Settings
    onClose={async () => {
      showSettings = false;
      await refreshSettings();
    }}
  />
{/if}

{#if trayHintVisible}
  <div class="tray-toast" role="status" aria-live="polite">
    <span>Cove Toolkit is still running in the tray. Right-click the tray icon to quit, or toggle this off in Settings.</span>
    <button class="tt-x" aria-label="Dismiss" onclick={() => (trayHintVisible = false)}>×</button>
  </div>
{/if}

<!--
  Frameless-window resize handles. Thin invisible zones around the window
  edges and corners that call Tauri's startResizeDragging on mousedown.
  Hidden when the window is maximized so a max'd window can't be dragged
  to resize accidentally. Kept narrow so they don't block clicks on the
  toolbar buttons, window controls, scrollbars, or table rows.
-->
{#if !isMaximized}
  <div class="resize-edges" aria-hidden="true">
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="rz rz-n" onmousedown={(e) => onResize(e, "North")}></div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="rz rz-s" onmousedown={(e) => onResize(e, "South")}></div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="rz rz-w" onmousedown={(e) => onResize(e, "West")}></div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="rz rz-e" onmousedown={(e) => onResize(e, "East")}></div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="rz rz-nw" onmousedown={(e) => onResize(e, "NorthWest")}></div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="rz rz-ne" onmousedown={(e) => onResize(e, "NorthEast")}></div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="rz rz-sw" onmousedown={(e) => onResize(e, "SouthWest")}></div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="rz rz-se" onmousedown={(e) => onResize(e, "SouthEast")}></div>
  </div>
{/if}

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg);
  }

  .topbar {
    display: flex;
    align-items: center;
    height: 42px;
    padding: 0 8px 0 14px;
    background: linear-gradient(180deg, #161b22 0%, #11161e 100%);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
    gap: 16px;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    padding-right: 14px;
    border-right: 1px solid var(--border-soft);
    height: 100%;
  }
  .brand-icon {
    width: 24px;
    height: 24px;
    object-fit: contain;
    image-rendering: -webkit-optimize-contrast;
    flex-shrink: 0;
  }

  .brand-text {
    display: flex;
    flex-direction: column;
    line-height: 1;
  }
  .brand-name {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-strong);
    letter-spacing: 0.2px;
  }
  .brand-version {
    font-size: 10px;
    color: var(--text-faint);
    font-family: var(--mono);
    margin-top: 2px;
  }

  .tabs {
    display: flex;
    align-items: stretch;
    height: 100%;
    gap: 2px;
    flex: 1;
  }

  .tab {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0 12px;
    font-size: 12.5px;
    font-weight: 500;
    color: var(--text-muted);
    border: none;
    border-bottom: 2px solid transparent;
    background: transparent;
    transition:
      color 0.12s ease,
      border-color 0.12s ease,
      background 0.12s ease;
  }
  .tab:hover {
    color: var(--text);
    background: rgba(255, 255, 255, 0.03);
  }
  .tab.active {
    color: var(--text-strong);
    border-bottom-color: var(--accent);
    background: rgba(47, 129, 247, 0.08);
  }
  .tab-icon {
    display: inline-flex;
    align-items: center;
    color: inherit;
    opacity: 0.85;
  }
  .tab .kbd {
    font-size: 9px;
    padding: 0 4px;
    margin-left: 2px;
    opacity: 0.65;
  }

  .topbar-actions {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .window-controls {
    display: flex;
    align-items: center;
    gap: 2px;
    margin-left: 6px;
    padding-left: 8px;
    border-left: 1px solid var(--border-soft);
    height: 100%;
  }
  .wc-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 26px;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
    transition: background 0.12s ease, color 0.12s ease;
  }
  .wc-btn:hover {
    color: var(--text-strong);
    background: rgba(255, 255, 255, 0.06);
  }
  .wc-btn.close:hover {
    color: #fff;
    background: #e81123;
  }

  .action-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    border-radius: 5px;
    color: var(--text-muted);
    font-size: 12px;
    font-weight: 500;
    background: transparent;
    border: 1px solid transparent;
  }
  .action-btn:hover {
    color: var(--text);
    background: var(--bg-hover);
    border-color: var(--border);
  }

  .view {
    flex: 1;
    min-height: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  /* Frameless-window resize handles (positioned at window edges/corners) */
  .resize-edges {
    position: fixed;
    inset: 0;
    pointer-events: none;
    z-index: 100;
  }
  .rz {
    position: fixed;
    pointer-events: auto;
    background: transparent;
  }
  /* Corners are larger than edges and painted last (later in the DOM) so they
     win hit-testing where they meet the edges. */
  .rz-n {
    top: 0;
    left: 16px;
    right: 16px;
    height: 6px;
    cursor: ns-resize;
  }
  .rz-s {
    bottom: 0;
    left: 16px;
    right: 16px;
    height: 6px;
    cursor: ns-resize;
  }
  .rz-w {
    top: 16px;
    bottom: 16px;
    left: 0;
    width: 6px;
    cursor: ew-resize;
  }
  .rz-e {
    top: 16px;
    bottom: 16px;
    right: 0;
    width: 6px;
    cursor: ew-resize;
  }
  .rz-nw {
    top: 0;
    left: 0;
    width: 16px;
    height: 16px;
    cursor: nwse-resize;
    z-index: 101;
  }
  .rz-ne {
    top: 0;
    right: 0;
    width: 16px;
    height: 16px;
    cursor: nesw-resize;
    z-index: 101;
  }
  .rz-sw {
    bottom: 0;
    left: 0;
    width: 16px;
    height: 16px;
    cursor: nesw-resize;
    z-index: 101;
  }
  .rz-se {
    bottom: 0;
    right: 0;
    width: 16px;
    height: 16px;
    cursor: nwse-resize;
    z-index: 101;
  }

  .tray-toast {
    position: fixed;
    right: 16px;
    bottom: 16px;
    z-index: 300;
    max-width: 360px;
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 10px 12px;
    background: var(--bg-2);
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
    color: var(--text);
    font-size: 12px;
    line-height: 1.4;
  }
  .tray-toast .tt-x {
    background: transparent;
    border: 0;
    color: var(--text-muted);
    font-size: 16px;
    line-height: 1;
    padding: 0 4px;
    cursor: pointer;
    border-radius: 3px;
  }
  .tray-toast .tt-x:hover {
    color: var(--text);
    background: var(--bg-hover);
  }
</style>
