import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  scanIndexMulti,
  startIndexAll,
  getIndexRoots,
  scanDiskUsage,
  cancelDiskUsageScan,
  cancelJob,
  loadCachedIndex,
  getIndexStats,
  getIndexScanState,
  getDiskUsage,
  getDiskUsageScanState,
  getCacheInfo,
  type ScanProgress,
  type ScanComplete,
  type DiskUsageProgress,
  type DiskUsageComplete,
  type DiskUsageInfo,
  type DiskUsageEntry,
  type DiskUsageScanState,
  type IndexStats,
  type CacheInfo,
  type IndexRootView,
} from "./ipc";

// ---------- Index / Search global state ----------
export type IndexStatus =
  | "idle"
  | "loading-cache"
  | "indexing"
  | "ready"
  | "error"
  | "cancelled";

interface IndexJob {
  status: IndexStatus;
  jobId: string | null;
  error: string;
  progress: ScanProgress | null;
  complete: ScanComplete | null;
  stats: IndexStats | null;
  cacheInfo: CacheInfo | null;
  prevTotal: number; // total files+dirs from previous run, for ETA
  // Per-root configuration + runtime status (from backend).
  rootViews: IndexRootView[];
  // Job IDs we've cancelled or finished — late events from these are ignored.
  ignoredJobIds: Set<string>;
}

let indexState = $state<IndexJob>({
  status: "idle",
  jobId: null,
  error: "",
  progress: null,
  complete: null,
  stats: null,
  cacheInfo: null,
  prevTotal: 0,
  rootViews: [],
  ignoredJobIds: new Set(),
});

export function getIndexState(): IndexJob {
  return indexState;
}

// ---------- Disk usage global state ----------
export type DiskStatus = "idle" | "scanning" | "ready" | "error" | "cancelled";

interface DiskJob {
  status: DiskStatus;
  jobId: string | null;
  error: string;
  scanRoot: string;
  progress: DiskUsageProgress | null;
  complete: DiskUsageComplete | null;
  rootInfo: DiskUsageInfo | null;
  pathHistory: string[];
  childCache: Record<string, DiskUsageEntry[]>;
  expanded: Set<string>;
  selectedPath: string | null;
  prevTotalFiles: number;
  // Diagnostic surface: phase + engine + message — populated from
  // backend snapshots (events or pulled state).
  phase: string;
  engine: string;
  message: string;
  // Job IDs we've cancelled — late events from these are ignored.
  ignoredJobIds: Set<string>;
}

let diskState = $state<DiskJob>({
  status: "idle",
  jobId: null,
  error: "",
  scanRoot: "",
  progress: null,
  complete: null,
  rootInfo: null,
  pathHistory: [],
  childCache: {},
  expanded: new Set(),
  selectedPath: null,
  prevTotalFiles: 0,
  phase: "idle",
  engine: "",
  message: "",
  ignoredJobIds: new Set(),
});

export function getDiskState(): DiskJob {
  return diskState;
}

// ---------- ETA helpers ----------
export function etaText(
  filesFound: number,
  prevTotal: number,
  filesPerSec: number,
  elapsedMs: number,
): string {
  if (filesPerSec <= 0 || elapsedMs < 1500) return "calculating…";
  if (prevTotal > filesFound) {
    const remaining = prevTotal - filesFound;
    const sec = Math.round(remaining / filesPerSec);
    if (sec < 1) return "<1s";
    if (sec < 60) return `${sec}s`;
    const m = Math.floor(sec / 60);
    const s = sec % 60;
    if (m < 60) return `${m}m ${s}s`;
    const h = Math.floor(m / 60);
    return `${h}h ${m % 60}m`;
  }
  return "calculating…";
}

// ---------- Disk usage history (localStorage) ----------
const DU_HISTORY_KEY = "cove.disk_usage_history.v1";

function loadDuHistory(): Record<string, number> {
  try {
    const raw = localStorage.getItem(DU_HISTORY_KEY);
    return raw ? JSON.parse(raw) : {};
  } catch {
    return {};
  }
}

function saveDuHistory(h: Record<string, number>): void {
  try {
    localStorage.setItem(DU_HISTORY_KEY, JSON.stringify(h));
  } catch {
    /* ignore */
  }
}

function rememberDuTotal(root: string, total: number): void {
  const h = loadDuHistory();
  h[root] = total;
  saveDuHistory(h);
}

function priorDuTotal(root: string): number {
  const h = loadDuHistory();
  return h[root] ?? 0;
}

// ---------- Listener wiring (initialize once at app boot) ----------
let listenersInited = false;
const unlisteners: UnlistenFn[] = [];

export async function initScanListeners(): Promise<void> {
  if (listenersInited) return;
  listenersInited = true;

  unlisteners.push(
    await listen<ScanProgress>("index.progress", (e) => {
      const id = e.payload.job_id;
      // Drop stale events from jobs we've cancelled or that already
      // completed — without this guard, a late progress event flips the
      // UI back to "indexing" after the scan has finished.
      if (indexState.ignoredJobIds.has(id)) return;
      if (indexState.complete && indexState.complete.job_id === id) return;
      // Identity guard: while jobId is null we accept the first progress
      // (covers the IPC-return race for fast scans).
      if (indexState.jobId && id !== indexState.jobId) return;
      indexState.progress = e.payload;
      if (indexState.status !== "indexing") indexState.status = "indexing";
    }),
  );
  unlisteners.push(
    await listen<ScanComplete>("index.complete", async (e) => {
      const id = e.payload.job_id;
      if (indexState.ignoredJobIds.has(id)) return;
      // Identity guard: don't apply a result for a scan that's been
      // replaced. While jobId is null (boot/recovery) we accept it.
      if (indexState.jobId && id !== indexState.jobId) return;
      indexState.complete = e.payload;
      indexState.jobId = null;
      indexState.status = "ready";
      indexState.error = "";
      // Drop the in-flight progress snapshot. Without this, if status
      // ever flips back to "indexing" before the next scan emits its
      // first progress event, the banner would render the just-finished
      // scan's stats instead of "Starting…", confusing the user.
      indexState.progress = null;
      stopIndexWatchdog();
      // Mark this job as finished so any straggler progress event with
      // the same id can't revert status back to "indexing".
      indexState.ignoredJobIds = new Set([
        ...indexState.ignoredJobIds,
        id,
      ]);
      try {
        indexState.stats = await getIndexStats();
        indexState.cacheInfo = await getCacheInfo();
        indexState.rootViews = await getIndexRoots();
        indexState.prevTotal =
          (indexState.stats?.total ?? 0) || indexState.prevTotal;
      } catch {
        /* ignore */
      }
    }),
  );
  unlisteners.push(
    await listen<{ job_id: string; error: string }>("index.error", (e) => {
      const id = e.payload?.job_id;
      if (id && indexState.ignoredJobIds.has(id)) return;
      if (indexState.jobId && id && id !== indexState.jobId) return;
      indexState.jobId = null;
      indexState.error = e.payload?.error || "Index error";
      indexState.status = "error";
      indexState.progress = null;
      if (id) {
        indexState.ignoredJobIds = new Set([
          ...indexState.ignoredJobIds,
          id,
        ]);
      }
      stopIndexWatchdog();
    }),
  );

  unlisteners.push(
    await listen<DiskUsageProgress>("diskusage.progress", (e) => {
      const id = e.payload.job_id;
      if (diskState.ignoredJobIds.has(id)) return;
      // If we already know our active jobId and this is a different scan,
      // drop it. While jobId is null (between "scanning" status set and
      // IPC return) we accept events — those belong to our just-started scan.
      if (diskState.jobId && id !== diskState.jobId) return;
      diskState.progress = e.payload;
      diskState.phase = e.payload.phase;
      diskState.engine = e.payload.engine;
      if (diskState.status !== "scanning") diskState.status = "scanning";
    }),
  );
  unlisteners.push(
    await listen<DiskUsageComplete>("diskusage.complete", async (e) => {
      const id = e.payload.job_id;
      if (diskState.ignoredJobIds.has(id)) return;
      if (diskState.jobId && id !== diskState.jobId) return;
      // Backend completion is authoritative. Exit the scanning UI BEFORE
      // hydration so Cancel hides and the spinner stops the moment we
      // know the backend is done. Show "Loading results…" until the
      // hydrated tree arrives or an error is surfaced.
      diskState.complete = e.payload;
      diskState.jobId = null;
      diskState.status = "ready";
      diskState.error = "";
      diskState.phase = "done";
      diskState.engine = e.payload.engine;
      diskState.message = "Loading results…";
      // Mark this job finished so any straggler progress event with the
      // same id can't flip status back to "scanning" during hydration.
      diskState.ignoredJobIds = new Set([
        ...diskState.ignoredJobIds,
        id,
      ]);
      rememberDuTotal(e.payload.root, e.payload.total_files);
      try {
        const info = await getDiskUsage(e.payload.root);
        if (diskState.complete?.job_id !== id) return;
        diskState.rootInfo = info;
        diskState.pathHistory = [];
        diskState.expanded = new Set();
        diskState.childCache = { [info.path]: info.children };
        diskState.selectedPath = info.path;
        diskState.message = "Scan complete";
      } catch (err) {
        // Hydration failed but the scan itself completed. Surface a real
        // error so Cancel stays hidden and the user can Rescan.
        if (diskState.complete?.job_id !== id) return;
        const msg = `Could not load scan result: ${err}`;
        diskState.error = msg;
        diskState.status = "error";
        diskState.message = msg;
      }
    }),
  );
  unlisteners.push(
    await listen<{ job_id: string; error: string }>("diskusage.error", (e) => {
      const id = e.payload?.job_id;
      if (id && diskState.ignoredJobIds.has(id)) return;
      if (diskState.jobId && id && id !== diskState.jobId) return;
      diskState.jobId = null;
      const err = e.payload?.error || "Disk usage error";
      diskState.error = err;
      diskState.message = err;
      diskState.phase = "done";
      diskState.status = err === "Scan cancelled" ? "cancelled" : "error";
    }),
  );
}

// ---- Index watchdog ---------------------------------------------------------
// Defensive reconciliation: while the UI thinks a scan is running, poll the
// backend's authoritative state on an interval. The backend's worker can
// drop its `index.complete` emission when its job got replaced or cancelled
// mid-flight (publish_guard re-validate fails, or the `still_active` check
// fails). When that happens the frontend has no event to act on and would
// otherwise be stuck on "Indexing — Starting…" until the user hits Cancel
// or switches tabs. The watchdog calls `syncIndexScanState`, which uses the
// same backend snapshot path that tab-switch and Cancel use, so the
// recovered state matches those flows exactly. Auto-stops when status
// leaves "indexing".
let indexWatchdogTimer: ReturnType<typeof setInterval> | null = null;
const INDEX_WATCHDOG_INTERVAL_MS = 1500;

export function startIndexWatchdog(): void {
  if (indexWatchdogTimer) return;
  indexWatchdogTimer = setInterval(() => {
    if (indexState.status !== "indexing") {
      stopIndexWatchdog();
      return;
    }
    syncIndexScanState().catch(() => {});
  }, INDEX_WATCHDOG_INTERVAL_MS);
}

export function stopIndexWatchdog(): void {
  if (indexWatchdogTimer) {
    clearInterval(indexWatchdogTimer);
    indexWatchdogTimer = null;
  }
}

// ---- Disk usage watchdog ----------------------------------------------------
// Defensive reconciliation: while the UI thinks a scan is running, poll the
// backend's authoritative state on an interval. If the backend reports the
// scan finished but our event-driven path missed the diskusage.complete event
// (dropped event, listener race, slow JS event loop, the user reload race),
// the watchdog hydrates the result and exits scanning state. The interval
// auto-stops when status is no longer "scanning".
let diskWatchdogTimer: ReturnType<typeof setInterval> | null = null;
const DISK_WATCHDOG_INTERVAL_MS = 1500;

export function startDiskWatchdog(): void {
  if (diskWatchdogTimer) return;
  diskWatchdogTimer = setInterval(() => {
    if (diskState.status !== "scanning") {
      stopDiskWatchdog();
      return;
    }
    syncDiskScanState().catch(() => {});
  }, DISK_WATCHDOG_INTERVAL_MS);
}

export function stopDiskWatchdog(): void {
  if (diskWatchdogTimer) {
    clearInterval(diskWatchdogTimer);
    diskWatchdogTimer = null;
  }
}

/**
 * Pull authoritative scan state from the backend. Called on mount in case
 * the frontend reloaded mid-scan or the user just switched tabs back.
 * Reconciles status, progress, and result tree without restarting anything.
 */
export async function syncDiskScanState(): Promise<void> {
  let s: DiskUsageScanState;
  try {
    s = await getDiskUsageScanState();
  } catch {
    return;
  }
  diskState.engine = s.engine;
  diskState.phase = s.phase;
  diskState.message = s.message;

  if (s.status === "scanning") {
    if (!diskState.jobId && s.scan_id) diskState.jobId = s.scan_id;
    if (!diskState.scanRoot) diskState.scanRoot = s.root_path;
    diskState.status = "scanning";
    diskState.progress = {
      job_id: s.scan_id ?? "",
      files_found: s.files_scanned,
      dirs_found: s.dirs_scanned,
      bytes_found: s.bytes_scanned,
      errors: s.errors_count,
      skipped: s.skipped_count,
      elapsed_ms: s.elapsed_ms,
      current_path: s.current_path,
      phase: s.phase,
      engine: s.engine,
    };
  } else if (s.status === "completed" && s.final_summary) {
    const summaryAtEntry = s.final_summary;
    const expectedJobId = summaryAtEntry.job_id;

    // Identity guard: a NEW different scan locally in flight means this
    // backend snapshot is stale. Don't apply.
    if (diskState.jobId && diskState.jobId !== expectedJobId) return;
    if (diskState.ignoredJobIds.has(expectedJobId)) return;
    // Don't reapply a result we already finished hydrating.
    if (
      diskState.complete &&
      diskState.complete.job_id === expectedJobId &&
      diskState.rootInfo &&
      diskState.rootInfo.path === summaryAtEntry.root
    ) {
      return;
    }

    // Backend completion is authoritative. Exit the scanning UI BEFORE
    // attempting hydration — Cancel must hide and the spinner must stop
    // the moment we know the backend is done.
    diskState.complete = summaryAtEntry;
    diskState.jobId = null;
    diskState.phase = "done";
    diskState.scanRoot = s.root_path;
    // Filter out any straggler progress/error events for this job so a
    // late event can't flip the UI back to "scanning" during hydration.
    // Intentionally NOT used as a hydration guard for this same job —
    // staleness for the same expectedJobId is detected via diskState.complete
    // and diskState.jobId, which startDiskScan resets when a new scan begins.
    diskState.ignoredJobIds = new Set([
      ...diskState.ignoredJobIds,
      expectedJobId,
    ]);
    if (!diskState.rootInfo) {
      diskState.status = "ready";
      diskState.message = "Loading results…";
      diskState.error = "";
      try {
        const info = await getDiskUsage(summaryAtEntry.root);
        // Identity re-check after await: only reject if a NEWER scan
        // replaced this one. startDiskScan resets `complete` to null and
        // installs a new `jobId` — either signals a stale apply. Do NOT
        // gate on ignoredJobIds here: this same expectedJobId was added
        // above for straggler-event filtering, so checking it would
        // always reject hydration for the same completed job.
        if (!diskState.complete || diskState.complete.job_id !== expectedJobId) return;
        if (diskState.jobId && diskState.jobId !== expectedJobId) return;
        diskState.rootInfo = info;
        diskState.pathHistory = [];
        diskState.expanded = new Set();
        diskState.childCache = { [info.path]: info.children };
        diskState.selectedPath = info.path;
        diskState.message = "Scan complete";
      } catch (err) {
        // Don't leave the UI stuck — surface a real error so the user
        // can Rescan. Same identity guard before we mutate.
        if (!diskState.complete || diskState.complete.job_id !== expectedJobId) return;
        if (diskState.jobId && diskState.jobId !== expectedJobId) return;
        const msg = `Could not load scan result: ${err}`;
        diskState.error = msg;
        diskState.status = "error";
        diskState.message = msg;
      }
    } else {
      // We already had results; just confirm the terminal state.
      diskState.status = "ready";
      diskState.message = "Scan complete";
    }
  } else if (s.status === "error") {
    diskState.status = "error";
    diskState.error = s.message;
    diskState.jobId = null;
  } else if (s.status === "cancelled") {
    diskState.status = diskState.rootInfo ? "ready" : "cancelled";
    diskState.jobId = null;
  }
}

// ---------- Index actions ----------
export async function startIndexScan(roots: string[]): Promise<void> {
  if (indexState.status === "indexing") return; // already running
  if (!roots || roots.length === 0) {
    indexState.error = "No indexed roots configured. Open Settings to add one.";
    indexState.status = "error";
    return;
  }
  // Mark any in-flight job as ignored before resetting so its late
  // events can't corrupt the new scan.
  if (indexState.jobId) {
    indexState.ignoredJobIds = new Set([
      ...indexState.ignoredJobIds,
      indexState.jobId,
    ]);
    try {
      await cancelJob(indexState.jobId);
    } catch {
      /* ignore */
    }
  }
  // Capture previous total for ETA
  indexState.prevTotal =
    indexState.stats?.total ?? indexState.cacheInfo?.entry_count ?? 0;
  indexState.status = "indexing";
  indexState.error = "";
  indexState.progress = null;
  indexState.complete = null;
  indexState.jobId = null;
  try {
    indexState.jobId = await scanIndexMulti(roots);
  } catch (e) {
    indexState.status = "error";
    indexState.error = `Scan failed: ${e}`;
  }
  // After the IPC returns, pull authoritative state once. Tiny scans may
  // already be done by then; this catches the case where the complete
  // event fired before the listener was wired or jobId was set.
  setTimeout(() => {
    syncIndexScanState().catch(() => {});
  }, 50);
  // Defensive watchdog: if the backend later drops its index.complete
  // emission (replaced/cancelled job whose still_active re-check fails),
  // the listener never fires. Without this the UI stays on "Indexing —
  // Starting…" until the user hits Cancel or switches tabs. The watchdog
  // self-stops the moment status leaves "indexing".
  startIndexWatchdog();
}

export async function cancelIndexScan(): Promise<void> {
  if (!indexState.jobId) return;
  // Ignore any straggler events from this job.
  indexState.ignoredJobIds = new Set([
    ...indexState.ignoredJobIds,
    indexState.jobId,
  ]);
  await cancelJob(indexState.jobId);
  indexState.jobId = null;
  indexState.status = (indexState.stats?.total ?? 0) > 0 ? "ready" : "cancelled";
  indexState.progress = null;
  stopIndexWatchdog();
}

/**
 * Pull authoritative index state from the backend. Reconciles status when
 * the frontend missed a completion event (tab switch race, dropped event,
 * fast scan finishing before listener was wired). Never starts a scan.
 */
export async function syncIndexScanState(): Promise<void> {
  let s;
  try {
    s = await getIndexScanState();
  } catch {
    return;
  }
  // Always refresh stats from the snapshot — they're authoritative.
  indexState.stats = {
    total: s.total,
    files: s.files,
    dirs: s.dirs,
    roots: s.roots,
    indexed_at: s.indexed_at,
  };
  if (s.root_views) indexState.rootViews = s.root_views;

  if (s.is_running) {
    // If the backend's active job is one we've already cancelled/finished
    // locally, treat it as terminal. The backend is mid-unwind; do NOT
    // resurrect it as active indexing — the user already cancelled, or
    // we already applied its completion. Leave the UI in its current
    // ready/cancelled state. The forthcoming index.error/index.complete
    // for this job will be filtered by the listeners (ignoredJobIds).
    if (
      s.current_job_id &&
      indexState.ignoredJobIds.has(s.current_job_id)
    ) {
      // Don't adopt, don't un-ignore, don't flip to indexing.
    } else {
      // Backend is actively scanning a job we still consider live.
      // Adopt its job id if we don't have one (e.g. resume from a fresh
      // page) and ensure the UI reflects indexing.
      if (s.current_job_id && !indexState.jobId) {
        indexState.jobId = s.current_job_id;
      }
      if (
        indexState.status !== "indexing" &&
        indexState.status !== "loading-cache"
      ) {
        indexState.status = "indexing";
      }
      // We adopted (or confirmed) an active backend scan. Make sure the
      // watchdog is running so a later dropped index.complete (replaced/
      // cancelled job) still recovers the UI without a tab switch. The
      // start fast-paths cover user-initiated scans; this covers reload
      // and Search-tab activation while a scan was already in flight.
      if (indexState.status === "indexing") startIndexWatchdog();
    }
  } else {
    // Backend has no active scan. If our UI is stuck on "indexing" but the
    // backend has finished (dropped/missed complete event), recover here.
    if (indexState.status === "indexing") {
      indexState.jobId = null;
      indexState.error = "";
      indexState.progress = null;
      indexState.status = s.total > 0 ? "ready" : "idle";
      stopIndexWatchdog();
      try {
        indexState.cacheInfo = await getCacheInfo();
      } catch {
        /* ignore */
      }
    } else if (
      indexState.status === "idle" &&
      s.total > 0 &&
      indexState.stats &&
      indexState.stats.total > 0
    ) {
      // Backend has data but we never picked it up — surface as ready.
      indexState.status = "ready";
    }
    // Even if we already think we're ready, refresh cache info so the
    // status bar reflects the latest indexed_at/roots.
    try {
      indexState.cacheInfo = await getCacheInfo();
    } catch {
      /* ignore */
    }
  }
}

export async function startIndexAllRoots(): Promise<void> {
  if (indexState.status === "indexing") return;
  if (indexState.jobId) {
    indexState.ignoredJobIds = new Set([
      ...indexState.ignoredJobIds,
      indexState.jobId,
    ]);
    try {
      await cancelJob(indexState.jobId);
    } catch {
      /* ignore */
    }
  }
  indexState.prevTotal =
    indexState.stats?.total ?? indexState.cacheInfo?.entry_count ?? 0;
  indexState.status = "indexing";
  indexState.error = "";
  indexState.progress = null;
  indexState.complete = null;
  indexState.jobId = null;
  try {
    indexState.jobId = await startIndexAll();
  } catch (e) {
    indexState.status = "error";
    indexState.error = `Scan failed: ${e}`;
  }
  setTimeout(() => {
    syncIndexScanState().catch(() => {});
  }, 50);
  // Defensive watchdog (see startIndexScan for rationale).
  startIndexWatchdog();
}

export async function refreshIndexRoots(): Promise<void> {
  try {
    indexState.rootViews = await getIndexRoots();
  } catch {
    /* ignore */
  }
}

export async function tryLoadCache(): Promise<boolean> {
  if (indexState.status === "indexing") return false;
  if ((indexState.stats?.total ?? 0) > 0) return true;
  indexState.status = "loading-cache";
  try {
    const resp = await loadCachedIndex();
    if (resp.status === "skipped_indexing") {
      // Backend refused because a scan is active — leave indexState alone
      // beyond resetting the loading-cache transient, so the existing
      // indexing UI continues uninterrupted on the next sync.
      indexState.status = "idle";
      await syncIndexScanState();
      return false;
    }
    if (resp.status === "skipped_empty") {
      // Cache existed but enabled-root filter left zero entries. Stay idle
      // ("No index loaded") rather than flipping to a deceptively-ready
      // empty search.
      indexState.status = "idle";
      return false;
    }
    indexState.stats = await getIndexStats();
    indexState.cacheInfo = await getCacheInfo();
    indexState.rootViews = await getIndexRoots();
    if ((indexState.stats?.total ?? 0) === 0) {
      indexState.status = "idle";
      return false;
    }
    indexState.status = "ready";
    return true;
  } catch {
    indexState.status = "idle";
    return false;
  }
}

export async function refreshIndexStats(): Promise<void> {
  try {
    indexState.stats = await getIndexStats();
    indexState.cacheInfo = await getCacheInfo();
    indexState.rootViews = await getIndexRoots();
  } catch {
    /* ignore */
  }
}

/**
 * Reset the global index state after a successful Clear Cache. Pairs with
 * the backend `clear_cache` command which now also drops the in-memory
 * FileIndex and last_index. Without this companion reset, Search and Disk
 * Usage keep showing the previously-loaded entries because `idx.status`
 * stays `ready` and `idx.stats.total` is stale until restart.
 */
export async function resetIndexAfterCacheClear(): Promise<void> {
  // Anything still in flight from a prior scan must not be allowed to
  // resurrect indexState — mark it ignored before clearing.
  if (indexState.jobId) {
    indexState.ignoredJobIds = new Set([
      ...indexState.ignoredJobIds,
      indexState.jobId,
    ]);
  }
  indexState.jobId = null;
  indexState.status = "idle";
  indexState.error = "";
  indexState.progress = null;
  indexState.complete = null;
  indexState.cacheInfo = null;
  indexState.stats = null;
  indexState.prevTotal = 0;
  // Pull authoritative numbers from the backend so any view that reads
  // stats/rootViews/cacheInfo (Settings, Search, Disk Usage status bar)
  // sees the post-clear zero state without an app restart.
  try {
    indexState.stats = await getIndexStats();
    indexState.cacheInfo = await getCacheInfo();
    indexState.rootViews = await getIndexRoots();
  } catch {
    /* ignore */
  }
}

// ---------- Disk usage actions ----------
export async function startDiskScan(root: string): Promise<void> {
  if (!root.trim()) return;
  // Mark any in-flight scan as ignored before resetting state, so its
  // late progress/error/complete events can't corrupt the new scan.
  if (diskState.jobId) {
    diskState.ignoredJobIds = new Set([
      ...diskState.ignoredJobIds,
      diskState.jobId,
    ]);
    try {
      await cancelJob(diskState.jobId);
    } catch {
      /* ignore */
    }
  }
  diskState.status = "scanning";
  diskState.scanRoot = root;
  diskState.error = "";
  diskState.progress = null;
  diskState.complete = null;
  diskState.rootInfo = null;
  diskState.pathHistory = [];
  diskState.childCache = {};
  diskState.expanded = new Set();
  diskState.selectedPath = null;
  diskState.prevTotalFiles = priorDuTotal(root);
  diskState.phase = "starting";
  diskState.message = "Starting scan…";
  diskState.jobId = null;
  try {
    diskState.jobId = await scanDiskUsage(root);
  } catch (e) {
    diskState.status = "error";
    diskState.error = `Scan failed: ${e}`;
  }
  // After the IPC returns, pull authoritative state once. Tiny scans may
  // have already finished by the time invoke resolves; this catches the
  // case where progress/complete events arrived before jobId was set.
  setTimeout(() => {
    syncDiskScanState().catch(() => {});
  }, 50);
  // Defensive watchdog: keep reconciling against the backend until the UI
  // leaves scanning state. Protects against missed completion events.
  startDiskWatchdog();
}

export async function cancelDiskScan(): Promise<void> {
  if (!diskState.jobId) return;
  // Ignore the cancellation-error event that the backend will emit.
  diskState.ignoredJobIds = new Set([
    ...diskState.ignoredJobIds,
    diskState.jobId,
  ]);
  try {
    await cancelDiskUsageScan();
  } catch {
    try {
      await cancelJob(diskState.jobId);
    } catch {
      /* ignore */
    }
  }
  diskState.jobId = null;
  diskState.status = diskState.rootInfo ? "ready" : "cancelled";
  diskState.phase = "done";
  diskState.message = "Scan cancelled";
}

export async function navigateDiskTo(path: string): Promise<void> {
  try {
    const info = await getDiskUsage(path);
    diskState.rootInfo = info;
    diskState.expanded = new Set();
    diskState.childCache = { [info.path]: info.children };
    diskState.selectedPath = info.path;
  } catch (e) {
    console.error("navigateDiskTo failed:", e);
  }
}

export async function drillDownDisk(entry: DiskUsageEntry): Promise<void> {
  if (!entry.is_dir || !diskState.rootInfo) return;
  diskState.pathHistory = [...diskState.pathHistory, diskState.rootInfo.path];
  await navigateDiskTo(entry.path);
}

export async function goUpDisk(): Promise<void> {
  if (diskState.pathHistory.length === 0) return;
  const prev = diskState.pathHistory[diskState.pathHistory.length - 1];
  diskState.pathHistory = diskState.pathHistory.slice(0, -1);
  await navigateDiskTo(prev);
}

export async function toggleExpandDisk(entry: DiskUsageEntry): Promise<void> {
  if (!entry.is_dir) return;
  const next = new Set(diskState.expanded);
  if (next.has(entry.path)) {
    next.delete(entry.path);
  } else {
    next.add(entry.path);
    if (!diskState.childCache[entry.path]) {
      try {
        const info = await getDiskUsage(entry.path);
        diskState.childCache = {
          ...diskState.childCache,
          [entry.path]: info.children,
        };
      } catch {
        /* ignore */
      }
    }
  }
  diskState.expanded = next;
}

export function setDiskSelected(path: string | null): void {
  diskState.selectedPath = path;
}

// breadcrumbs helper
export function diskBreadcrumbs(): { path: string; label: string }[] {
  if (!diskState.rootInfo) return [];
  return [
    ...diskState.pathHistory.map((p) => ({
      path: p,
      label: p.split(/[/\\]/).filter(Boolean).pop() || p,
    })),
    {
      path: diskState.rootInfo.path,
      label: diskState.rootInfo.name,
    },
  ];
}
