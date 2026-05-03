<script lang="ts">
  import { onMount } from "svelte";
  import {
    getSettings,
    saveSettings,
    getCacheInfo,
    clearCache,
    formatSize,
    formatDate,
    type AppSettings,
    type CacheInfo,
  } from "../ipc";
  import { resetIndexAfterCacheClear } from "../scanStore.svelte";

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();

  let settings = $state<AppSettings | null>(null);
  let cacheInfo = $state<CacheInfo | null>(null);
  let newRoot = $state("");
  let newPattern = $state("");
  let saving = $state(false);
  let error = $state("");
  let success = $state("");

  async function load() {
    try {
      settings = await getSettings();
      cacheInfo = await getCacheInfo();
    } catch (e) {
      error = `Failed to load settings: ${e}`;
    }
  }

  async function handleSave() {
    if (!settings) return;
    saving = true;
    error = "";
    success = "";
    try {
      await saveSettings(settings);
      success = "Settings saved.";
      setTimeout(() => (success = ""), 1800);
    } catch (e) {
      error = `Save failed: ${e}`;
    } finally {
      saving = false;
    }
  }

  function addRoot() {
    if (!settings || !newRoot.trim()) return;
    const r = newRoot.trim();
    if (!settings.indexed_roots.some((root) => root.path === r)) {
      const id =
        (typeof crypto !== "undefined" && "randomUUID" in crypto)
          ? crypto.randomUUID()
          : `r-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const display = r.split(/[/\\]/).filter(Boolean).pop() || r;
      settings.indexed_roots = [
        ...settings.indexed_roots,
        { id, path: r, display_name: display, enabled: true },
      ];
    }
    newRoot = "";
  }
  function removeRoot(i: number) {
    if (!settings) return;
    settings.indexed_roots = settings.indexed_roots.filter((_, idx) => idx !== i);
  }
  function toggleRootEnabled(i: number) {
    if (!settings) return;
    settings.indexed_roots = settings.indexed_roots.map((r, idx) =>
      idx === i ? { ...r, enabled: !r.enabled } : r,
    );
  }

  function addPattern() {
    if (!settings || !newPattern.trim()) return;
    const p = newPattern.trim();
    if (!settings.excluded_patterns.includes(p))
      settings.excluded_patterns = [...settings.excluded_patterns, p];
    newPattern = "";
  }
  function removePattern(i: number) {
    if (!settings) return;
    settings.excluded_patterns = settings.excluded_patterns.filter(
      (_, idx) => idx !== i,
    );
  }

  async function handleClearCache() {
    try {
      await clearCache();
      cacheInfo = null;
      success = "Cache cleared.";
      setTimeout(() => (success = ""), 1800);
      // Backend now also drops the in-memory index. Sync the global scan
      // store so Search/Disk Usage immediately switch to the unloaded
      // state instead of continuing to render stale results until restart.
      await resetIndexAfterCacheClear();
    } catch (e) {
      error = `Clear cache failed: ${e}`;
    }
  }

  function handleKey(e: KeyboardEvent) {
    if (e.key === "Escape") onClose();
  }

  onMount(load);
</script>

<svelte:window onkeydown={handleKey} />

<div class="overlay" onclick={onClose} onkeydown={(e) => e.key === "Escape" && onClose()} role="presentation">
  <div
    class="panel"
    onclick={(e) => e.stopPropagation()}
    onkeydown={(e) => e.stopPropagation()}
    role="dialog"
    aria-label="Settings"
    tabindex="-1"
  >
    <div class="ph">
      <span class="title">Settings</span>
      <button class="x" onclick={onClose}>×</button>
    </div>

    {#if !settings}
      <div class="loading">Loading settings…</div>
    {:else}
      <div class="body">
        <!-- Indexed roots -->
        <section class="section">
          <h3>Indexed Locations</h3>
          <p class="hint">
            The Search tab indexes files under these roots. Add as many as you
            want — drives, project folders, your home directory.
          </p>
          <ul class="root-list">
            {#each settings.indexed_roots as root, i}
              <li class="root-item" class:disabled={!root.enabled}>
                <input
                  type="checkbox"
                  class="enable-cb"
                  checked={root.enabled}
                  onchange={() => toggleRootEnabled(i)}
                  title={root.enabled ? "Enabled" : "Disabled"}
                />
                <span class="ico">
                  <svg viewBox="0 0 16 16" width="13" height="13"
                    ><path
                      d="M1.5 3.75A1.75 1.75 0 0 1 3.25 2h3a1.75 1.75 0 0 1 1.4.7l.6.8h4.5c.97 0 1.75.78 1.75 1.75v6.5c0 .97-.78 1.75-1.75 1.75H3.25a1.75 1.75 0 0 1-1.75-1.75V3.75Z"
                      fill={root.enabled ? "#e2b94a" : "#6b7280"}
                    /></svg
                  >
                </span>
                <span class="root-path mono">{root.path}</span>
                <button class="rm" onclick={() => removeRoot(i)} title="Remove"
                  >×</button
                >
              </li>
            {/each}
            {#if settings.indexed_roots.length === 0}
              <li class="empty-hint">No indexed roots — add one below.</li>
            {/if}
          </ul>
          <div class="add-row">
            <input
              type="text"
              bind:value={newRoot}
              onkeydown={(e) => e.key === "Enter" && addRoot()}
              placeholder="/home/sin/Documents"
            />
            <button class="btn ghost sm" onclick={addRoot}>Add root</button>
          </div>
        </section>

        <!-- Search behavior -->
        <section class="section">
          <h3>Search Behavior</h3>
          <label class="check"
            ><input type="checkbox" bind:checked={settings.case_sensitive} />
            Case-sensitive search</label
          >
          <label class="check"
            ><input type="checkbox" bind:checked={settings.match_path} /> Match
            against full path (in addition to filename)</label
          >
          <label class="check"
            ><input type="checkbox" bind:checked={settings.auto_load_cache} />
            Auto-load cached index on startup</label
          >
        </section>

        <!-- Window behavior -->
        <section class="section">
          <h3>Window</h3>
          <label class="check"
            ><input type="checkbox" bind:checked={settings.close_to_tray} />
            Close to tray (X button hides to system tray instead of quitting)</label
          >
        </section>

        <!-- Excluded patterns -->
        <section class="section">
          <h3>Excluded Folders</h3>
          <p class="hint">
            Folder names that match any pattern are skipped during indexing and
            disk usage scans.
          </p>
          <div class="chips">
            {#each settings.excluded_patterns as p, i}
              <span class="chip">
                {p}
                <button class="cx" onclick={() => removePattern(i)}>×</button>
              </span>
            {/each}
          </div>
          <div class="add-row">
            <input
              type="text"
              bind:value={newPattern}
              onkeydown={(e) => e.key === "Enter" && addPattern()}
              placeholder="e.g. .cache"
            />
            <button class="btn ghost sm" onclick={addPattern}>Add</button>
          </div>
        </section>

        <!-- Default scan root -->
        <section class="section">
          <h3>Default Disk Usage Path</h3>
          <input
            class="root-input mono"
            type="text"
            bind:value={settings.default_root}
            placeholder="/home"
          />
        </section>

        <!-- Cache -->
        <section class="section">
          <h3>Index Cache</h3>
          {#if cacheInfo}
            <div class="cache-info">
              <div>
                <span class="cl">Items:</span>
                <span class="cv">{cacheInfo.entry_count.toLocaleString()}</span>
              </div>
              <div>
                <span class="cl">Last indexed:</span>
                <span class="cv">{formatDate(cacheInfo.timestamp)}</span>
              </div>
              <div>
                <span class="cl">Roots:</span>
                <span class="cv mono"
                  >{cacheInfo.roots.length > 0
                    ? cacheInfo.roots.join(", ")
                    : cacheInfo.root}</span
                >
              </div>
            </div>
            <button class="btn ghost sm" onclick={handleClearCache}
              >Clear cache</button
            >
          {:else}
            <p class="hint muted">No cached index yet.</p>
          {/if}
        </section>
      </div>

      <div class="footer">
        {#if error}<span class="msg err">{error}</span>{/if}
        {#if success}<span class="msg ok">{success}</span>{/if}
        <button class="btn ghost" onclick={onClose}>Close</button>
        <button class="btn primary" disabled={saving} onclick={handleSave}
          >{saving ? "Saving…" : "Save"}</button
        >
      </div>
    {/if}
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
  }
  .panel {
    background: var(--bg-2);
    border: 1px solid var(--border-strong);
    border-radius: 8px;
    width: 560px;
    max-width: 92vw;
    max-height: 85vh;
    display: flex;
    flex-direction: column;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.6);
  }
  .ph {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 14px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-surface);
    border-radius: 7px 7px 0 0;
  }
  .title {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-strong);
  }
  .x {
    color: var(--text-muted);
    font-size: 18px;
    line-height: 1;
    padding: 0 8px;
    border-radius: 4px;
  }
  .x:hover {
    color: var(--text);
    background: var(--bg-hover);
  }
  .body {
    overflow: auto;
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  .loading {
    padding: 32px;
    text-align: center;
    color: var(--text-muted);
  }
  h3 {
    font-size: 11.5px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--text-strong);
    margin: 0 0 4px 0;
    font-weight: 700;
  }
  .hint {
    font-size: 11.5px;
    color: var(--text-muted);
    margin: 0 0 6px 0;
    line-height: 1.4;
  }
  .hint.muted {
    color: var(--text-faint);
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .root-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .root-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 8px;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 4px;
  }
  .root-item.disabled .root-path {
    color: var(--text-faint);
    text-decoration: line-through;
  }
  .enable-cb {
    accent-color: var(--accent);
    cursor: pointer;
  }
  .root-item .ico {
    display: inline-flex;
  }
  .root-path {
    flex: 1;
    color: var(--text);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .empty-hint {
    color: var(--text-faint);
    font-size: 12px;
    padding: 6px 8px;
  }
  .rm {
    color: var(--text-faint);
    font-size: 14px;
    padding: 0 6px;
    border-radius: 3px;
  }
  .rm:hover {
    color: var(--danger);
    background: rgba(248, 81, 73, 0.12);
  }

  .add-row {
    display: flex;
    gap: 6px;
  }
  .add-row input {
    flex: 1;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 5px 8px;
    color: var(--text);
    font-size: 12px;
  }
  .add-row input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12.5px;
    color: var(--text);
    cursor: pointer;
  }
  .check input[type="checkbox"] {
    accent-color: var(--accent);
    cursor: pointer;
  }

  .root-input {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 6px 8px;
    color: var(--text);
    font-size: 12px;
  }
  .root-input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 4px 2px 8px;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 11px;
    font-size: 11.5px;
    color: var(--text);
  }
  .cx {
    color: var(--text-faint);
    font-size: 12px;
    padding: 0 4px;
    line-height: 1;
    border-radius: 3px;
  }
  .cx:hover {
    color: var(--danger);
    background: rgba(248, 81, 73, 0.12);
  }

  .cache-info {
    display: flex;
    flex-direction: column;
    gap: 3px;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 8px 10px;
    font-size: 12px;
  }
  .cache-info .cl {
    color: var(--text-muted);
    margin-right: 4px;
  }
  .cache-info .cv {
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .cache-info .mono {
    font-family: var(--mono);
    font-size: 11.5px;
  }

  .footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    border-top: 1px solid var(--border);
    background: var(--bg-surface);
    justify-content: flex-end;
    border-radius: 0 0 7px 7px;
  }
  .msg {
    margin-right: auto;
    font-size: 12px;
  }
  .msg.err {
    color: var(--danger);
  }
  .msg.ok {
    color: var(--success);
  }

  .btn {
    height: 30px;
    padding: 0 14px;
    border-radius: 4px;
    font-size: 12px;
    font-weight: 600;
    border: 1px solid transparent;
    cursor: pointer;
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
    opacity: 0.5;
    cursor: not-allowed;
  }
  .btn.ghost {
    background: var(--bg-surface-2);
    color: var(--text);
    border-color: var(--border);
  }
  .btn.ghost:hover {
    background: var(--bg-hover);
  }

  .mono {
    font-family: var(--mono);
  }
</style>
