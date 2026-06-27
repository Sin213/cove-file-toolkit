<script lang="ts">
  import type { DiskUsageEntry } from "../ipc";
  import { formatSize } from "../ipc";

  interface Props {
    items: DiskUsageEntry[];
    onSelect?: (entry: DiskUsageEntry) => void;
    onDrill?: (entry: DiskUsageEntry) => void;
    selectedPath?: string | null;
    height?: number;
  }

  let {
    items,
    onSelect,
    onDrill,
    selectedPath = null,
    height = 200,
  }: Props = $props();

  let containerEl: HTMLDivElement | undefined = $state();
  let cw = $state(800);
  let ch = $state(height);

  // Color palette inspired by WizTree's vivid blocks.
  const PALETTE = [
    "#e94e4e", // red
    "#e67e22", // orange
    "#f1c40f", // yellow
    "#2ecc71", // green
    "#1abc9c", // teal
    "#3498db", // blue
    "#5b6df0", // indigo
    "#9b59b6", // purple
    "#e056b5", // pink
    "#c0392b", // dark red
    "#d35400", // dark orange
    "#16a085", // dark teal
    "#27ae60", // dark green
    "#2980b9", // dark blue
    "#8e44ad", // dark purple
    "#cd6155",
    "#dc7633",
    "#f4d03f",
    "#52be80",
    "#5dade2",
    "#a569bd",
  ];

  function colorFor(name: string, index: number, isDir: boolean): string {
    let hash = 0;
    for (let i = 0; i < name.length; i++) {
      hash = (hash * 31 + name.charCodeAt(i)) >>> 0;
    }
    const idx = (hash + index) % PALETTE.length;
    let c = PALETTE[idx];
    if (!isDir) {
      // tint files slightly darker so dirs read as dominant
      c = c + "cc";
    }
    return c;
  }

  // Squarified treemap algorithm (Bruls et al.)
  type Rect = { x: number; y: number; w: number; h: number };
  type Tile = { entry: DiskUsageEntry; rect: Rect; color: string };

  function squarify(
    values: number[],
    rect: Rect,
  ): Rect[] {
    const out: Rect[] = new Array(values.length);
    if (values.length === 0) return out;

    const total = values.reduce((a, b) => a + b, 0);
    if (total <= 0) {
      // fallback: equal split
      const w = rect.w / values.length;
      values.forEach((_v, i) => {
        out[i] = { x: rect.x + i * w, y: rect.y, w, h: rect.h };
      });
      return out;
    }

    // scale values to area
    const area = rect.w * rect.h;
    const scaled = values.map((v) => (v / total) * area);
    const indices = scaled.map((_, i) => i);
    let cur = { ...rect };
    let remaining = indices.slice();

    while (remaining.length > 0) {
      const row: number[] = [];
      const rowIdx: number[] = [];
      const sideShort = Math.min(cur.w, cur.h);
      let bestRatio = Infinity;

      let i = 0;
      while (i < remaining.length) {
        const candidate = [...row, scaled[remaining[i]]];
        const ratio = worstRatio(candidate, sideShort);
        if (ratio <= bestRatio || row.length === 0) {
          row.push(scaled[remaining[i]]);
          rowIdx.push(remaining[i]);
          bestRatio = ratio;
          i++;
        } else {
          break;
        }
      }

      // Place row in current rect
      const rowSum = row.reduce((a, b) => a + b, 0);
      if (cur.w >= cur.h) {
        // place as a column on the left
        const colW = rowSum / cur.h;
        let yOff = 0;
        for (let k = 0; k < row.length; k++) {
          const segH = row[k] / colW;
          out[rowIdx[k]] = {
            x: cur.x,
            y: cur.y + yOff,
            w: colW,
            h: segH,
          };
          yOff += segH;
        }
        cur = { x: cur.x + colW, y: cur.y, w: cur.w - colW, h: cur.h };
      } else {
        // place as a row at the top
        const rowH = rowSum / cur.w;
        let xOff = 0;
        for (let k = 0; k < row.length; k++) {
          const segW = row[k] / rowH;
          out[rowIdx[k]] = {
            x: cur.x + xOff,
            y: cur.y,
            w: segW,
            h: rowH,
          };
          xOff += segW;
        }
        cur = { x: cur.x, y: cur.y + rowH, w: cur.w, h: cur.h - rowH };
      }

      remaining = remaining.slice(row.length);
    }

    return out;
  }

  function worstRatio(row: number[], side: number): number {
    if (row.length === 0) return Infinity;
    const sum = row.reduce((a, b) => a + b, 0);
    let max = -Infinity;
    let min = Infinity;
    for (const v of row) {
      if (v > max) max = v;
      if (v < min) min = v;
    }
    const sideSq = side * side;
    const sumSq = sum * sum;
    return Math.max((sideSq * max) / sumSq, sumSq / (sideSq * min));
  }

  // Take items, filter out 0-size, take top N for clarity
  let tiles = $derived.by((): Tile[] => {
    if (!items || items.length === 0 || cw < 4 || ch < 4) return [];
    const filtered = items
      .filter((e) => e.size > 0)
      .sort((a, b) => b.size - a.size)
      .slice(0, 80);
    if (filtered.length === 0) return [];
    const values = filtered.map((e) => e.size);
    const rects = squarify(values, { x: 0, y: 0, w: cw, h: ch });
    return filtered.map((entry, i) => ({
      entry,
      rect: rects[i] || { x: 0, y: 0, w: 0, h: 0 },
      color: colorFor(entry.name, i, entry.is_dir),
    }));
  });

  function onResize() {
    if (!containerEl) return;
    cw = containerEl.clientWidth;
    ch = containerEl.clientHeight;
  }

  $effect(() => {
    if (!containerEl) return;
    const ro = new ResizeObserver(() => onResize());
    ro.observe(containerEl);
    onResize();
    return () => ro.disconnect();
  });
</script>

<div class="treemap-wrap" bind:this={containerEl} style:height="{height}px">
  {#each tiles as tile (tile.entry.path)}
    {@const r = tile.rect}
    {@const showText = r.w >= 40 && r.h >= 15}
    {@const showSize = r.w >= 64 && r.h >= 30}
    <button
      class="tm-tile"
      class:selected={selectedPath === tile.entry.path}
      style:left="{r.x}px"
      style:top="{r.y}px"
      style:width="{Math.max(0, r.w - 1)}px"
      style:height="{Math.max(0, r.h - 1)}px"
      style:background={tile.color}
      title={`${tile.entry.path}\n${formatSize(tile.entry.size)}`}
      onclick={() => onSelect?.(tile.entry)}
      ondblclick={() => onDrill?.(tile.entry)}
    >
      {#if showText}
        <div class="tm-name">{tile.entry.name}</div>
      {/if}
      {#if showSize}
        <div class="tm-size">{formatSize(tile.entry.size)}</div>
      {/if}
    </button>
  {/each}
  {#if tiles.length === 0}
    <div class="tm-empty">No data to display.</div>
  {/if}
</div>

<style>
  .treemap-wrap {
    position: relative;
    width: 100%;
    background: #0a0d12;
    border-top: 1px solid var(--border);
    overflow: hidden;
  }
  .tm-tile {
    position: absolute;
    overflow: hidden;
    border: 1px solid rgba(0, 0, 0, 0.45);
    color: rgba(20, 20, 20, 0.92);
    text-align: left;
    padding: 2px 4px;
    cursor: pointer;
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.18),
      inset 0 -1px 0 rgba(0, 0, 0, 0.3);
    transition: transform 0.06s ease, filter 0.06s ease;
    line-height: 1.15;
  }
  .tm-tile:hover {
    filter: brightness(1.15) saturate(1.05);
    z-index: 2;
  }
  .tm-tile.selected {
    border-color: #fff;
    box-shadow: inset 0 0 0 1px #fff,
      inset 0 1px 0 rgba(255, 255, 255, 0.18),
      inset 0 -1px 0 rgba(0, 0, 0, 0.3);
    z-index: 3;
  }
  .tm-name {
    font-size: 10.5px;
    font-weight: 700;
    color: rgba(15, 15, 15, 0.95);
    text-shadow: 0 1px 0 rgba(255, 255, 255, 0.18);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tm-size {
    font-size: 10px;
    font-family: var(--mono);
    color: rgba(15, 15, 15, 0.78);
    margin-top: 1px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tm-empty {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-faint);
    font-size: 12px;
  }
</style>
