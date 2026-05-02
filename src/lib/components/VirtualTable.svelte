<script lang="ts">
  import type { FileItem } from "../ipc";

  interface Props {
    items: FileItem[];
    total: number;
    sortField: string;
    sortAsc: boolean;
    onSort: (field: string) => void;
    onLoadMore: () => void;
    selectable?: boolean;
    selectedPaths?: Set<string>;
    onToggleSelect?: (path: string) => void;
    onSelectAll?: (selectAll: boolean) => void;
  }

  let {
    items,
    total,
    sortField,
    sortAsc,
    onSort,
    onLoadMore,
    selectable = false,
    selectedPaths = new Set<string>(),
    onToggleSelect,
    onSelectAll,
  }: Props = $props();

  const ROW_HEIGHT = 28;
  const BUFFER = 5;

  let container: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let containerHeight = $state(600);

  let allSelected = $derived(
    selectable && items.length > 0 && items.every((i) => selectedPaths.has(i.path)),
  );

  function handleScroll() {
    if (!container) return;
    scrollTop = container.scrollTop;
    const distFromBottom =
      container.scrollHeight - container.scrollTop - container.clientHeight;
    if (distFromBottom < 200 && items.length < total) {
      onLoadMore();
    }
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
  let totalHeight = $derived(items.length * ROW_HEIGHT);

  function sortIndicator(field: string): string {
    if (field !== sortField) return "";
    return sortAsc ? " ▲" : " ▼";
  }

  function formatSize(bytes: number): string {
    if (bytes === 0) return "—";
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1073741824) return `${(bytes / 1048576).toFixed(1)} MB`;
    return `${(bytes / 1073741824).toFixed(1)} GB`;
  }

  function formatDate(epoch: number): string {
    if (epoch === 0) return "—";
    return new Date(epoch * 1000).toLocaleDateString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  }

  $effect(() => {
    if (container) {
      const obs = new ResizeObserver((entries) => {
        containerHeight = entries[0].contentRect.height;
      });
      obs.observe(container);
      return () => obs.disconnect();
    }
  });
</script>

<div class="table-wrapper" bind:this={container} onscroll={handleScroll}>
  <table>
    <thead>
      <tr>
        {#if selectable}
          <th class="col-check">
            <input
              type="checkbox"
              checked={allSelected}
              onchange={() => onSelectAll?.(!allSelected)}
            />
          </th>
        {/if}
        <th class="col-name" onclick={() => onSort("name")}
          >Name{sortIndicator("name")}</th
        >
        <th class="col-path" onclick={() => onSort("path")}
          >Path{sortIndicator("path")}</th
        >
        <th class="col-size" onclick={() => onSort("size")}
          >Size{sortIndicator("size")}</th
        >
        <th class="col-mtime" onclick={() => onSort("mtime")}
          >Modified{sortIndicator("mtime")}</th
        >
      </tr>
    </thead>
    <tbody>
      <tr style="height: {topPad}px"
        ><td colspan={selectable ? 5 : 4}></td></tr
      >
      {#each visibleItems as item, i (visibleStart + i)}
        <tr>
          {#if selectable}
            <td class="col-check">
              <input
                type="checkbox"
                checked={selectedPaths.has(item.path)}
                onchange={() => onToggleSelect?.(item.path)}
              />
            </td>
          {/if}
          <td class="col-name">
            <span class="icon">{item.is_dir ? "📁" : "📄"}</span>
            {item.name}
          </td>
          <td class="col-path" title={item.path}>{item.path}</td>
          <td class="col-size">{formatSize(item.size)}</td>
          <td class="col-mtime">{formatDate(item.mtime)}</td>
        </tr>
      {/each}
      <tr
        style="height: {Math.max(0, totalHeight - visibleEnd * ROW_HEIGHT)}px"
        ><td colspan={selectable ? 5 : 4}></td></tr
      >
    </tbody>
  </table>
</div>

<style>
  .table-wrapper {
    flex: 1;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: 4px;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    table-layout: fixed;
  }

  thead {
    position: sticky;
    top: 0;
    z-index: 1;
  }

  th {
    background: var(--bg-surface);
    padding: 6px 8px;
    text-align: left;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-muted);
    cursor: pointer;
    user-select: none;
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
  }

  th:hover {
    color: var(--text);
  }

  td {
    padding: 4px 8px;
    font-size: 13px;
    height: 28px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-bottom: 1px solid var(--border);
  }

  tr:hover td {
    background: var(--bg-hover);
  }

  .col-check {
    width: 36px;
    text-align: center;
    cursor: default;
  }

  .col-check input[type="checkbox"] {
    cursor: pointer;
  }

  .col-name {
    width: 28%;
  }
  .col-path {
    width: 38%;
    direction: rtl;
    text-align: left;
  }
  .col-size {
    width: 14%;
    text-align: right;
  }
  .col-mtime {
    width: 14%;
    text-align: right;
  }

  .icon {
    margin-right: 4px;
  }
</style>
