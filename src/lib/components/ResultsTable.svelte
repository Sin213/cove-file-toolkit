<script lang="ts">
  import {
    formatSize,
    formatDate,
    fileExtension,
    type FileItem,
  } from "../ipc";

  interface Props {
    items: FileItem[];
    total: number;
    sortField: string;
    sortAsc: boolean;
    onSort: (field: string) => void;
    onLoadMore: () => void;
    selectedPaths: Set<string>;
    onToggleSelect: (path: string, multi: boolean, range: boolean) => void;
    onSelectAll: (sel: boolean) => void;
    onActivate: (item: FileItem) => void;
    onContext: (e: MouseEvent, item: FileItem) => void;
    activeIndex: number;
    setActiveIndex: (i: number) => void;
  }

  let {
    items,
    total,
    sortField,
    sortAsc,
    onSort,
    onLoadMore,
    selectedPaths,
    onToggleSelect,
    onSelectAll,
    onActivate,
    onContext,
    activeIndex,
    setActiveIndex,
  }: Props = $props();

  const ROW_HEIGHT = 24;
  const BUFFER = 8;

  let container: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let containerHeight = $state(600);

  let selectedVisible = $derived(
    items.reduce((n, i) => (selectedPaths.has(i.path) ? n + 1 : n), 0),
  );
  let allSelected = $derived(
    items.length > 0 && selectedVisible === items.length,
  );
  // The header checkbox shows an indeterminate state when SOME but not
  // all visible rows are selected — standard tri-state UX. Bound through
  // an effect since `indeterminate` is an HTMLInputElement DOM property,
  // not a Svelte attribute binding.
  let headerCb: HTMLInputElement | undefined = $state();
  $effect(() => {
    if (!headerCb) return;
    headerCb.indeterminate = selectedVisible > 0 && selectedVisible < items.length;
  });

  function handleScroll() {
    if (!container) return;
    scrollTop = container.scrollTop;
    const dist =
      container.scrollHeight - container.scrollTop - container.clientHeight;
    if (dist < 300 && items.length < total) onLoadMore();
  }

  let visibleStart = $derived(
    Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - BUFFER),
  );
  let visibleEnd = $derived(
    Math.min(
      items.length,
      Math.ceil((scrollTop + containerHeight) / ROW_HEIGHT) + BUFFER,
    ),
  );
  let visibleItems = $derived(items.slice(visibleStart, visibleEnd));
  let topPad = $derived(visibleStart * ROW_HEIGHT);
  let bottomPad = $derived(
    Math.max(0, items.length * ROW_HEIGHT - visibleEnd * ROW_HEIGHT),
  );

  function indicator(field: string): string {
    if (field !== sortField) return "";
    return sortAsc ? " ▲" : " ▼";
  }

  function onRowClick(e: MouseEvent, idx: number, item: FileItem) {
    setActiveIndex(idx);
    onToggleSelect(item.path, e.ctrlKey || e.metaKey, e.shiftKey);
  }

  $effect(() => {
    if (!container) return;
    const obs = new ResizeObserver((entries) => {
      containerHeight = entries[0].contentRect.height;
    });
    obs.observe(container);
    return () => obs.disconnect();
  });

  function scrollIntoView(idx: number) {
    if (!container) return;
    const top = idx * ROW_HEIGHT;
    const bottom = top + ROW_HEIGHT;
    if (top < container.scrollTop) container.scrollTop = top;
    else if (bottom > container.scrollTop + container.clientHeight)
      container.scrollTop = bottom - container.clientHeight;
  }

  $effect(() => {
    if (activeIndex >= 0 && activeIndex < items.length) scrollIntoView(activeIndex);
  });
</script>

<div class="rt-wrap" bind:this={container} onscroll={handleScroll}>
  <table class="rt">
    <colgroup>
      <col style="width: 36px" />
      <col style="width: 30%" />
      <col style="width: 38%" />
      <col style="width: 9%" />
      <col style="width: 7%" />
      <col style="width: 16%" />
    </colgroup>
    <thead>
      <tr>
        <th
          class="check"
          title={allSelected
            ? "Deselect all visible"
            : selectedVisible > 0
              ? `${selectedVisible} of ${items.length} selected`
              : "Select all visible"}
          onclick={(e) => e.stopPropagation()}
        >
          <input
            bind:this={headerCb}
            type="checkbox"
            class="cb"
            checked={allSelected}
            onclick={(e) => e.stopPropagation()}
            onchange={() => onSelectAll(!allSelected)}
          />
        </th>
        <th class="name" onclick={() => onSort("name")}
          >Name{indicator("name")}</th
        >
        <th class="path" onclick={() => onSort("path")}
          >Path{indicator("path")}</th
        >
        <th class="size num" onclick={() => onSort("size")}
          >Size{indicator("size")}</th
        >
        <th class="ext" onclick={() => onSort("ext")}
          >Type{indicator("ext")}</th
        >
        <th class="mtime num" onclick={() => onSort("mtime")}
          >Date Modified{indicator("mtime")}</th
        >
      </tr>
    </thead>
    <tbody>
      {#if topPad > 0}
        <tr style="height: {topPad}px"><td colspan="6"></td></tr>
      {/if}
      {#each visibleItems as item, i (visibleStart + i)}
        {@const idx = visibleStart + i}
        {@const sel = selectedPaths.has(item.path)}
        {@const ext = item.is_dir ? "Folder" : fileExtension(item.name) || "File"}
        <tr
          class="row"
          class:selected={sel}
          class:active={idx === activeIndex}
          class:dir={item.is_dir}
          ondblclick={() => onActivate(item)}
          oncontextmenu={(e) => {
            e.preventDefault();
            setActiveIndex(idx);
            if (!sel) onToggleSelect(item.path, false, false);
            onContext(e, item);
          }}
          onclick={(e) => onRowClick(e, idx, item)}
        >
          <td
            class="check"
            onclick={(e) => {
              // Whole cell stops propagation so clicking the padding
              // around the checkbox doesn't fall through to the row's
              // single-click handler (which would otherwise clear the
              // existing multi-selection).
              e.stopPropagation();
            }}
            ondblclick={(e) => e.stopPropagation()}
          >
            <input
              type="checkbox"
              class="cb"
              checked={sel}
              onclick={(e) => e.stopPropagation()}
              onchange={() => onToggleSelect(item.path, true, false)}
            />
          </td>
          <td class="name">
            <span class="ico" aria-hidden="true">
              {#if item.is_dir}
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
            <span class="name-text">{item.name}</span>
          </td>
          <td class="path" title={item.path}>{item.path}</td>
          <td class="size num">{item.is_dir ? "—" : formatSize(item.size)}</td>
          <td class="ext">{ext}</td>
          <td class="mtime num">{formatDate(item.mtime)}</td>
        </tr>
      {/each}
      {#if bottomPad > 0}
        <tr style="height: {bottomPad}px"><td colspan="6"></td></tr>
      {/if}
    </tbody>
  </table>
</div>

<style>
  .rt-wrap {
    flex: 1;
    min-height: 0;
    overflow: auto;
    background: var(--bg-card);
  }

  table.rt {
    width: 100%;
    border-collapse: collapse;
    table-layout: fixed;
    font-size: 12px;
  }

  thead {
    position: sticky;
    top: 0;
    z-index: 2;
  }

  th {
    background: var(--bg-surface);
    color: var(--text-muted);
    text-align: left;
    font-weight: 600;
    font-size: 11px;
    padding: 4px 8px;
    border-bottom: 1px solid var(--border);
    cursor: pointer;
    user-select: none;
    white-space: nowrap;
    text-transform: uppercase;
    letter-spacing: 0.4px;
  }
  th:hover {
    color: var(--text);
    background: var(--bg-surface-2);
  }
  th.num {
    text-align: right;
  }

  td {
    padding: 0 8px;
    height: 24px;
    line-height: 24px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-bottom: 1px solid var(--border-soft);
    /* Click on a row should select the row, not start a text selection
       drag. The Copy path / Copy name context menu actions still cover the
       "I want this string on my clipboard" use case. */
    user-select: none;
    -webkit-user-select: none;
  }

  td.num {
    text-align: right;
    font-variant-numeric: tabular-nums;
    color: var(--text-muted);
  }

  td.name {
    color: var(--text-strong);
  }
  .ico {
    margin-right: 6px;
    vertical-align: -1px;
  }

  td.path {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--text-muted);
  }

  td.ext {
    color: var(--text-muted);
    text-transform: uppercase;
    font-size: 10.5px;
    letter-spacing: 0.4px;
  }

  td.check,
  th.check {
    text-align: center;
    padding: 0 4px;
    cursor: default;
  }
  /* Visible checkbox against the dark theme. Larger than the GTK default
     so it's actually clickable and not lost in the row chrome — the user
     report was "no checkboxes visible". */
  input.cb {
    width: 16px;
    height: 16px;
    margin: 0;
    padding: 0;
    vertical-align: middle;
    cursor: pointer;
    accent-color: var(--accent);
    /* Faint outline so the unchecked box reads as a control even on the
       row's hover/selected backgrounds. */
    outline: 1px solid var(--border-strong);
    outline-offset: -1px;
    border-radius: 2px;
  }
  input.cb:checked {
    outline-color: var(--accent);
  }

  tr.row {
    cursor: default;
  }
  tr.row:hover td {
    background: var(--bg-hover);
  }
  tr.row.selected td {
    background: rgba(47, 129, 247, 0.14);
  }
  tr.row.active td {
    box-shadow: inset 2px 0 0 var(--accent);
  }
  tr.row.selected.active td {
    background: rgba(47, 129, 247, 0.22);
  }

  input[type="checkbox"] {
    cursor: pointer;
    accent-color: var(--accent);
  }
</style>
