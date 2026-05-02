<script lang="ts">
  import { onMount } from "svelte";
  import ContextMenu from "../lib/components/ContextMenu.svelte";
  import Treemap from "../lib/components/Treemap.svelte";
  import {
    openPath,
    revealInFolder,
    formatSize,
    formatDate,
    formatElapsed,
    type DiskUsageEntry,
  } from "../lib/ipc";
  import {
    getDiskState,
    startDiskScan,
    cancelDiskScan,
    drillDownDisk,
    goUpDisk,
    toggleExpandDisk,
    setDiskSelected,
    diskBreadcrumbs,
    navigateDiskTo,
    etaText,
    syncDiskScanState,
  } from "../lib/scanStore.svelte";

  interface Props {
    sendToRename: (files: Array<{ name: string; path: string }>) => void;
    defaultRoot?: string;
    refreshSignal?: boolean;
  }

  let {
    sendToRename,
    defaultRoot = "/home",
    refreshSignal = false,
  }: Props = $props();

  // Persistent global state — survives tab switches.
  const ds = getDiskState();

  // Local-only UI state (path input, sort, right-panel mode, context menu)
  let scanRoot = $state(ds.scanRoot || defaultRoot);
  let sortField = $state<
    "size" | "name" | "items" | "files" | "folders" | "mtime" | "pct"
  >("size");
  let sortAsc = $state(false);

  let rightPanel = $state<"extensions" | "largest">("extensions");
  let ctxMenu = $state<
    { x: number; y: number; path: string; isDir: boolean } | null
  >(null);

  // Multi-select state for the table. Mirrors the Search view so users get
  // the same Ctrl/Cmd-click + Shift-click + checkbox UX in both places, and
  // can hand a multi-row pick straight to Rename. ds.selectedPath is kept
  // separately as the "focused" row for the breadcrumb/treemap.
  let selectedPaths = $state(new Set<string>());
  let lastClickedIndex = $state(-1);

  function clearSelection() {
    selectedPaths = new Set();
    lastClickedIndex = -1;
  }

  function toggleRowSelect(idx: number, path: string, multi: boolean, range: boolean) {
    const next = new Set(selectedPaths);
    if (range && lastClickedIndex >= 0) {
      const a = Math.min(lastClickedIndex, idx);
      const b = Math.max(lastClickedIndex, idx);
      for (let i = a; i <= b && i < flatRows.length; i++) {
        next.add(flatRows[i].entry.path);
      }
    } else if (multi) {
      if (next.has(path)) next.delete(path);
      else next.add(path);
    } else {
      next.clear();
      next.add(path);
    }
    // Reset the Shift-click anchor when Ctrl/Cmd-toggling left nothing
    // selected — otherwise a stale lastClickedIndex would silently extend
    // a future Shift-click into a row the user thought they'd cleared.
    if (next.size === 0) {
      lastClickedIndex = -1;
    } else if (idx >= 0) {
      lastClickedIndex = idx;
    }
    selectedPaths = next;
  }

  function selectAllVisible(sel: boolean) {
    if (sel) selectedPaths = new Set(flatRows.map((r) => r.entry.path));
    else clearSelection();
  }

  // Header checkbox bookkeeping. allSelected reflects the visible flat rows.
  // `$derived.by` is required (not `$derived(expr)`) because `flatRows` is
  // declared later in this script and must be read lazily.
  let visibleSelectedCount = $derived.by(() =>
    flatRows.reduce((n, r) => (selectedPaths.has(r.entry.path) ? n + 1 : n), 0),
  );
  let allVisibleSelected = $derived.by(
    () => flatRows.length > 0 && visibleSelectedCount === flatRows.length,
  );
  let headerCb: HTMLInputElement | undefined = $state();
  $effect(() => {
    if (!headerCb) return;
    headerCb.indeterminate =
      visibleSelectedCount > 0 && visibleSelectedCount < flatRows.length;
  });

  function selectedItemsInOrder(): Array<{ name: string; path: string }> {
    return flatRows
      .filter((r) => selectedPaths.has(r.entry.path))
      .map((r) => ({ name: r.entry.name, path: r.entry.path }));
  }

  function handleSendSelectedToRename() {
    const list = selectedItemsInOrder();
    if (list.length === 0) return;
    sendToRename(list);
    clearSelection();
  }

  // Whenever the displayed dataset shifts (different scan root, drilled
  // into a folder, full reset) the previous selection refers to rows that
  // may no longer exist. Wipe both the set and the Shift anchor so counts
  // and Send-to-Rename always describe what's on screen.
  $effect(() => {
    ds.rootInfo;
    clearSelection();
  });

  // Keep input in sync if a scan is running with a different root
  $effect(() => {
    if (ds.scanRoot && !scanRoot) scanRoot = ds.scanRoot;
  });

  $effect(() => {
    if (refreshSignal) handleStartScan();
  });

  function sortChildren(arr: DiskUsageEntry[]): DiskUsageEntry[] {
    const out = [...arr];
    out.sort((a, b) => {
      let cmp = 0;
      switch (sortField) {
        case "name":
          cmp = a.name.localeCompare(b.name);
          break;
        case "size":
          cmp = a.size - b.size;
          break;
        case "items":
          cmp = a.item_count - b.item_count;
          break;
        case "files":
          cmp = a.file_count - b.file_count;
          break;
        case "folders":
          cmp = a.dir_count - b.dir_count;
          break;
        case "mtime":
          cmp = a.mtime - b.mtime;
          break;
        case "pct":
          cmp = a.percentage - b.percentage;
          break;
      }
      return sortAsc ? cmp : -cmp;
    });
    return out;
  }

  function onSort(field: typeof sortField) {
    if (sortField === field) sortAsc = !sortAsc;
    else {
      sortField = field;
      sortAsc = field === "name";
    }
  }

  function indicator(field: typeof sortField): string {
    if (field !== sortField) return "";
    return sortAsc ? " ▲" : " ▼";
  }

  async function handleStartScan() {
    if (!scanRoot.trim()) return;
    await startDiskScan(scanRoot);
  }

  async function handleStopScan() {
    await cancelDiskScan();
  }

  async function handleDrill(entry: DiskUsageEntry) {
    if (entry.is_dir) await drillDownDisk(entry);
    else openPath(entry.path).catch(() => {});
  }

  function onContext(e: MouseEvent, entry: DiskUsageEntry) {
    e.preventDefault();
    setDiskSelected(entry.path);
    // Right-clicking an unselected row replaces selection with just that
    // row so single-target context menu actions don't silently use a
    // hidden previous selection. Multi-row right-click (clicked row was
    // already in selection) is preserved so "Send N selected" shows up.
    if (!selectedPaths.has(entry.path)) {
      const idx = flatRows.findIndex((r) => r.entry.path === entry.path);
      toggleRowSelect(idx, entry.path, false, false);
    }
    ctxMenu = {
      x: e.clientX,
      y: e.clientY,
      path: entry.path,
      isDir: entry.is_dir,
    };
  }

  function findEntry(path: string): DiskUsageEntry | null {
    for (const list of Object.values(ds.childCache)) {
      const m = list.find((e) => e.path === path);
      if (m) return m;
    }
    const lf = ds.rootInfo?.largest_files?.find((e) => e.path === path);
    return lf ?? null;
  }

  function ctxItems() {
    if (!ctxMenu) return [];
    const isDir = ctxMenu.isDir;
    const path = ctxMenu.path;
    // If the right-clicked row is part of an existing multi-row selection,
    // the primary "send" action operates on the whole selection. Mirrors the
    // Search view so the gesture is identical across both tables.
    const multi = selectedPaths.size > 1 && selectedPaths.has(path);
    return [
      ...(isDir
        ? [
            {
              label: "Drill in",
              action: () => {
                const entry = findEntry(path);
                if (entry) drillDownDisk(entry);
              },
            },
          ]
        : []),
      {
        label: isDir ? "Open folder" : "Open file",
        action: () => openPath(path).catch(() => {}),
      },
      {
        label: "Open containing folder",
        action: () => revealInFolder(path).catch(() => {}),
      },
      { separator: true } as any,
      {
        label: "Copy path",
        action: () => navigator.clipboard?.writeText(path).catch(() => {}),
      },
      { separator: true } as any,
      multi
        ? {
            label: `Send ${selectedPaths.size} selected to Rename`,
            action: () => handleSendSelectedToRename(),
          }
        : {
            label: "Send to Rename",
            action: () => {
              const entry = findEntry(path);
              if (entry) sendToRename([{ name: entry.name, path: entry.path }]);
            },
          },
    ];
  }

  let breadcrumbs = $derived.by(() => diskBreadcrumbs());

  async function jumpBreadcrumb(i: number) {
    if (i >= breadcrumbs.length - 1) return;
    const target = breadcrumbs[i].path;
    ds.pathHistory = ds.pathHistory.slice(0, i);
    await navigateDiskTo(target);
  }

  type RowEntry = { entry: DiskUsageEntry; depth: number };
  let flatRows = $derived.by((): RowEntry[] => {
    if (!ds.rootInfo) return [];
    const out: RowEntry[] = [];
    const walk = (parentPath: string, depth: number) => {
      const list = ds.childCache[parentPath];
      if (!list) return;
      const sorted = sortChildren(list);
      for (const e of sorted) {
        out.push({ entry: e, depth });
        if (e.is_dir && ds.expanded.has(e.path)) walk(e.path, depth + 1);
      }
    };
    walk(ds.rootInfo.path, 0);
    return out;
  });

  // Windowed rendering for the table: keeps DOM bounded regardless of how
  // many rows flatRows contains. Without this, expanding a folder with
  // thousands of children freezes the UI on layout/style work.
  const ROW_HEIGHT = 27; // td height (26) + 1px bottom border
  const ROW_BUFFER = 8;
  let tableEl: HTMLDivElement | undefined = $state();
  let tableScrollTop = $state(0);
  let tableViewportH = $state(600);

  function onTableScroll() {
    if (!tableEl) return;
    tableScrollTop = tableEl.scrollTop;
  }

  let visibleStart = $derived(
    Math.max(0, Math.floor(tableScrollTop / ROW_HEIGHT) - ROW_BUFFER),
  );
  let visibleEnd = $derived(
    Math.min(
      flatRows.length,
      Math.ceil((tableScrollTop + tableViewportH) / ROW_HEIGHT) + ROW_BUFFER,
    ),
  );
  let visibleRows = $derived(flatRows.slice(visibleStart, visibleEnd));
  let topPad = $derived(visibleStart * ROW_HEIGHT);
  let bottomPad = $derived(
    Math.max(0, (flatRows.length - visibleEnd) * ROW_HEIGHT),
  );

  $effect(() => {
    if (!tableEl) return;
    const ro = new ResizeObserver((entries) => {
      tableViewportH = entries[0].contentRect.height;
    });
    ro.observe(tableEl);
    tableViewportH = tableEl.clientHeight || 600;
    return () => ro.disconnect();
  });

  let filesPerSec = $derived.by(() => {
    if (!ds.progress || ds.progress.elapsed_ms <= 0) return 0;
    return Math.round(
      ds.progress.files_found / (ds.progress.elapsed_ms / 1000),
    );
  });

  let etaLabel = $derived.by(() => {
    if (!ds.progress) return "calculating…";
    return etaText(
      ds.progress.files_found,
      ds.prevTotalFiles,
      filesPerSec,
      ds.progress.elapsed_ms,
    );
  });

  // Treemap data: prefer top-level children of the currently displayed folder
  // mixed with its largest files for a richer block panel.
  let treemapItems = $derived.by((): DiskUsageEntry[] => {
    if (!ds.rootInfo) return [];
    const fromChildren = ds.rootInfo.children.slice(0, 60);
    if (fromChildren.length > 0) return fromChildren;
    return ds.rootInfo.largest_files ?? [];
  });

  function onTreemapSelect(entry: DiskUsageEntry) {
    setDiskSelected(entry.path);
  }

  async function onTreemapDrill(entry: DiskUsageEntry) {
    if (entry.is_dir) await drillDownDisk(entry);
    else openPath(entry.path).catch(() => {});
  }

  onMount(() => {
    // The global scanStore owns event listeners. Pull authoritative state
    // in case events were missed while we weren't on this tab — handles
    // the cross-tab persistence acceptance test.
    syncDiskScanState().catch(() => {});
  });
</script>

<div class="du-view">
  <!-- Toolbar -->
  <div class="toolbar">
    <div class="path-input">
      <span class="ico"
        ><svg viewBox="0 0 16 16" width="14" height="14"
          ><path
            d="M1.5 3.75A1.75 1.75 0 0 1 3.25 2h3a1.75 1.75 0 0 1 1.4.7l.6.8h4.5c.97 0 1.75.78 1.75 1.75v6.5c0 .97-.78 1.75-1.75 1.75H3.25a1.75 1.75 0 0 1-1.75-1.75V3.75Z"
            fill="currentColor"
          /></svg
        ></span
      >
      <input
        type="text"
        bind:value={scanRoot}
        placeholder="Path to scan (e.g. /home/sin/Pictures)"
        disabled={ds.status === "scanning"}
        onkeydown={(e) => e.key === "Enter" && handleStartScan()}
      />
    </div>
    {#if ds.status === "scanning"}
      <button class="btn danger sm" onclick={handleStopScan}>Cancel</button>
      <span
        class="hint"
        title="Scanning runs in the background — switching tabs will not stop it."
      >scan keeps running across tabs</span
      >
    {:else}
      <button
        class="btn primary sm"
        onclick={handleStartScan}
        title={ds.rootInfo ? "Re-scan this path." : "Walk the directory and compute sizes."}
        >{ds.rootInfo ? "Rescan" : "Scan"}</button
      >
    {/if}
    {#if ds.rootInfo}
      <button
        class="btn ghost sm"
        disabled={ds.pathHistory.length === 0}
        onclick={goUpDisk}>Up</button
      >
    {/if}
  </div>

  <!-- Summary / progress bar -->
  {#if ds.status === "scanning"}
    <div class="banner">
      <div class="b-row">
        <span class="badge indexing">Scanning</span>
        {#if ds.progress}
          <span class="b-stat">{ds.progress.files_found.toLocaleString()} files</span>
          <span class="dot">•</span>
          <span class="b-stat">{ds.progress.dirs_found.toLocaleString()} folders</span>
          <span class="dot">•</span>
          <span class="b-stat">{formatSize(ds.progress.bytes_found ?? 0)}</span>
          <span class="dot">•</span>
          <span class="b-stat">{formatElapsed(ds.progress.elapsed_ms)}</span>
          <span class="dot">•</span>
          <span class="b-stat">{filesPerSec.toLocaleString()} files/s</span>
          <span class="dot">•</span>
          <span class="b-stat" title="Estimated time remaining">ETA: {etaLabel}</span>
          {#if (ds.progress.errors ?? 0) > 0}
            <span class="dot">•</span>
            <span class="b-stat err" title="Entries skipped due to errors / permission denied"
              >{ds.progress.errors.toLocaleString()} skipped</span
            >
          {/if}
        {:else}
          <span class="b-stat">Starting…</span>
        {/if}
      </div>
      {#if ds.progress?.current_path}
        <div class="b-path" title={ds.progress.current_path}>{ds.progress.current_path}</div>
      {/if}
      <div class="b-bar"><div class="b-fill"></div></div>
    </div>
  {:else if ds.rootInfo}
    <div class="summary">
      <div class="sum-block">
        <div class="sum-label">Total size</div>
        <div class="sum-value">{formatSize(ds.rootInfo.total_size)}</div>
      </div>
      <div class="sum-block">
        <div class="sum-label">Files</div>
        <div class="sum-value">{ds.rootInfo.total_file_count.toLocaleString()}</div>
      </div>
      {#if ds.complete}
        <div class="sum-block">
          <div class="sum-label">Folders</div>
          <div class="sum-value">{ds.complete.total_dirs.toLocaleString()}</div>
        </div>
        <div class="sum-block">
          <div class="sum-label">Scan time</div>
          <div class="sum-value">{formatElapsed(ds.complete.elapsed_ms)}</div>
        </div>
      {/if}
      <div class="sum-block grow">
        <div class="sum-label">Path</div>
        <div class="sum-value mono path-line" title={ds.rootInfo.path}
          >{ds.rootInfo.path}</div
        >
      </div>
    </div>
  {/if}

  {#if ds.error}
    <div class="error">{ds.error}</div>
  {/if}

  <!-- Breadcrumbs -->
  {#if ds.rootInfo}
    <div class="breadcrumbs">
      {#each breadcrumbs as crumb, i (crumb.path)}
        {#if i > 0}<span class="bc-sep">/</span>{/if}
        {#if i < breadcrumbs.length - 1}
          <button class="bc" onclick={() => jumpBreadcrumb(i)}>{crumb.label}</button>
        {:else}
          <span class="bc current">{crumb.label}</span>
        {/if}
      {/each}
    </div>
  {/if}

  {#if ds.rootInfo && selectedPaths.size > 0}
    <div class="selection-bar">
      <span class="sel-count">{selectedPaths.size} selected</span>
      <button class="btn primary sm" onclick={handleSendSelectedToRename}
        >Send to Rename</button
      >
      <button class="btn ghost sm" onclick={clearSelection}>Clear selection</button>
    </div>
  {/if}

  <!-- Main area: table + side panel + treemap row -->
  {#if ds.rootInfo}
    <div class="main-area">
      <div class="upper">
        <div
          class="table-wrap"
          bind:this={tableEl}
          onscroll={onTableScroll}
        >
          <table class="du">
            <colgroup>
              <col style="width: 32px" />
              <col />
              <col style="width: 9%" />
              <col style="width: 12%" />
              <col style="width: 9%" />
              <col style="width: 9%" />
              <col style="width: 9%" />
              <col style="width: 14%" />
            </colgroup>
            <thead>
              <tr>
                <th
                  class="check"
                  title={allVisibleSelected
                    ? "Deselect all"
                    : visibleSelectedCount > 0
                      ? `${visibleSelectedCount} of ${flatRows.length} selected`
                      : "Select all"}
                  onclick={(e) => e.stopPropagation()}
                >
                  <input
                    bind:this={headerCb}
                    type="checkbox"
                    class="cb"
                    checked={allVisibleSelected}
                    onclick={(e) => e.stopPropagation()}
                    onchange={() => selectAllVisible(!allVisibleSelected)}
                  />
                </th>
                <th class="name-h" onclick={() => onSort("name")}
                  >Folder / File{indicator("name")}</th
                >
                <th class="num" onclick={() => onSort("pct")}
                  >% of Parent{indicator("pct")}</th
                >
                <th class="num" onclick={() => onSort("size")}>Size{indicator("size")}</th>
                <th class="num" onclick={() => onSort("items")}>Items{indicator("items")}</th>
                <th class="num" onclick={() => onSort("files")}>Files{indicator("files")}</th>
                <th class="num" onclick={() => onSort("folders")}
                  >Folders{indicator("folders")}</th
                >
                <th class="num" onclick={() => onSort("mtime")}
                  >Modified{indicator("mtime")}</th
                >
              </tr>
            </thead>
            <tbody>
              {#if topPad > 0}
                <tr class="pad-row" style="height: {topPad}px"
                  ><td colspan="8"></td></tr
                >
              {/if}
              {#each visibleRows as row, i (row.entry.path + ":" + row.depth)}
                {@const e = row.entry}
                {@const isExpanded = ds.expanded.has(e.path)}
                {@const idx = visibleStart + i}
                {@const isChecked = selectedPaths.has(e.path)}
                <tr
                  class="row"
                  class:dir={e.is_dir}
                  class:selected={ds.selectedPath === e.path || isChecked}
                  onclick={(ev) => {
                    setDiskSelected(e.path);
                    toggleRowSelect(idx, e.path, ev.ctrlKey || ev.metaKey, ev.shiftKey);
                  }}
                  ondblclick={() => handleDrill(e)}
                  oncontextmenu={(ev) => onContext(ev, e)}
                >
                  <td
                    class="check"
                    onclick={(ev) => ev.stopPropagation()}
                    ondblclick={(ev) => ev.stopPropagation()}
                  >
                    <input
                      type="checkbox"
                      class="cb"
                      checked={isChecked}
                      onclick={(ev) => ev.stopPropagation()}
                      onchange={() => toggleRowSelect(idx, e.path, true, false)}
                    />
                  </td>
                  <td class="name-cell">
                    <span class="indent" style="width: {row.depth * 14}px"></span>
                    {#if e.is_dir}
                      <button
                        class="caret"
                        onclick={(ev) => {
                          ev.stopPropagation();
                          toggleExpandDisk(e);
                        }}
                        aria-label={isExpanded ? "Collapse" : "Expand"}
                      >
                        <svg viewBox="0 0 10 10" width="9" height="9"
                          ><path
                            d={isExpanded ? "M1 3l4 4 4-4" : "M3 1l4 4-4 4"}
                            stroke="currentColor"
                            stroke-width="1.5"
                            fill="none"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                          /></svg
                        >
                      </button>
                    {:else}
                      <span class="caret-spacer"></span>
                    {/if}
                    <span class="ico" aria-hidden="true">
                      {#if e.is_dir}
                        <svg viewBox="0 0 16 16" width="12" height="12"
                          ><path
                            d="M1.5 3.75A1.75 1.75 0 0 1 3.25 2h3a1.75 1.75 0 0 1 1.4.7l.6.8h4.5c.97 0 1.75.78 1.75 1.75v6.5c0 .97-.78 1.75-1.75 1.75H3.25a1.75 1.75 0 0 1-1.75-1.75V3.75Z"
                            fill="#e2b94a"
                          /></svg
                        >
                      {:else}
                        <svg viewBox="0 0 16 16" width="12" height="12"
                          ><path
                            d="M3.5 1.5h6.3a1.5 1.5 0 0 1 1.06.44l2.7 2.7c.28.28.44.66.44 1.06V13.5A1.5 1.5 0 0 1 12.5 15h-9A1.5 1.5 0 0 1 2 13.5v-10A1.5 1.5 0 0 1 3.5 2v-.5Z"
                            fill="#5fb3ff"
                            opacity="0.85"
                          /></svg
                        >
                      {/if}
                    </span>
                    <span class="name-text" title={e.path}>{e.name}</span>
                  </td>
                  <td class="num pct">
                    <div class="pct-bar">
                      <div
                        class="pct-fill"
                        style="width: {Math.min(100, Math.max(0, e.percentage)).toFixed(1)}%"
                      ></div>
                    </div>
                    <span class="pct-text">{e.percentage.toFixed(1)}%</span>
                  </td>
                  <td class="num">{formatSize(e.size)}</td>
                  <td class="num muted">{e.is_dir ? e.item_count.toLocaleString() : "—"}</td>
                  <td class="num muted">{e.is_dir ? e.file_count.toLocaleString() : "—"}</td>
                  <td class="num muted">{e.is_dir ? e.dir_count.toLocaleString() : "—"}</td>
                  <td class="num muted">{formatDate(e.mtime)}</td>
                </tr>
              {/each}
              {#if bottomPad > 0}
                <tr class="pad-row" style="height: {bottomPad}px"
                  ><td colspan="8"></td></tr
                >
              {/if}
            </tbody>
          </table>
          {#if flatRows.length === 0}
            <div class="empty-folder">This folder is empty.</div>
          {/if}
        </div>

        <!-- Side panel -->
        <aside class="side">
          <div class="side-tabs">
            <button
              class:active={rightPanel === "extensions"}
              onclick={() => (rightPanel = "extensions")}>By File Type</button
            >
            <button
              class:active={rightPanel === "largest"}
              onclick={() => (rightPanel = "largest")}>Largest Files</button
            >
          </div>

          {#if rightPanel === "extensions"}
            <div class="side-body">
              {#if ds.rootInfo.extensions.length === 0}
                <div class="side-empty">No file types in this folder.</div>
              {:else}
                <div class="ext-list">
                  {#each ds.rootInfo.extensions as ext}
                    <div class="ext-row" title={`${ext.count.toLocaleString()} files`}>
                      <div class="ext-bar-wrap">
                        <div
                          class="ext-bar"
                          style={`width: ${Math.min(100, ext.percentage).toFixed(1)}%`}
                        ></div>
                      </div>
                      <div class="ext-name">{ext.extension}</div>
                      <div class="ext-size">{formatSize(ext.size)}</div>
                      <div class="ext-pct">{ext.percentage.toFixed(1)}%</div>
                    </div>
                  {/each}
                </div>
              {/if}
            </div>
          {:else}
            <div class="side-body">
              {#if ds.rootInfo.largest_files.length === 0}
                <div class="side-empty">No files found here.</div>
              {:else}
                <div class="lg-list">
                  {#each ds.rootInfo.largest_files as f}
                    <button
                      class="lg-row"
                      title={f.path}
                      onclick={() => setDiskSelected(f.path)}
                      ondblclick={() => openPath(f.path).catch(() => {})}
                      oncontextmenu={(ev) => {
                        ev.preventDefault();
                        ctxMenu = { x: ev.clientX, y: ev.clientY, path: f.path, isDir: false };
                      }}
                    >
                      <div class="lg-name">{f.name}</div>
                      <div class="lg-bar">
                        <div
                          class="lg-fill"
                          style={`width: ${Math.min(100, f.percentage).toFixed(1)}%`}
                        ></div>
                      </div>
                      <div class="lg-size">{formatSize(f.size)}</div>
                    </button>
                  {/each}
                </div>
              {/if}
            </div>
          {/if}
        </aside>
      </div>

      <!-- Treemap (WizTree-style colored blocks) -->
      <div class="treemap-section">
        <div class="treemap-header">
          <span class="tm-title">Treemap</span>
          <span class="tm-sub">{treemapItems.length} blocks · {ds.rootInfo.name}</span>
          <span class="tm-hint">double-click a block to drill in</span>
        </div>
        <Treemap
          items={treemapItems}
          selectedPath={ds.selectedPath}
          onSelect={onTreemapSelect}
          onDrill={onTreemapDrill}
          height={220}
        />
      </div>
    </div>
  {:else if ds.status === "scanning"}
    <div class="empty">
      <div class="spinner"></div>
      <div class="empty-title">
        {ds.phase === "starting"
          ? "Starting scan…"
          : ds.phase === "finalizing"
            ? "Finalizing…"
            : "Scanning…"}
      </div>
      <div class="empty-hint">
        {ds.message || "Walking directories and computing sizes."}
        {#if ds.engine}
          <div class="engine-hint">engine: {ds.engine}</div>
        {/if}
      </div>
    </div>
  {:else if ds.complete && !ds.rootInfo && !ds.error}
    <!-- Backend reports completion but result tree hasn't hydrated yet.
         Distinct UX from "Scanning…" — Cancel is gone, spinner says
         "Loading results…" instead. -->
    <div class="empty">
      <div class="spinner"></div>
      <div class="empty-title">Loading results…</div>
      <div class="empty-hint">
        Reading scan results for display.
        {#if ds.engine}
          <div class="engine-hint">engine: {ds.engine}</div>
        {/if}
      </div>
    </div>
  {:else if ds.status === "error"}
    <div class="empty">
      <div class="empty-title">Scan failed</div>
      <div class="empty-hint">{ds.error || "Try a different path."}</div>
    </div>
  {:else}
    <div class="empty">
      <div class="empty-title">No scan yet</div>
      <div class="empty-hint">
        Pick a directory and click Scan. Try
        <span class="kbd">{defaultRoot}</span> to start.
      </div>
    </div>
  {/if}

  <!-- Status bar -->
  <div class="status-bar">
    <span
      class="badge"
      class:idle={ds.status === "idle"}
      class:indexing={ds.status === "scanning"}
      class:ready={ds.status === "ready"}
      class:error={ds.status === "error"}
      >{ds.status === "ready"
        ? "Ready"
        : ds.status === "scanning"
          ? "Scanning"
          : ds.status === "error"
            ? "Error"
            : ds.status === "cancelled"
              ? "Cancelled"
              : "Idle"}</span
    >
    {#if ds.rootInfo}
      <span class="status-stat">{formatSize(ds.rootInfo.total_size)} total</span>
      <span class="status-sep">|</span>
      <span class="status-stat"
        >{ds.rootInfo.total_file_count.toLocaleString()} files</span
      >
    {/if}
    <span class="spacer"></span>
    {#if ds.selectedPath}
      <span class="path-hint" title={ds.selectedPath}>{ds.selectedPath}</span>
    {/if}
  </div>
</div>

{#if ctxMenu}
  <ContextMenu
    x={ctxMenu.x}
    y={ctxMenu.y}
    items={ctxItems()}
    onClose={() => (ctxMenu = null)}
  />
{/if}

<style>
  .du-view {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    background: var(--bg);
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    background: var(--bg-2);
    border-bottom: 1px solid var(--border);
  }

  .path-input {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 6px;
    background: var(--bg-surface);
    border: 1px solid var(--border-strong);
    border-radius: 5px;
    padding: 0 8px;
    height: 30px;
  }
  .path-input:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-bg);
  }
  .path-input input {
    flex: 1;
    background: transparent;
    border: none;
    outline: none;
    color: var(--text-strong);
    font-size: 13px;
    font-family: var(--mono);
  }
  .path-input .ico {
    color: var(--text-muted);
    display: flex;
  }
  .hint {
    font-size: 11px;
    color: var(--text-faint);
    font-style: italic;
  }

  .btn {
    height: 30px;
    padding: 0 12px;
    border-radius: 4px;
    font-size: 12px;
    font-weight: 600;
    border: 1px solid transparent;
    cursor: pointer;
    white-space: nowrap;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .btn.sm {
    height: 28px;
    padding: 0 10px;
  }
  .btn.primary {
    background: var(--accent);
    color: #fff;
  }
  .btn.primary:hover {
    background: var(--accent-hover);
  }
  .btn.danger {
    background: var(--danger);
    color: #fff;
  }
  .btn.danger:hover {
    background: #d83a32;
  }
  .btn.ghost {
    background: var(--bg-surface);
    color: var(--text-muted);
    border-color: var(--border);
  }
  .btn.ghost:hover {
    color: var(--text);
    background: var(--bg-hover);
  }
  .btn.ghost:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .banner {
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border);
    padding: 8px 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .b-row {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
    color: var(--text-muted);
    font-size: 12px;
  }
  .b-stat {
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .b-stat.err {
    color: var(--warning);
  }
  .dot {
    color: var(--border-strong);
  }
  .b-path {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .b-bar {
    height: 3px;
    background: var(--border);
    border-radius: 2px;
    overflow: hidden;
  }
  .b-fill {
    height: 100%;
    width: 30%;
    background: linear-gradient(90deg, var(--accent), var(--accent-strong));
    animation: idx-pulse 1.4s ease-in-out infinite;
  }
  @keyframes idx-pulse {
    0% { transform: translateX(-100%); }
    100% { transform: translateX(380%); }
  }

  .summary {
    display: flex;
    gap: 14px;
    padding: 8px 12px;
    background: var(--bg-2);
    border-bottom: 1px solid var(--border);
    align-items: center;
  }
  .sum-block {
    display: flex;
    flex-direction: column;
    line-height: 1.3;
    min-width: 0;
  }
  .sum-block.grow {
    flex: 1;
    min-width: 0;
  }
  .sum-label {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--text-faint);
  }
  .sum-value {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-strong);
    font-variant-numeric: tabular-nums;
  }
  .sum-value.mono { font-family: var(--mono); font-weight: 500; font-size: 12px; }
  .path-line {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-muted);
  }

  .error {
    color: var(--danger);
    font-size: 12px;
    padding: 6px 12px;
    background: rgba(248, 81, 73, 0.08);
    border-bottom: 1px solid rgba(248, 81, 73, 0.3);
  }

  .breadcrumbs {
    display: flex;
    align-items: center;
    gap: 2px;
    flex-wrap: wrap;
    padding: 4px 12px;
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border);
    font-size: 12px;
  }

  .selection-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    background: var(--accent-bg);
    border-bottom: 1px solid var(--accent);
    font-size: 12px;
  }
  .sel-count {
    color: var(--accent-strong);
    font-weight: 600;
    margin-right: auto;
  }
  .bc {
    color: var(--accent-strong);
    background: transparent;
    padding: 2px 6px;
    border-radius: 3px;
    font-size: 12px;
  }
  .bc:hover {
    background: var(--bg-hover);
  }
  .bc.current {
    color: var(--text-strong);
    cursor: default;
    font-weight: 600;
  }
  .bc-sep {
    color: var(--text-faint);
    margin: 0 1px;
  }

  .main-area {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
  .upper {
    display: flex;
    flex: 1 1 0;
    min-height: 0;
  }
  .table-wrap {
    flex: 1;
    overflow: auto;
    background: var(--bg-card);
    border-right: 1px solid var(--border);
    min-height: 0;
  }
  .side {
    flex: 0 0 320px;
    background: var(--bg-2);
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  table.du {
    width: 100%;
    border-collapse: collapse;
    table-layout: fixed;
    font-size: 12px;
  }
  table.du thead {
    position: sticky;
    top: 0;
    z-index: 2;
  }
  table.du th {
    background: var(--bg-surface);
    color: var(--text-muted);
    text-align: left;
    font-weight: 600;
    font-size: 11px;
    padding: 5px 8px;
    border-bottom: 1px solid var(--border);
    cursor: pointer;
    user-select: none;
    white-space: nowrap;
    text-transform: uppercase;
    letter-spacing: 0.4px;
  }
  table.du th.num {
    text-align: right;
  }
  table.du th:hover {
    color: var(--text);
    background: var(--bg-surface-2);
  }

  table.du td {
    padding: 0 8px;
    height: 26px;
    line-height: 26px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-bottom: 1px solid var(--border-soft);
    /* Click-to-select rows: don't let drag start a text selection.
       Copy path / Send to Rename context-menu actions cover the
       legitimate "give me this string" cases. */
    user-select: none;
    -webkit-user-select: none;
  }
  table.du td.check,
  table.du th.check {
    text-align: center;
    padding: 0 4px;
    cursor: default;
  }
  table.du input.cb {
    width: 16px;
    height: 16px;
    margin: 0;
    padding: 0;
    vertical-align: middle;
    cursor: pointer;
    accent-color: var(--accent);
    outline: 1px solid var(--border-strong);
    outline-offset: -1px;
    border-radius: 2px;
  }
  table.du input.cb:checked {
    outline-color: var(--accent);
  }
  /* Virtual-scroll spacer rows. Height comes from the inline style on the
     <tr>. We zero out td styling so the spacer never paints chrome. */
  tr.pad-row td {
    padding: 0;
    height: auto;
    line-height: 0;
    border-bottom: none;
    background: transparent;
  }
  tr.pad-row:hover td {
    background: transparent;
  }
  table.du td.num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  table.du td.muted {
    color: var(--text-muted);
  }
  tr.row:hover td {
    background: var(--bg-hover);
  }
  tr.row.selected td {
    background: rgba(47, 129, 247, 0.16);
  }

  .name-cell {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--text-strong);
  }
  .indent {
    flex-shrink: 0;
  }
  .caret {
    width: 14px;
    height: 14px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--text-muted);
    background: transparent;
    border-radius: 3px;
  }
  .caret:hover {
    color: var(--text);
    background: var(--bg-active);
  }
  .caret-spacer {
    width: 14px;
    display: inline-block;
  }
  .ico {
    display: inline-flex;
  }
  .name-text {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .pct {
    position: relative;
  }
  .pct-bar {
    position: absolute;
    inset: 5px 6px 5px 6px;
    background: var(--border-soft);
    border-radius: 2px;
    overflow: hidden;
  }
  .pct-fill {
    height: 100%;
    background: linear-gradient(90deg, var(--bar), var(--bar-strong));
    border-radius: 2px;
  }
  .pct-text {
    position: relative;
    z-index: 1;
    color: var(--text);
    font-weight: 500;
    text-shadow: 0 0 2px var(--bg);
  }

  .empty-folder {
    color: var(--text-muted);
    font-size: 13px;
    padding: 24px;
    text-align: center;
  }

  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    text-align: center;
    color: var(--text-muted);
    padding: 24px;
  }
  .empty-title {
    color: var(--text);
    font-weight: 600;
    font-size: 15px;
  }
  .empty-hint {
    font-size: 12.5px;
  }
  .engine-hint {
    margin-top: 8px;
    font-size: 11px;
    font-family: var(--mono);
    color: var(--text-faint);
  }
  .spinner {
    width: 24px;
    height: 24px;
    border: 3px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .side-tabs {
    display: flex;
    border-bottom: 1px solid var(--border);
  }
  .side-tabs button {
    flex: 1;
    padding: 7px 8px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    font-weight: 600;
    color: var(--text-muted);
    background: transparent;
    border-bottom: 2px solid transparent;
  }
  .side-tabs button:hover {
    color: var(--text);
    background: var(--bg-hover);
  }
  .side-tabs button.active {
    color: var(--accent-strong);
    border-bottom-color: var(--accent);
    background: var(--accent-bg);
  }
  .side-body {
    flex: 1;
    overflow: auto;
    padding: 6px 8px;
  }
  .side-empty {
    color: var(--text-faint);
    font-size: 12px;
    padding: 16px 8px;
    text-align: center;
  }

  .ext-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .ext-row {
    display: grid;
    grid-template-columns: 1fr 60px 70px 50px;
    grid-template-areas: "bar name size pct";
    align-items: center;
    column-gap: 6px;
    padding: 3px 6px;
    border-radius: 3px;
    position: relative;
    overflow: hidden;
  }
  .ext-row:hover {
    background: var(--bg-hover);
  }
  .ext-bar-wrap {
    grid-area: bar;
    position: relative;
    height: 100%;
    pointer-events: none;
  }
  .ext-bar {
    position: absolute;
    inset: 0;
    background: linear-gradient(
      90deg,
      rgba(47, 129, 247, 0.18),
      rgba(47, 129, 247, 0.04)
    );
    border-radius: 2px;
  }
  .ext-name {
    grid-area: name;
    font-family: var(--mono);
    font-size: 11px;
    color: var(--text-strong);
    text-transform: uppercase;
    letter-spacing: 0.4px;
    z-index: 1;
  }
  .ext-size {
    grid-area: size;
    text-align: right;
    font-variant-numeric: tabular-nums;
    color: var(--text);
    font-size: 11.5px;
    z-index: 1;
  }
  .ext-pct {
    grid-area: pct;
    text-align: right;
    color: var(--text-muted);
    font-size: 11px;
    z-index: 1;
  }

  .lg-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .lg-row {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 4px 6px;
    border-radius: 4px;
    background: var(--bg-surface);
    border: 1px solid var(--border-soft);
    text-align: left;
    cursor: default;
  }
  .lg-row:hover {
    border-color: var(--border-strong);
    background: var(--bg-surface-2);
  }
  .lg-name {
    color: var(--text-strong);
    font-size: 11.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .lg-bar {
    height: 4px;
    background: var(--border);
    border-radius: 2px;
    overflow: hidden;
  }
  .lg-fill {
    height: 100%;
    background: linear-gradient(90deg, var(--accent), var(--accent-strong));
    border-radius: 2px;
  }
  .lg-size {
    font-size: 10.5px;
    color: var(--text-muted);
    align-self: flex-end;
    font-variant-numeric: tabular-nums;
  }

  /* Treemap section */
  .treemap-section {
    flex: 0 0 auto;
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--border);
    background: var(--bg-2);
  }
  .treemap-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 12px;
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border);
    font-size: 11px;
  }
  .tm-title {
    color: var(--text-strong);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.6px;
  }
  .tm-sub {
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .tm-hint {
    color: var(--text-faint);
    margin-left: auto;
    font-style: italic;
  }

  .status-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 10px;
    background: var(--bg-2);
    border-top: 1px solid var(--border);
    font-size: 11px;
    color: var(--text-muted);
    flex-shrink: 0;
    min-height: 24px;
  }
  .status-stat {
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .status-sep {
    color: var(--border-strong);
  }
  .spacer {
    flex: 1;
  }
  .path-hint {
    color: var(--text-muted);
    font-family: var(--mono);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 60%;
  }

  .badge {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    font-weight: 600;
    padding: 1px 6px;
    border-radius: 3px;
    border: 1px solid;
    line-height: 14px;
  }
  .badge.idle {
    color: var(--text-muted);
    border-color: var(--border);
    background: var(--bg-surface);
  }
  .badge.indexing {
    color: var(--accent-strong);
    border-color: var(--accent);
    background: var(--accent-bg);
  }
  .badge.ready {
    color: var(--success);
    border-color: var(--success);
    background: rgba(63, 185, 80, 0.1);
  }
  .badge.error {
    color: var(--danger);
    border-color: var(--danger);
    background: rgba(248, 81, 73, 0.1);
  }
  .kbd {
    display: inline-block;
    padding: 1px 5px;
    font-family: var(--mono);
    font-size: 10px;
    background: var(--bg-surface);
    border: 1px solid var(--border-strong);
    border-radius: 3px;
    color: var(--text-muted);
    min-width: 16px;
    text-align: center;
  }
</style>
