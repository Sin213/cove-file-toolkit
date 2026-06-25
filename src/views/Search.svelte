<script lang="ts">
  import { onMount, tick } from "svelte";
  import ResultsTable from "../lib/components/ResultsTable.svelte";
  import ContextMenu from "../lib/components/ContextMenu.svelte";
  import ConfirmDialog from "../lib/components/ConfirmDialog.svelte";
  import {
    search,
    openPath,
    revealInFolder,
    moveToTrash,
    deletePermanently,
    formatElapsed,
    formatDate,
    addIndexRoot,
    removeIndexRoot,
    updateIndexRootEnabled,
    detectIndexRoots,
    type FileItem,
    type SearchFilters,
    type AppSettings,
    type IndexRootView,
    type DetectedRoot,
  } from "../lib/ipc";
  import {
    getIndexState,
    startIndexScan,
    startIndexAllRoots,
    cancelIndexScan,
    tryLoadCache,
    refreshIndexStats,
    syncIndexScanState,
    refreshIndexRoots,
    etaText,
  } from "../lib/scanStore.svelte";

  interface Props {
    sendToRename: (files: Array<{ name: string; path: string }>) => void;
    settings: AppSettings | null;
    focusSignal: boolean;
    refreshSignal: boolean;
    onCacheChange: () => void;
  }

  let {
    sendToRename,
    settings,
    focusSignal,
    refreshSignal,
    onCacheChange,
  }: Props = $props();

  // Global, persistent scan/index state (survives tab switches).
  const idx = getIndexState();

  let query = $state("");
  let showFilters = $state(false);

  let extFilter = $state("");
  let minSize = $state("");
  let maxSize = $state("");
  let pathInclude = $state("");
  let pathExclude = $state("");
  let typeFilter = $state<"all" | "files" | "dirs">("all");

  let sortField = $state("name");
  let sortAsc = $state(true);

  let results = $state<FileItem[]>([]);
  let totalResults = $state(0);
  let currentPage = $state(0);
  const PAGE_SIZE = 250;

  let selectedPaths = $state(new Set<string>());
  let activeIndex = $state(-1);
  let lastClickedIndex = $state(-1);

  let searchTimer: ReturnType<typeof setTimeout> | null = null;
  let searchInput: HTMLInputElement | undefined = $state();

  let ctxMenu = $state<{ x: number; y: number; item: FileItem } | null>(null);
  let confirmDialog = $state<{ title: string; message: string; confirmLabel: string; danger: boolean; onConfirm: () => void } | null>(null);

  // Surfaces the most recent open failure to the user (file not found,
  // launcher refused, etc.) so a failed double-click is never silent.
  let openError = $state<string>("");
  let openErrorTimer: ReturnType<typeof setTimeout> | null = null;

  // Track when index becomes ready and re-run search if there's an active query.
  let lastSeenStatus = $state(idx.status);

  function buildFilters(): SearchFilters {
    const f: SearchFilters = {};
    if (extFilter.trim()) {
      f.extensions = extFilter
        .split(",")
        .map((e) => e.trim())
        .filter(Boolean);
    }
    if (minSize.trim()) f.min_size = parseSizeInput(minSize);
    if (maxSize.trim()) f.max_size = parseSizeInput(maxSize);
    if (pathInclude.trim()) f.path_include = pathInclude.trim();
    if (pathExclude.trim()) f.path_exclude = pathExclude.trim();
    if (typeFilter === "files") f.files_only = true;
    if (typeFilter === "dirs") f.dirs_only = true;
    return f;
  }

  function parseSizeInput(s: string): number {
    const v = parseFloat(s);
    if (isNaN(v)) return 0;
    const lower = s.toLowerCase().trim();
    if (lower.endsWith("gb")) return v * 1073741824;
    if (lower.endsWith("mb")) return v * 1048576;
    if (lower.endsWith("kb")) return v * 1024;
    return v;
  }

  async function doSearch(reset = true) {
    if (idx.status !== "ready" && (idx.stats?.total ?? 0) === 0) return;
    if (reset) {
      currentPage = 0;
      activeIndex = -1;
      // A reset replaces the results array, so any prior selection now
      // refers to rows that may no longer exist. Clear it so the
      // selection count, context-menu labels, header select-all state,
      // and Send-to-Rename payload all describe the current results.
      // Pagination (reset=false, via onLoadMore) preserves selection
      // because it only appends to the existing list.
      clearSelection();
    }
    try {
      const res = await search(
        query,
        buildFilters(),
        { field: sortField, ascending: sortAsc },
        currentPage,
        PAGE_SIZE,
      );
      if (reset) results = res.items;
      else results = [...results, ...res.items];
      totalResults = res.total;
    } catch (e) {
      console.error("search error:", e);
    }
  }

  function onQueryInput() {
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => doSearch(), 80);
  }

  function onSort(field: string) {
    if (sortField === field) sortAsc = !sortAsc;
    else {
      sortField = field;
      sortAsc = field === "name" || field === "path" || field === "ext";
    }
    doSearch();
  }

  function onLoadMore() {
    currentPage++;
    doSearch(false);
  }

  function toggleSelect(path: string, multi: boolean, range: boolean) {
    const next = new Set(selectedPaths);
    const ix = results.findIndex((r) => r.path === path);
    if (range && lastClickedIndex >= 0) {
      const a = Math.min(lastClickedIndex, ix);
      const b = Math.max(lastClickedIndex, ix);
      for (let i = a; i <= b; i++) next.add(results[i].path);
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
    } else if (ix >= 0) {
      lastClickedIndex = ix;
    }
    selectedPaths = next;
  }

  function clearSelection() {
    selectedPaths = new Set();
    lastClickedIndex = -1;
  }

  function selectAll(sel: boolean) {
    if (sel) selectedPaths = new Set(results.map((r) => r.path));
    else clearSelection();
  }

  function setActiveIndex(i: number) {
    activeIndex = i;
  }

  async function activateItem(item: FileItem) {
    // Always use the canonical full path stored on the FileItem — never
    // any rendered/truncated string from the table cell. The Path column
    // is visually ellipsized via CSS but `item.path` carries the full
    // value untouched.
    const fullPath = item.path;
    console.log("[search:open]", {
      name: item.name,
      path: fullPath,
      is_dir: item.is_dir,
      root_id: item.root_id,
      root_path: item.root_path,
    });
    try {
      await openPath(fullPath);
      // Clear any stale error from a previous failure.
      if (openError) {
        openError = "";
        if (openErrorTimer) clearTimeout(openErrorTimer);
      }
    } catch (e) {
      const msg = `${e}`;
      console.error("[search:open] failed", fullPath, msg);
      openError = `Could not open: ${fullPath}\n${msg}`;
      if (openErrorTimer) clearTimeout(openErrorTimer);
      openErrorTimer = setTimeout(() => {
        openError = "";
      }, 8000);
    }
  }

  function onContextMenu(e: MouseEvent, item: FileItem) {
    ctxMenu = { x: e.clientX, y: e.clientY, item };
  }

  // Build the list of files to hand off to Rename, keyed by full path
  // (never any rendered/truncated cell text). Order is preserved by
  // walking the current `results` array — the result table the user is
  // looking at — so the Rename queue mirrors what they saw.
  function selectedItemsInOrder(): Array<{ name: string; path: string }> {
    return results
      .filter((r) => selectedPaths.has(r.path))
      .map((r) => ({ name: r.name, path: r.path }));
  }

  async function handleSendToRenameSelected() {
    const list = selectedItemsInOrder();
    if (list.length > 0) {
      sendToRename(list);
      clearSelection();
    }
  }

  // Right-click → Send to Rename. If the right-clicked row is part of a
  // multi-row selection we send everything currently selected; otherwise
  // we send only that row. The table's contextmenu handler has already
  // ensured an unselected row gets selected before this runs, so a single
  // right-click also lands in this same path.
  function handleSendContextToRename(item: FileItem) {
    if (selectedPaths.size > 1 && selectedPaths.has(item.path)) {
      handleSendToRenameSelected();
    } else {
      sendToRename([{ name: item.name, path: item.path }]);
    }
  }

  async function handleStartScan() {
    const configured = settings?.indexed_roots ?? [];
    const enabled = configured.filter((r) => r.enabled);
    // Validate BEFORE flipping into "indexing" so the UI can never get
    // stuck on the "Starting…" banner waiting for a job that the backend
    // will reject (or that we shouldn't even be sending). Existing loaded
    // cache/index is left untouched because we never enter the scan path.
    if (configured.length > 0 && enabled.length === 0) {
      idx.status = "error";
      idx.error =
        "All indexed locations are disabled. Enable at least one in Locations or Settings before indexing.";
      return;
    }
    if (configured.length === 0) {
      // First-boot fallback: index $HOME so the user gets a working search
      // without a Settings detour. Only kicks in when nothing is configured
      // — we no longer silently fall back when the user has explicitly
      // configured roots and disabled them.
      const home = (settings?.default_root || "/home").trim();
      if (!home) {
        idx.status = "error";
        idx.error =
          "Select at least one folder to index — open Settings to add an indexed location.";
        return;
      }
      await startIndexScan([home]);
    } else {
      await startIndexAllRoots();
    }
    await refreshIndexRoots();
  }

  // ---- Indexed Locations panel ----
  let showRoots = $state(false);
  let newRootPath = $state("");
  let rootError = $state("");
  let detected = $state<DetectedRoot[]>([]);
  let detecting = $state(false);

  function getRoots(): IndexRootView[] {
    return idx.rootViews ?? [];
  }

  async function handleAddRoot() {
    const p = newRootPath.trim();
    if (!p) return;
    rootError = "";
    try {
      await addIndexRoot(p);
      newRootPath = "";
      await refreshIndexRoots();
      onCacheChange();
    } catch (e) {
      rootError = `${e}`;
    }
  }

  async function handleRemoveRoot(id: string) {
    try {
      await removeIndexRoot(id);
      await refreshIndexRoots();
      onCacheChange();
    } catch (e) {
      rootError = `${e}`;
    }
  }

  async function handleToggleEnabled(id: string, enabled: boolean) {
    try {
      await updateIndexRootEnabled(id, enabled);
      await refreshIndexRoots();
      onCacheChange();
    } catch (e) {
      rootError = `${e}`;
    }
  }

  async function handleDetect() {
    detecting = true;
    rootError = "";
    try {
      detected = await detectIndexRoots();
    } catch (e) {
      rootError = `${e}`;
    } finally {
      detecting = false;
    }
  }

  async function addDetected(d: DetectedRoot) {
    rootError = "";
    try {
      await addIndexRoot(d.path, d.display_name);
      detected = detected.filter((x) => x.path !== d.path);
      await refreshIndexRoots();
      onCacheChange();
    } catch (e) {
      rootError = `${e}`;
    }
  }

  function rootStateLabel(s: string): string {
    switch (s) {
      case "indexing":
        return "Indexing";
      case "ready":
        return "Ready";
      case "pending":
        return "Pending";
      case "error":
        return "Error";
      case "disabled":
        return "Off";
      case "missing":
        return "Missing";
      default:
        return "Idle";
    }
  }

  async function handleStopScan() {
    await cancelIndexScan();
  }

  async function handleLoadCache() {
    if (await tryLoadCache()) {
      doSearch();
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Delete" && selectedPaths.size > 0 && document.activeElement !== searchInput) {
      e.preventDefault();
      const paths = getSelectedPaths();
      if (e.shiftKey) {
        handleDeletePermanently(paths);
      } else {
        handleTrash(paths);
      }
      return;
    }
    if (e.key === "Escape") {
      if (ctxMenu) {
        ctxMenu = null;
        e.preventDefault();
        return;
      }
      if (query) {
        query = "";
        doSearch();
        e.preventDefault();
        return;
      }
    }
    if (
      e.key === "Enter" &&
      activeIndex >= 0 &&
      activeIndex < results.length
    ) {
      activateItem(results[activeIndex]);
      e.preventDefault();
      return;
    }
    if (e.key === "ArrowDown" && document.activeElement === searchInput) {
      if (results.length > 0) {
        activeIndex = Math.min(results.length - 1, activeIndex + 1);
        e.preventDefault();
      }
      return;
    }
    if (e.key === "ArrowUp" && document.activeElement === searchInput) {
      if (results.length > 0) {
        activeIndex = Math.max(0, activeIndex - 1);
        e.preventDefault();
      }
      return;
    }
  }

  $effect(() => {
    if (focusSignal) {
      tick().then(() => searchInput?.focus());
      searchInput?.select();
    }
  });

  $effect(() => {
    if (refreshSignal) {
      handleStartScan();
    }
  });

  // When status transitions to "ready" (e.g. scan finished while we were on
  // another tab) re-run the search and tell App to refresh cache info.
  // When status transitions to "idle" with no index loaded (e.g. Clear
  // Cache), drop component-local results/selection so we don't keep
  // rendering stale rows over a wiped index — and so the next "ready"
  // (after a rebuild) actually re-runs the search instead of being
  // suppressed by the stale results.length check below.
  $effect(() => {
    const cur = idx.status;
    if (lastSeenStatus !== cur) {
      const prev = lastSeenStatus;
      lastSeenStatus = cur;
      if (cur === "ready") {
        onCacheChange();
        if (results.length === 0) doSearch();
      } else if (cur === "idle" && prev !== "idle") {
        results = [];
        totalResults = 0;
        currentPage = 0;
        activeIndex = -1;
        clearSelection();
      }
    }
  });

  onMount(async () => {
    window.addEventListener("keydown", onKey);

    // Reconcile with the backend's authoritative scan state. Handles the
    // case where the user switched away during indexing and the complete
    // event was missed, or where the index finished before the listener
    // was wired up.
    await syncIndexScanState();

    // If the index already has data (e.g. cache was loaded at boot, or another
    // tab triggered a scan), surface results immediately.
    if ((idx.stats?.total ?? 0) > 0 && results.length === 0) {
      doSearch();
    } else {
      // Try to refresh stats in case backend has data we don't know about.
      await refreshIndexStats();
      if ((idx.stats?.total ?? 0) > 0) doSearch();
    }

    return () => window.removeEventListener("keydown", onKey);
  });

  // Status bar derived values
  let filesPerSec = $derived.by(() => {
    if (!idx.progress || idx.progress.elapsed_ms <= 0) return 0;
    return Math.round(
      idx.progress.files_found / (idx.progress.elapsed_ms / 1000),
    );
  });

  let etaLabel = $derived.by(() => {
    if (!idx.progress) return "calculating…";
    const seen = idx.progress.files_found + idx.progress.dirs_found;
    const itemsPerSec =
      idx.progress.elapsed_ms > 0
        ? Math.round(seen / (idx.progress.elapsed_ms / 1000))
        : 0;
    return etaText(seen, idx.prevTotal, itemsPerSec, idx.progress.elapsed_ms);
  });

  let indexRootsLine = $derived.by(() => {
    const roots = idx.stats?.roots ?? idx.cacheInfo?.roots ?? [];
    return roots.length === 0
      ? "no roots"
      : roots.length === 1
        ? roots[0]
        : `${roots.length} roots`;
  });

  let lastIndexed = $derived.by(() => {
    const ts = idx.stats?.indexed_at ?? idx.cacheInfo?.timestamp ?? 0;
    return ts > 0 ? formatDate(ts) : "—";
  });

  function getSelectedPaths(): string[] {
    return [...selectedPaths];
  }

  function handleTrash(paths: string[]) {
    const count = paths.length;
    confirmDialog = {
      title: "Move to Trash",
      message: `Move ${count} item${count > 1 ? "s" : ""} to system trash?`,
      confirmLabel: "Move to Trash",
      danger: true,
      onConfirm: async () => {
        confirmDialog = null;
        try {
          await moveToTrash(paths);
          for (const p of paths) selectedPaths.delete(p);
          selectedPaths = new Set(selectedPaths);
          doSearch();
        } catch (e) {
          confirmDialog = { title: "Error", message: String(e), confirmLabel: "OK", danger: false, onConfirm: () => { confirmDialog = null; } };
        }
      },
    };
  }

  function handleDeletePermanently(paths: string[]) {
    const count = paths.length;
    confirmDialog = {
      title: "Delete Permanently",
      message: `Permanently delete ${count} item${count > 1 ? "s" : ""}? This cannot be undone.`,
      confirmLabel: "Delete Permanently",
      danger: true,
      onConfirm: async () => {
        confirmDialog = null;
        try {
          await deletePermanently(paths);
          for (const p of paths) selectedPaths.delete(p);
          selectedPaths = new Set(selectedPaths);
          doSearch();
        } catch (e) {
          confirmDialog = { title: "Error", message: String(e), confirmLabel: "OK", danger: false, onConfirm: () => { confirmDialog = null; } };
        }
      },
    };
  }

  function ctxItems() {
    if (!ctxMenu) return [];
    const item = ctxMenu.item;
    // Multi-select right-click: if the clicked row is part of an
    // existing multi-row selection, the menu's primary "send" action
    // operates on the whole selection rather than just this row.
    const multi = selectedPaths.size > 1 && selectedPaths.has(item.path);
    const opPaths = multi ? getSelectedPaths() : [item.path];
    return [
      {
        label: item.is_dir ? "Open folder" : "Open",
        action: () => activateItem(item),
      },
      {
        label: "Open containing folder",
        action: () => revealInFolder(item.path).catch(() => {}),
      },
      { separator: true } as any,
      {
        label: "Copy path",
        action: () => navigator.clipboard?.writeText(item.path).catch(() => {}),
      },
      {
        label: "Copy name",
        action: () => navigator.clipboard?.writeText(item.name).catch(() => {}),
      },
      { separator: true } as any,
      multi
        ? {
            label: `Send ${selectedPaths.size} selected to Rename`,
            action: () => handleSendToRenameSelected(),
          }
        : {
            label: "Send to Rename",
            action: () => handleSendContextToRename(item),
          },
      { separator: true } as any,
      {
        label: multi ? `Move ${opPaths.length} items to Trash` : "Move to Trash",
        danger: true,
        action: () => handleTrash(opPaths),
      },
      {
        label: multi ? `Delete ${opPaths.length} items permanently` : "Delete permanently",
        danger: true,
        action: () => handleDeletePermanently(opPaths),
      },
    ];
  }

  let placeholder = $derived(
    idx.status === "ready"
      ? "Search files and folders... (use * and ? for wildcards)"
      : idx.status === "indexing"
        ? "Indexing — search will be ready as soon as indexing completes"
        : idx.status === "loading-cache"
          ? "Loading cached index…"
          : "No index loaded — click Index Now or open Settings",
  );

  let primaryBtnLabel = $derived(
    (idx.stats?.total ?? 0) > 0 ? "Force Rebuild" : "Index Now",
  );

  let primaryBtnTitle = $derived(
    (idx.stats?.total ?? 0) > 0
      ? "Deletes and rebuilds the search index/cache. Use this if files are missing or results look stale."
      : "Build the search index across configured roots so search becomes instant.",
  );
</script>

<div class="search-view">
  <!-- Top toolbar -->
  <div class="toolbar">
    <div class="search-box">
      <span class="ico">
        <svg viewBox="0 0 16 16" width="14" height="14"
          ><path
            d="M11.5 7.5a4 4 0 1 1-8 0 4 4 0 0 1 8 0Zm-1.1 3.4a5.5 5.5 0 1 1 1.06-1.06l3.34 3.34a.75.75 0 0 1-1.06 1.06l-3.34-3.34Z"
            fill="currentColor"
          /></svg
        >
      </span>
      <input
        bind:this={searchInput}
        bind:value={query}
        oninput={onQueryInput}
        type="text"
        class="search-input"
        placeholder={placeholder}
        autocomplete="off"
        spellcheck="false"
      />
      {#if query}
        <button
          class="clear"
          title="Clear (Esc)"
          onclick={() => {
            query = "";
            doSearch();
            searchInput?.focus();
          }}
        >×</button>
      {/if}
    </div>

    <div class="toolbar-actions">
      <button
        class="btn ghost sm"
        class:active={showRoots}
        onclick={() => (showRoots = !showRoots)}
        title="Manage indexed locations"
      >Locations</button>
      <button
        class="btn ghost sm"
        class:active={showFilters}
        onclick={() => (showFilters = !showFilters)}
        title="Toggle filters"
      >Filters</button>

      {#if idx.status === "indexing"}
        <button class="btn danger sm" onclick={handleStopScan}>Cancel scan</button>
      {:else if idx.status === "loading-cache"}
        <button class="btn ghost sm" disabled>Loading…</button>
      {:else if (idx.stats?.total ?? 0) === 0 && idx.cacheInfo}
        <button
          class="btn ghost sm"
          onclick={handleLoadCache}
          title="Load the saved index from disk"
        >Load cache</button>
        <button
          class="btn primary sm"
          onclick={handleStartScan}
          title={primaryBtnTitle}
        >Index Now</button>
      {:else}
        <button
          class="btn primary sm"
          onclick={handleStartScan}
          title={primaryBtnTitle}
        >{primaryBtnLabel}</button>
      {/if}
    </div>
  </div>

  <!-- Optional filter bar -->
  {#if showFilters}
    <div class="filter-row">
      <label class="filt"
        ><span>Extensions</span
        ><input
          type="text"
          bind:value={extFilter}
          oninput={() => doSearch()}
          placeholder="e.g. rs,ts,json"
        /></label
      >
      <label class="filt"
        ><span>Min size</span
        ><input
          type="text"
          bind:value={minSize}
          oninput={() => doSearch()}
          placeholder="1KB"
        /></label
      >
      <label class="filt"
        ><span>Max size</span
        ><input
          type="text"
          bind:value={maxSize}
          oninput={() => doSearch()}
          placeholder="100MB"
        /></label
      >
      <label class="filt"
        ><span>Path includes</span
        ><input
          type="text"
          bind:value={pathInclude}
          oninput={() => doSearch()}
          placeholder="*/Documents/*"
        /></label
      >
      <label class="filt"
        ><span>Path excludes</span
        ><input
          type="text"
          bind:value={pathExclude}
          oninput={() => doSearch()}
          placeholder="*/.git/*"
        /></label
      >
      <label class="filt small"
        ><span>Type</span
        ><select bind:value={typeFilter} onchange={() => doSearch()}>
          <option value="all">All</option>
          <option value="files">Files only</option>
          <option value="dirs">Folders only</option>
        </select></label
      >
    </div>
  {/if}

  <!-- Indexed Locations panel -->
  {#if showRoots}
    <div class="roots-panel">
      <div class="roots-head">
        <div class="roots-title">Indexed Locations</div>
        <div class="roots-head-actions">
          <button
            class="btn ghost xs"
            onclick={handleDetect}
            disabled={detecting}
            title="Find mounted drives that look indexable"
          >{detecting ? "Detecting…" : "Detect drives"}</button>
        </div>
      </div>
      <p class="roots-hint">
        Add multiple folders or drives — Search returns combined results across
        every enabled root.
      </p>

      {#if getRoots().length === 0}
        <div class="roots-empty">No locations configured. Add a folder below.</div>
      {:else}
        <ul class="roots-list">
          {#each getRoots() as r}
            <li class="root-row" class:disabled={!r.enabled}>
              <input
                type="checkbox"
                class="enable-cb"
                checked={r.enabled}
                onchange={() => handleToggleEnabled(r.id, !r.enabled)}
                title={r.enabled ? "Enabled" : "Disabled"}
              />
              <span class="root-name" title={r.display_name}>{r.display_name}</span>
              <span class="root-path mono" title={r.path}>{r.path}</span>
              <span class="root-state-cell">
                <span class="state-pill state-{r.state}">{rootStateLabel(r.state)}</span>
                {#if r.item_count > 0}
                  <span class="root-count" title="Indexed items">{r.item_count.toLocaleString()}</span>
                {/if}
                {#if r.last_indexed > 0}
                  <span class="root-time" title="Last indexed">{formatDate(r.last_indexed)}</span>
                {/if}
              </span>
              <button
                class="root-rm"
                onclick={() => handleRemoveRoot(r.id)}
                title="Remove this location"
              >×</button>
            </li>
            {#if r.error}
              <li class="root-err">{r.error}</li>
            {/if}
          {/each}
        </ul>
      {/if}

      <div class="roots-add-row">
        <input
          type="text"
          bind:value={newRootPath}
          onkeydown={(e) => e.key === "Enter" && handleAddRoot()}
          placeholder="/mnt/storage   or   /home/sin   or   /run/media/sin/Drive"
          class="roots-add-input"
          autocomplete="off"
          spellcheck="false"
        />
        <button class="btn primary xs" onclick={handleAddRoot}>Add location</button>
      </div>

      {#if rootError}
        <div class="roots-error">{rootError}</div>
      {/if}

      {#if detected.length > 0}
        <div class="detected-block">
          <div class="detected-title">Detected drives</div>
          <div class="detected-list">
            {#each detected as d}
              <button
                class="detected-pill"
                onclick={() => addDetected(d)}
                title={d.path}
              >+ {d.display_name}</button>
            {/each}
          </div>
        </div>
      {/if}
    </div>
  {/if}

  <!-- Indexing banner -->
  {#if idx.status === "indexing"}
    <div class="index-banner">
      <div class="ib-row">
        <span class="badge indexing">Indexing</span>
        {#if idx.progress}
          <span class="ib-stat"
            >{idx.progress.files_found.toLocaleString()} files</span
          >
          <span class="dot">•</span>
          <span class="ib-stat"
            >{idx.progress.dirs_found.toLocaleString()} folders</span
          >
          <span class="dot">•</span>
          <span class="ib-stat">{formatElapsed(idx.progress.elapsed_ms)}</span>
          <span class="dot">•</span>
          <span class="ib-stat">{filesPerSec.toLocaleString()} files/s</span>
          <span class="dot">•</span>
          <span class="ib-stat" title="Estimated time remaining">ETA: {etaLabel}</span>
          {#if idx.progress.roots_total > 1}
            <span class="dot">•</span>
            <span class="ib-stat"
              >Root {idx.progress.roots_done + 1} of {idx.progress.roots_total}</span
            >
          {/if}
        {:else}
          <span class="ib-stat">Starting…</span>
        {/if}
      </div>
      {#if idx.progress?.current_path}
        <div class="ib-path" title={idx.progress.current_path}>
          {idx.progress.current_path}
        </div>
      {/if}
      <div class="ib-bar">
        <div class="ib-fill"></div>
      </div>
    </div>
  {/if}

  {#if idx.error}
    <div class="error">{idx.error}</div>
  {/if}

  {#if openError}
    <div class="error open-error">
      <span class="open-error-msg">{openError}</span>
      <button
        class="open-error-close"
        title="Dismiss"
        onclick={() => {
          openError = "";
          if (openErrorTimer) clearTimeout(openErrorTimer);
        }}
      >×</button>
    </div>
  {/if}

  {#if selectedPaths.size > 0}
    <div class="selection-bar">
      <span class="sel-count">{selectedPaths.size} selected</span>
      <button class="btn primary sm" onclick={handleSendToRenameSelected}
        >Send to Rename</button
      >
      <button class="btn ghost sm" onclick={clearSelection}
        >Clear selection</button
      >
    </div>
  {/if}

  <!-- Results -->
  {#if (idx.status === "ready" || (idx.stats?.total ?? 0) > 0) && results.length > 0}
    <ResultsTable
      items={results}
      total={totalResults}
      {sortField}
      {sortAsc}
      {onSort}
      {onLoadMore}
      {selectedPaths}
      onToggleSelect={toggleSelect}
      onSelectAll={selectAll}
      onActivate={activateItem}
      onContext={onContextMenu}
      {activeIndex}
      {setActiveIndex}
    />
  {:else if (idx.status === "ready" || (idx.stats?.total ?? 0) > 0) && query}
    <div class="empty">
      <div class="empty-title">No matches</div>
      <div class="empty-hint">
        Nothing in {(idx.stats?.total ?? 0).toLocaleString()} indexed entries matches “{query}”.
      </div>
    </div>
  {:else if idx.status === "ready" || (idx.stats?.total ?? 0) > 0}
    <div class="empty">
      <div class="empty-title">Search ready</div>
      <div class="empty-hint">
        Type to search across {(idx.stats?.total ?? 0).toLocaleString()} indexed entries.<br
        />
        Wildcards: <span class="kbd">*</span>
        <span class="kbd">?</span> · case-insensitive · matches name and path.
      </div>
    </div>
  {:else if idx.status === "indexing"}
    <div class="empty">
      <div class="empty-title">Building index…</div>
      <div class="empty-hint">
        Search becomes available as soon as indexing completes. You can switch
        tabs — indexing keeps running.
      </div>
    </div>
  {:else if idx.status === "loading-cache"}
    <div class="empty">
      <div class="empty-title">Loading cached index…</div>
    </div>
  {:else}
    <div class="empty">
      <div class="empty-title">No index loaded</div>
      <div class="empty-hint">
        {#if (settings?.indexed_roots?.length ?? 0) === 0}
          No indexed roots configured yet. Click Index Now to start with
          <span class="kbd">{settings?.default_root || "$HOME"}</span>, or open
          Settings to pick custom roots.
        {:else}
          Indexed roots are configured. Click Index Now to build a fast search
          index.
        {/if}
      </div>
      <button
        class="btn primary"
        onclick={handleStartScan}
        title={primaryBtnTitle}
      >Index Now</button>
    </div>
  {/if}

  <!-- Status bar -->
  <div class="status-bar">
    <span
      class="badge"
      class:idle={idx.status === "idle"}
      class:indexing={idx.status === "indexing"}
      class:ready={idx.status === "ready" || (idx.stats?.total ?? 0) > 0}
      class:error={idx.status === "error"}
      >{idx.status === "ready"
        ? "Ready"
        : idx.status === "indexing"
          ? "Indexing"
          : idx.status === "loading-cache"
            ? "Loading"
            : idx.status === "error"
              ? "Error"
              : idx.status === "cancelled"
                ? "Cancelled"
                : (idx.stats?.total ?? 0) > 0
                  ? "Cache loaded"
                  : "Idle"}</span
    >
    <span class="status-stat"
      >{totalResults.toLocaleString()} results</span
    >
    <span class="status-sep">|</span>
    <span class="status-stat"
      >{(idx.stats?.total ?? 0).toLocaleString()} indexed</span
    >
    {#if idx.stats}
      <span class="status-sep">|</span>
      <span class="status-stat"
        >{idx.stats.files.toLocaleString()} files / {idx.stats.dirs.toLocaleString()} folders</span
      >
    {/if}
    <span class="status-sep">|</span>
    <span class="status-stat" title={indexRootsLine}>{indexRootsLine}</span>
    <span class="status-sep">|</span>
    <span class="status-stat">last: {lastIndexed}</span>
    <span class="spacer"></span>
    {#if activeIndex >= 0 && activeIndex < results.length}
      <span class="status-stat path-hint" title={results[activeIndex].path}
        >{results[activeIndex].path}</span
      >
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

{#if confirmDialog}
  <ConfirmDialog
    title={confirmDialog.title}
    message={confirmDialog.message}
    confirmLabel={confirmDialog.confirmLabel}
    danger={confirmDialog.danger}
    onConfirm={confirmDialog.onConfirm}
    onCancel={() => (confirmDialog = null)}
  />
{/if}

<style>
  .search-view {
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

  .search-box {
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
  .search-box:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-bg);
  }
  .search-box .ico {
    color: var(--text-muted);
    display: flex;
  }
  .search-input {
    flex: 1;
    background: transparent;
    border: none;
    outline: none;
    color: var(--text-strong);
    font-size: 13.5px;
  }
  .search-input::placeholder {
    color: var(--text-faint);
  }
  .clear {
    color: var(--text-muted);
    font-size: 16px;
    line-height: 1;
    padding: 0 6px;
    border-radius: 3px;
  }
  .clear:hover {
    color: var(--text);
    background: var(--bg-hover);
  }

  .toolbar-actions {
    display: flex;
    align-items: center;
    gap: 6px;
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
    transition: background 0.12s, border-color 0.12s, color 0.12s;
  }
  .btn.sm {
    height: 28px;
    padding: 0 10px;
  }
  .btn.xs {
    height: 24px;
    padding: 0 9px;
    font-size: 11px;
  }
  .btn.primary {
    background: var(--accent);
    color: #fff;
  }
  .btn.primary:hover {
    background: var(--accent-hover);
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
  .btn.ghost.active {
    color: var(--accent-strong);
    border-color: var(--accent);
    background: var(--accent-bg);
  }
  .btn.ghost:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .btn.danger {
    background: var(--danger);
    color: #fff;
  }
  .btn.danger:hover {
    background: #d83a32;
  }

  .filter-row {
    display: flex;
    gap: 8px;
    padding: 6px 10px;
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border);
    flex-wrap: wrap;
  }
  .filt {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 110px;
    gap: 2px;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-faint);
  }
  .filt.small {
    flex: 0 0 110px;
  }
  .filt input,
  .filt select {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 4px 6px;
    color: var(--text-strong);
    font-size: 12px;
    text-transform: none;
  }
  .filt input:focus,
  .filt select:focus {
    outline: none;
    border-color: var(--accent);
  }
  .filt select {
    -webkit-appearance: none;
    appearance: none;
    background-image: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 12 8'><path fill='%238b949e' d='M6 8 0 0h12z'/></svg>");
    background-repeat: no-repeat;
    background-position: right 6px center;
    background-size: 8px 6px;
    padding-right: 20px;
    cursor: pointer;
  }
  .filt select:hover {
    border-color: var(--border-strong);
  }
  /* Dropdown menu items — webkit/gtk respects these on the popup. */
  .filt select option {
    background-color: var(--bg-surface);
    color: var(--text-strong);
  }
  .filt select option:checked,
  .filt select option:hover {
    background-color: var(--bg-active);
    color: var(--text-strong);
  }

  .index-banner {
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border);
    padding: 8px 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .ib-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    font-size: 12px;
    color: var(--text-muted);
  }
  .ib-stat {
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .dot {
    color: var(--border-strong);
    font-size: 10px;
  }
  .ib-path {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .ib-bar {
    height: 3px;
    background: var(--border);
    border-radius: 2px;
    overflow: hidden;
  }
  .ib-fill {
    height: 100%;
    width: 30%;
    background: linear-gradient(90deg, var(--accent), var(--accent-strong));
    animation: idx-pulse 1.4s ease-in-out infinite;
  }
  @keyframes idx-pulse {
    0% {
      transform: translateX(-100%);
    }
    100% {
      transform: translateX(380%);
    }
  }

  .error {
    color: var(--danger);
    font-size: 12px;
    padding: 6px 12px;
    background: rgba(248, 81, 73, 0.08);
    border-bottom: 1px solid rgba(248, 81, 73, 0.3);
  }
  .open-error {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    white-space: pre-line;
    word-break: break-all;
    font-family: var(--mono);
    font-size: 11.5px;
    line-height: 1.4;
  }
  .open-error-msg {
    flex: 1;
  }
  .open-error-close {
    color: var(--danger);
    background: transparent;
    border: none;
    cursor: pointer;
    font-size: 14px;
    line-height: 1;
    padding: 0 4px;
    border-radius: 3px;
  }
  .open-error-close:hover {
    background: rgba(248, 81, 73, 0.18);
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

  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    text-align: center;
    color: var(--text-muted);
    padding: 24px;
  }
  .empty-title {
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
  }
  .empty-hint {
    font-size: 12.5px;
    color: var(--text-muted);
    line-height: 1.5;
  }
  .empty .kbd {
    margin: 0 2px;
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
    max-width: 50%;
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

  /* ---- Indexed Locations panel ---- */
  .roots-panel {
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border);
    padding: 8px 12px 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .roots-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
  }
  .roots-title {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--text-strong);
    font-weight: 700;
  }
  .roots-head-actions {
    display: flex;
    gap: 6px;
  }
  .roots-hint {
    margin: 0;
    font-size: 11.5px;
    color: var(--text-muted);
  }
  .roots-empty {
    font-size: 12px;
    color: var(--text-faint);
    padding: 8px 0;
  }
  .roots-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .root-row {
    display: grid;
    grid-template-columns: auto 130px 1fr auto auto;
    gap: 8px;
    align-items: center;
    padding: 4px 8px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    font-size: 12px;
  }
  .root-row.disabled {
    opacity: 0.6;
  }
  .root-row .enable-cb {
    accent-color: var(--accent);
    cursor: pointer;
  }
  .root-name {
    color: var(--text);
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .root-path {
    color: var(--text-muted);
    font-family: var(--mono);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .root-state-cell {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-muted);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .root-count {
    color: var(--text);
  }
  .root-time {
    color: var(--text-faint);
    font-family: var(--mono);
    font-size: 10.5px;
  }
  .state-pill {
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    font-weight: 700;
    padding: 1px 5px;
    border-radius: 3px;
    border: 1px solid;
    line-height: 14px;
    white-space: nowrap;
  }
  .state-pill.state-idle {
    color: var(--text-muted);
    border-color: var(--border-strong);
    background: var(--bg-surface);
  }
  .state-pill.state-pending {
    color: var(--text);
    border-color: var(--border-strong);
    background: var(--bg-surface);
  }
  .state-pill.state-indexing {
    color: var(--accent-strong);
    border-color: var(--accent);
    background: var(--accent-bg);
  }
  .state-pill.state-ready {
    color: var(--success);
    border-color: var(--success);
    background: rgba(63, 185, 80, 0.1);
  }
  .state-pill.state-error,
  .state-pill.state-missing {
    color: var(--danger);
    border-color: var(--danger);
    background: rgba(248, 81, 73, 0.1);
  }
  .state-pill.state-disabled {
    color: var(--text-faint);
    border-color: var(--border);
    background: var(--bg-surface);
  }
  .root-rm {
    color: var(--text-faint);
    font-size: 14px;
    padding: 0 6px;
    border-radius: 3px;
    background: transparent;
    border: none;
    cursor: pointer;
  }
  .root-rm:hover {
    color: var(--danger);
    background: rgba(248, 81, 73, 0.12);
  }
  .root-err {
    list-style: none;
    color: var(--danger);
    font-size: 11px;
    padding: 2px 8px 4px 8px;
  }
  .roots-add-row {
    display: flex;
    gap: 6px;
    align-items: center;
    margin-top: 4px;
  }
  .roots-add-input {
    flex: 1;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 5px 8px;
    color: var(--text);
    font-size: 12px;
    font-family: var(--mono);
  }
  .roots-add-input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .roots-error {
    color: var(--danger);
    font-size: 11.5px;
    padding-top: 2px;
  }
  .detected-block {
    margin-top: 6px;
    border-top: 1px dashed var(--border);
    padding-top: 6px;
  }
  .detected-title {
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-muted);
    margin-bottom: 4px;
  }
  .detected-list {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .detected-pill {
    background: var(--bg);
    border: 1px solid var(--border-strong);
    color: var(--text);
    font-size: 11px;
    padding: 3px 8px;
    border-radius: 999px;
    cursor: pointer;
  }
  .detected-pill:hover {
    background: var(--bg-hover);
    border-color: var(--accent);
  }
</style>
