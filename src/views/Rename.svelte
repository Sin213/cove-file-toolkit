<script lang="ts">
  import {
    previewRename,
    applyRename,
    undoRename,
    type RenameRule,
    type RenamePreviewItem,
  } from "../lib/ipc";
  import {
    isRenameChecked,
    toggleRenameChecked,
    setAllRenameChecked,
    setRenameFiles,
  } from "../lib/selection.svelte";

  interface Props {
    files: Array<{ name: string; path: string }>;
  }

  let { files = $bindable() }: Props = $props();

  // Local mutable queue (so we can remove items)
  let queue = $state<Array<{ name: string; path: string }>>([]);
  $effect(() => {
    queue = [...files];
    setRenameFiles(files);
  });

  // ----- Rule states -----
  // Replace
  let replaceFrom = $state("");
  let replaceTo = $state("");
  let replaceCS = $state(false);
  let replaceStem = $state(true);

  // Remove (substring)
  let removeText = $state("");
  let removeCS = $state(false);
  let removeStem = $state(true);

  // Remove range (chars 1..n)
  let removeStart = $state("");
  let removeEnd = $state("");

  // Trim Ends
  let trimFirst = $state(0);
  let trimLast = $state(0);

  // Add (prefix/suffix)
  let prefixText = $state("");
  let suffixText = $state("");

  // Case
  let caseMode = $state("");
  let extCaseMode = $state("");

  // Numbering
  let numMode = $state("");
  let numStart = $state(1);
  let numStep = $state(1);
  let numPad = $state(3);
  let numSep = $state("_");

  // Extension
  let extNew = $state("");

  // RegEx
  let regexPattern = $state("");
  let regexReplacement = $state("");
  let regexStem = $state(false);

  // Filters (preview-side only — exclude items from the queue display)
  let filterContains = $state("");
  let filterExt = $state("");

  let previews = $state<RenamePreviewItem[]>([]);
  let applying = $state(false);
  let canUndo = $state(false);
  let statusMessage = $state("");
  let previewTimer: ReturnType<typeof setTimeout> | null = null;

  let rules = $derived.by(() => {
    const r: RenameRule[] = [];
    if (regexPattern)
      r.push({ type: "regex_replace", pattern: regexPattern, replacement: regexReplacement, stem_only: regexStem });
    if (replaceFrom)
      r.push({ type: "replace", from: replaceFrom, to: replaceTo, case_sensitive: replaceCS, stem_only: replaceStem });
    if (removeText)
      r.push({ type: "remove", text: removeText, case_sensitive: removeCS, stem_only: removeStem });
    const rs = parseInt(removeStart, 10);
    const re = parseInt(removeEnd, 10);
    if (!isNaN(rs) && !isNaN(re)) r.push({ type: "remove_range", start: rs, end: re });
    if (trimFirst > 0 || trimLast > 0) r.push({ type: "remove_ends", first: trimFirst, last: trimLast });
    if (prefixText) r.push({ type: "prefix", text: prefixText });
    if (suffixText) r.push({ type: "suffix", text: suffixText });
    if (caseMode) r.push({ type: "case_change", mode: caseMode });
    if (extCaseMode) r.push({ type: "ext_case", mode: extCaseMode });
    if (numMode)
      r.push({
        type: "numbering",
        start: numStart,
        step: numStep,
        padding: numPad,
        position: numMode,
        separator: numSep,
      });
    if (extNew) r.push({ type: "ext_change", new_ext: extNew });
    return r;
  });

  let visibleQueue = $derived.by(() => {
    return queue.filter((q) => {
      if (filterContains && !q.name.toLowerCase().includes(filterContains.toLowerCase()))
        return false;
      if (filterExt) {
        const wanted = filterExt
          .split(",")
          .map((e) => e.trim().replace(/^\./, "").toLowerCase())
          .filter(Boolean);
        const ext = (q.name.split(".").pop() || "").toLowerCase();
        if (wanted.length && !wanted.includes(ext)) return false;
      }
      return true;
    });
  });

  let activeQueue = $derived.by(() => {
    return visibleQueue.filter((q) => isRenameChecked(q.path));
  });

  let allChecked = $derived(queue.length > 0 && queue.every((q) => isRenameChecked(q.path)));
  let noneChecked = $derived(queue.length === 0 || queue.every((q) => !isRenameChecked(q.path)));

  let okCount = $derived(previews.filter((p) => p.status === "ok").length);
  let errCount = $derived(
    previews.filter((p) => p.status === "error" || p.status === "conflict").length,
  );
  let unchangedCount = $derived(previews.filter((p) => p.status === "unchanged").length);
  let canApply = $derived(okCount > 0 && errCount === 0 && !applying);

  function schedulePreview() {
    if (previewTimer) clearTimeout(previewTimer);
    previewTimer = setTimeout(() => doPreview(), 120);
  }

  async function doPreview() {
    if (activeQueue.length === 0 || rules.length === 0) {
      previews = [];
      return;
    }
    try {
      previews = await previewRename(activeQueue.map((q) => q.path), rules);
    } catch (e) {
      console.error("preview error:", e);
    }
  }

  $effect(() => {
    activeQueue;
    rules;
    schedulePreview();
  });

  async function handleApply() {
    if (!canApply) return;
    applying = true;
    statusMessage = "";
    try {
      const count = await applyRename(activeQueue.map((q) => q.path), rules);
      statusMessage = `Renamed ${count} file${count === 1 ? "" : "s"}.`;
      canUndo = true;
      // After rename, paths are stale — clear queue
      queue = [];
      previews = [];
      resetRules();
    } catch (e) {
      statusMessage = `Error: ${e}`;
    } finally {
      applying = false;
    }
  }

  async function handleUndo() {
    statusMessage = "";
    try {
      const n = await undoRename();
      statusMessage = `Undid ${n} rename${n === 1 ? "" : "s"}.`;
      canUndo = false;
    } catch (e) {
      statusMessage = `Undo error: ${e}`;
    }
  }

  function resetRules() {
    replaceFrom = ""; replaceTo = ""; replaceCS = false; replaceStem = true;
    removeText = ""; removeCS = false; removeStem = true;
    removeStart = ""; removeEnd = "";
    trimFirst = 0; trimLast = 0;
    prefixText = ""; suffixText = "";
    caseMode = ""; extCaseMode = "";
    numMode = ""; numStart = 1; numStep = 1; numPad = 3; numSep = "_";
    extNew = "";
    regexPattern = ""; regexReplacement = ""; regexStem = false;
  }

  function clearQueue() {
    queue = [];
    previews = [];
    setAllRenameChecked(false);
  }

  function removeFromQueue(path: string) {
    queue = queue.filter((q) => q.path !== path);
  }

  function previewFor(path: string): RenamePreviewItem | undefined {
    return previews.find((p) => p.original_path === path);
  }

  // keyboard
  function onKey(e: KeyboardEvent) {
    if (e.key === "Delete") {
      // remove selected previews
      // Currently no selection model — skip
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="rn-view">
  {#if queue.length === 0}
    <div class="empty">
      <div class="empty-title">Rename queue is empty</div>
      <div class="empty-hint">
        Send files here from <strong>Search</strong> or <strong>Disk Usage</strong>
        using the <em>Send to Rename</em> action.
      </div>
      {#if canUndo}
        <!-- Undo must remain reachable after a successful apply, otherwise
             the queue clears and the action becomes hidden. -->
        <div class="empty-undo">
          <button class="btn ghost sm" onclick={handleUndo}>Undo last rename</button>
          {#if statusMessage}
            <span
              class="status-msg"
              class:err={statusMessage.startsWith("Error") ||
                statusMessage.startsWith("Undo error")}
              >{statusMessage}</span
            >
          {/if}
        </div>
      {:else if statusMessage}
        <span
          class="status-msg"
          class:err={statusMessage.startsWith("Error") ||
            statusMessage.startsWith("Undo error")}
          >{statusMessage}</span
        >
      {/if}
    </div>
  {:else}
    <!-- Rule grid -->
    <div class="rule-grid">
      <!-- RegEx -->
      <section class="panel" class:on={!!regexPattern}>
        <header class="ph">
          <span class="num">1</span>
          <span class="title">RegEx</span>
          <label class="ph-opt"
            ><input type="checkbox" bind:checked={regexStem} /> Stem only</label
          >
        </header>
        <div class="row">
          <input class="mono" type="text" bind:value={regexPattern} placeholder="Pattern" />
        </div>
        <div class="row">
          <input
            class="mono"
            type="text"
            bind:value={regexReplacement}
            placeholder="Replacement"
          />
        </div>
      </section>

      <!-- Name (Replace) -->
      <section class="panel" class:on={!!replaceFrom}>
        <header class="ph">
          <span class="num">2</span>
          <span class="title">Replace</span>
          <label class="ph-opt"
            ><input type="checkbox" bind:checked={replaceCS} /> Case</label
          >
          <label class="ph-opt"
            ><input type="checkbox" bind:checked={replaceStem} /> Stem only</label
          >
        </header>
        <div class="row">
          <input type="text" bind:value={replaceFrom} placeholder="From" />
        </div>
        <div class="row">
          <input type="text" bind:value={replaceTo} placeholder="To" />
        </div>
      </section>

      <!-- Remove -->
      <section class="panel" class:on={!!removeText || removeStart || removeEnd}>
        <header class="ph">
          <span class="num">3</span>
          <span class="title">Remove</span>
          <label class="ph-opt"
            ><input type="checkbox" bind:checked={removeCS} /> Case</label
          >
          <label class="ph-opt"
            ><input type="checkbox" bind:checked={removeStem} /> Stem only</label
          >
        </header>
        <div class="row">
          <input type="text" bind:value={removeText} placeholder="Text to remove" />
        </div>
        <div class="row split">
          <label class="inl"
            ><span>Range from</span
            ><input type="number" bind:value={removeStart} min="-100" max="100" /></label
          >
          <label class="inl"
            ><span>to</span
            ><input type="number" bind:value={removeEnd} min="-100" max="100" /></label
          >
        </div>
        <span class="range-hint">Negative values count from end. Range is [start, end) — end index is exclusive.</span>
      </section>

      <!-- Trim Ends -->
      <section class="panel" class:on={trimFirst > 0 || trimLast > 0}>
        <header class="ph">
          <span class="title">Trim Ends</span>
        </header>
        <div class="row split">
          <label class="inl"
            ><span>First N</span
            ><input type="number" bind:value={trimFirst} min="0" /></label
          >
          <label class="inl"
            ><span>Last N</span
            ><input type="number" bind:value={trimLast} min="0" /></label
          >
        </div>
      </section>

      <!-- Add -->
      <section class="panel" class:on={!!prefixText || !!suffixText}>
        <header class="ph">
          <span class="num">4</span>
          <span class="title">Add</span>
        </header>
        <div class="row">
          <input type="text" bind:value={prefixText} placeholder="Prefix" />
        </div>
        <div class="row">
          <input type="text" bind:value={suffixText} placeholder="Suffix" />
        </div>
      </section>

      <!-- Case -->
      <section class="panel" class:on={!!caseMode || !!extCaseMode}>
        <header class="ph">
          <span class="num">5</span>
          <span class="title">Case</span>
        </header>
        <div class="row split">
          <label class="inl">
            <span>Name</span>
            <select bind:value={caseMode}>
              <option value="">— none —</option>
              <option value="lower">lowercase</option>
              <option value="upper">UPPERCASE</option>
              <option value="title">Title Case</option>
              <option value="sentence">Sentence case</option>
            </select>
          </label>
          <label class="inl">
            <span>Ext</span>
            <select bind:value={extCaseMode}>
              <option value="">—</option>
              <option value="lower">.ext</option>
              <option value="upper">.EXT</option>
            </select>
          </label>
        </div>
      </section>

      <!-- Numbering -->
      <section class="panel" class:on={!!numMode}>
        <header class="ph">
          <span class="num">6</span>
          <span class="title">Numbering</span>
        </header>
        <div class="row split">
          <label class="inl"
            ><span>Position</span>
            <select bind:value={numMode}>
              <option value="">— none —</option>
              <option value="prefix">Prefix</option>
              <option value="suffix">Suffix</option>
            </select></label
          >
          <label class="inl"
            ><span>Sep</span><input
              type="text"
              bind:value={numSep}
              maxlength="3"
              style="width: 32px"
            /></label
          >
        </div>
        <div class="row split">
          <label class="inl"
            ><span>Start</span><input type="number" bind:value={numStart} min="0" /></label
          >
          <label class="inl"
            ><span>Step</span><input type="number" bind:value={numStep} min="1" /></label
          >
          <label class="inl"
            ><span>Pad</span><input
              type="number"
              bind:value={numPad}
              min="1"
              max="10"
            /></label
          >
        </div>
      </section>

      <!-- Extension -->
      <section class="panel" class:on={!!extNew}>
        <header class="ph">
          <span class="num">7</span>
          <span class="title">Extension</span>
        </header>
        <div class="row">
          <input type="text" bind:value={extNew} placeholder="New ext (e.g. txt)" />
        </div>
      </section>

      <!-- Filters -->
      <section class="panel" class:on={!!filterContains || !!filterExt}>
        <header class="ph">
          <span class="num">8</span>
          <span class="title">Filters</span>
        </header>
        <div class="row">
          <input
            type="text"
            bind:value={filterContains}
            placeholder="Name contains…"
          />
        </div>
        <div class="row">
          <input
            type="text"
            bind:value={filterExt}
            placeholder="Only ext (e.g. jpg,png)"
          />
        </div>
      </section>
    </div>

    <!-- Action bar -->
    <div class="action-bar">
      <span class="qstat">
        <span class="strong">{activeQueue.length}</span> in queue
        {#if queue.length !== visibleQueue.length || visibleQueue.length !== activeQueue.length}
          <span class="muted">({queue.length - activeQueue.length} excluded)</span>
        {/if}
        <span class="sep">|</span>
        <span class="ok">{okCount} ready</span>
        <span class="sep">|</span>
        <span class="warn">{unchangedCount} unchanged</span>
        <span class="sep">|</span>
        <span class="err" class:dim={errCount === 0}>{errCount} errors</span>
      </span>

      <button class="btn ghost sm" onclick={resetRules}>Reset rules</button>
      <button class="btn ghost sm" onclick={clearQueue}>Clear queue</button>
      {#if canUndo}
        <button class="btn ghost sm" onclick={handleUndo}>Undo last rename</button>
      {/if}
      <button class="btn primary" disabled={!canApply} onclick={handleApply}>
        {applying ? "Applying…" : `Apply ${okCount}`}
      </button>
      {#if statusMessage}
        <span class="status-msg" class:err={statusMessage.startsWith("Error")}
          >{statusMessage}</span
        >
      {/if}
    </div>

    <!-- Preview -->
    <div class="preview-wrap">
      <table class="prev">
        <colgroup>
          <col style="width: 28px" />
          <col style="width: 32px" />
          <col style="width: 34%" />
          <col style="width: 34%" />
          <col style="width: 18%" />
          <col style="width: 8%" />
        </colgroup>
        <thead>
          <tr>
            <th class="chk-col">
              <input
                type="checkbox"
                checked={allChecked}
                indeterminate={!allChecked && !noneChecked}
                onchange={() => setAllRenameChecked(!allChecked)}
              />
            </th>
            <th></th>
            <th>Original Name</th>
            <th>New Name</th>
            <th>Status</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each visibleQueue as q (q.path)}
            {@const checked = isRenameChecked(q.path)}
            {@const p = checked ? previewFor(q.path) : undefined}
            {@const status = p?.status ?? "—"}
            <tr
              class:row-ok={status === "ok"}
              class:row-bad={status === "error" || status === "conflict"}
              class:row-skip={status === "unchanged"}
              class:row-unchecked={!checked}
            >
              <td class="chk-col">
                <input
                  type="checkbox"
                  checked={checked}
                  onchange={() => toggleRenameChecked(q.path)}
                />
              </td>
              <td class="num">
                <span class="dot dot-{status}" title={status}></span>
              </td>
              <td class="orig mono" title={q.path}>{q.name}</td>
              <td class="new mono" title={p?.new_path ?? ""}>
                {p?.new_name ?? q.name}
              </td>
              <td class="status">
                <span class="pill pill-{status}">{status}</span>
                {#if p?.message}
                  <span class="msg" title={p.message}>{p.message}</span>
                {/if}
              </td>
              <td class="num">
                <button
                  class="rm"
                  title="Remove from queue"
                  onclick={() => removeFromQueue(q.path)}>×</button
                >
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<style>
  .rn-view {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    background: var(--bg);
  }

  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    text-align: center;
    color: var(--text-muted);
    gap: 8px;
    padding: 30px;
  }
  .empty-title {
    color: var(--text);
    font-weight: 600;
    font-size: 15px;
  }
  .empty-hint {
    font-size: 12.5px;
    line-height: 1.6;
  }
  .empty-undo {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    margin-top: 14px;
  }

  .rule-grid {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 6px;
    padding: 8px 10px;
    background: var(--bg-2);
    border-bottom: 1px solid var(--border);
  }
  @media (max-width: 1100px) {
    .rule-grid {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }

  .panel {
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 6px 8px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    transition: border-color 0.12s, background 0.12s;
  }
  .panel.on {
    border-color: var(--accent);
    background: var(--bg-surface-2);
  }
  .ph {
    display: flex;
    align-items: center;
    gap: 6px;
    padding-bottom: 2px;
  }
  .ph .num {
    width: 16px;
    height: 16px;
    border-radius: 3px;
    background: var(--bg);
    color: var(--text-faint);
    font-family: var(--mono);
    font-size: 10px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-weight: 600;
  }
  .panel.on .ph .num {
    background: var(--accent);
    color: #fff;
  }
  .ph .title {
    color: var(--text-muted);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    font-weight: 700;
    flex: 1;
  }
  .panel.on .ph .title {
    color: var(--accent-strong);
  }
  .ph-opt {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    color: var(--text-muted);
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    cursor: pointer;
  }
  .ph-opt input[type="checkbox"] {
    accent-color: var(--accent);
    cursor: pointer;
  }

  .row {
    display: flex;
    gap: 4px;
  }
  .row.split {
    gap: 6px;
  }
  .row > input,
  .row .inl input,
  .row .inl select {
    width: 100%;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 4px 6px;
    color: var(--text);
    font-size: 12px;
  }
  .row > input.mono {
    font-family: var(--mono);
    font-size: 11.5px;
  }
  .row > input:focus,
  .row .inl input:focus,
  .row .inl select:focus {
    outline: none;
    border-color: var(--accent);
  }
  /* Native dropdown popup readability — without these, Linux/WebKit can
     render light text on a default light popup background. */
  .row .inl select option {
    background-color: var(--bg-surface);
    color: var(--text-strong);
  }
  .row .inl select option:checked,
  .row .inl select option:hover {
    background-color: var(--bg-active);
    color: var(--text-strong);
  }
  .inl {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 10px;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.4px;
    flex: 1;
  }

  .action-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    background: var(--bg-2);
    border-bottom: 1px solid var(--border);
  }
  .qstat {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--text-muted);
    font-size: 12px;
    margin-right: auto;
  }
  .qstat .strong {
    color: var(--text-strong);
    font-weight: 600;
  }
  .qstat .ok {
    color: var(--success);
    font-weight: 600;
  }
  .qstat .warn {
    color: var(--text-muted);
  }
  .qstat .err {
    color: var(--danger);
    font-weight: 600;
  }
  .qstat .err.dim {
    color: var(--text-faint);
    font-weight: normal;
  }
  .qstat .sep {
    color: var(--border-strong);
  }
  .qstat .muted {
    color: var(--text-faint);
    font-size: 11px;
  }

  .btn {
    height: 28px;
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
    height: 26px;
    padding: 0 10px;
  }
  .btn.primary {
    background: var(--accent);
    color: #fff;
  }
  .btn.primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  .btn.primary:disabled {
    opacity: 0.4;
    cursor: not-allowed;
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

  .status-msg {
    color: var(--success);
    font-size: 12px;
    font-weight: 500;
  }
  .status-msg.err {
    color: var(--danger);
  }

  .preview-wrap {
    flex: 1;
    overflow: auto;
    background: var(--bg-card);
  }
  table.prev {
    width: 100%;
    border-collapse: collapse;
    table-layout: fixed;
    font-size: 12px;
  }
  table.prev thead {
    position: sticky;
    top: 0;
    z-index: 2;
  }
  table.prev th {
    background: var(--bg-surface);
    color: var(--text-muted);
    font-weight: 600;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    text-align: left;
    padding: 5px 8px;
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
  }
  table.prev td {
    padding: 0 8px;
    height: 24px;
    line-height: 24px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-bottom: 1px solid var(--border-soft);
    color: var(--text);
  }
  td.mono {
    font-family: var(--mono);
    font-size: 11.5px;
  }
  td.orig {
    color: var(--text-muted);
  }
  tr.row-ok td.new {
    color: var(--accent-strong);
  }
  tr.row-bad td {
    background: rgba(248, 81, 73, 0.08);
  }
  tr.row-bad td.new {
    color: var(--danger);
  }
  tr.row-skip td.new {
    color: var(--text-muted);
  }

  .num {
    text-align: center;
  }
  .dot {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--text-faint);
  }
  .dot-ok {
    background: var(--success);
    box-shadow: 0 0 4px rgba(63, 185, 80, 0.5);
  }
  .dot-error,
  .dot-conflict {
    background: var(--danger);
    box-shadow: 0 0 4px rgba(248, 81, 73, 0.5);
  }
  .dot-unchanged {
    background: var(--text-faint);
  }

  .pill {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    font-weight: 600;
    padding: 1px 6px;
    border-radius: 3px;
    border: 1px solid;
  }
  .pill-ok {
    color: var(--success);
    border-color: var(--success);
    background: rgba(63, 185, 80, 0.1);
  }
  .pill-error,
  .pill-conflict {
    color: var(--danger);
    border-color: var(--danger);
    background: rgba(248, 81, 73, 0.1);
  }
  .pill-unchanged {
    color: var(--text-muted);
    border-color: var(--border);
    background: var(--bg-surface);
  }

  .msg {
    margin-left: 6px;
    color: var(--text-muted);
    font-size: 11px;
  }

  .rm {
    color: var(--text-faint);
    font-size: 14px;
    line-height: 1;
    padding: 0 4px;
    border-radius: 3px;
  }
  .rm:hover {
    color: var(--danger);
    background: rgba(248, 81, 73, 0.12);
  }

  .chk-col {
    text-align: center;
    width: 28px;
  }
  .chk-col input[type="checkbox"] {
    accent-color: var(--accent);
    cursor: pointer;
  }
  tr.row-unchecked td {
    color: var(--text-faint);
  }

  .range-hint {
    color: var(--text-faint);
    font-size: 11px;
  }
</style>
