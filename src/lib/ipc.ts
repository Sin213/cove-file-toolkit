import { invoke } from "@tauri-apps/api/core";

export interface FileItem {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  mtime: number;
  root_id: string;
  root_path: string;
}

export interface SearchFilters {
  extensions?: string[];
  min_size?: number;
  max_size?: number;
  min_mtime?: number;
  max_mtime?: number;
  path_include?: string;
  path_exclude?: string;
  dirs_only?: boolean;
  files_only?: boolean;
}

export interface SearchSort {
  field?: string;
  ascending?: boolean;
}

export interface SearchPage {
  items: FileItem[];
  total: number;
  page: number;
  page_size: number;
}

export interface ScanProgress {
  job_id: string;
  files_found: number;
  dirs_found: number;
  elapsed_ms: number;
  current_path: string;
  current_root: string;
  current_root_id: string;
  roots_done: number;
  roots_total: number;
}

export interface RootResult {
  root_id: string;
  path: string;
  canonical_path: string;
  item_count: number;
  files: number;
  dirs: number;
  error: string | null;
  covered_by?: string | null;
}

export interface ScanComplete {
  job_id: string;
  total_files: number;
  total_dirs: number;
  elapsed_ms: number;
  roots: string[];
  per_root: RootResult[];
}

export interface IndexStats {
  total: number;
  files: number;
  dirs: number;
  roots: string[];
  indexed_at: number;
}

export type RootStateName =
  | "idle"
  | "pending"
  | "indexing"
  | "ready"
  | "error"
  | "disabled"
  | "missing";

export interface IndexRootView {
  id: string;
  path: string;
  display_name: string;
  enabled: boolean;
  state: RootStateName;
  item_count: number;
  last_indexed: number;
  error: string | null;
}

export interface DetectedRoot {
  path: string;
  display_name: string;
  kind: string;
}

export interface IndexScanStateView {
  is_running: boolean;
  current_job_id: string | null;
  total: number;
  files: number;
  dirs: number;
  roots: string[];
  indexed_at: number;
  root_views: IndexRootView[];
}

export async function scanIndex(root: string): Promise<string> {
  return invoke<string>("scan_index", { root });
}

export async function scanIndexMulti(roots: string[]): Promise<string> {
  return invoke<string>("scan_index_multi", { roots });
}

export async function startIndexAll(): Promise<string> {
  return invoke<string>("start_index_all");
}

export async function rescanIndexRoot(rootId: string): Promise<string> {
  return invoke<string>("rescan_index_root", { rootId });
}

export async function getIndexRoots(): Promise<IndexRootView[]> {
  return invoke<IndexRootView[]>("get_index_roots");
}

export async function addIndexRoot(
  path: string,
  displayName?: string,
): Promise<IndexRootView> {
  return invoke<IndexRootView>("add_index_root", {
    path,
    displayName: displayName ?? null,
  });
}

export async function removeIndexRoot(rootId: string): Promise<void> {
  return invoke<void>("remove_index_root", { rootId });
}

export async function updateIndexRootEnabled(
  rootId: string,
  enabled: boolean,
): Promise<void> {
  return invoke<void>("update_index_root_enabled", { rootId, enabled });
}

export async function detectIndexRoots(): Promise<DetectedRoot[]> {
  return invoke<DetectedRoot[]>("detect_index_roots");
}

export async function search(
  query: string,
  filters: SearchFilters = {},
  sort: SearchSort = {},
  page: number = 0,
  pageSize: number = 200,
): Promise<SearchPage> {
  return invoke<SearchPage>("search", {
    query,
    filters,
    sort,
    page,
    pageSize,
  });
}

export async function cancelJob(id: string): Promise<boolean> {
  return invoke<boolean>("cancel_job", { id });
}

export async function getIndexStats(): Promise<IndexStats> {
  return invoke<IndexStats>("get_index_stats");
}

export async function getIndexScanState(): Promise<IndexScanStateView> {
  return invoke<IndexScanStateView>("get_index_scan_state");
}

export interface DiskUsageEntry {
  name: string;
  path: string;
  size: number;
  file_count: number;
  dir_count: number;
  child_count: number;
  item_count: number;
  mtime: number;
  percentage: number;
  is_dir: boolean;
}

export interface ExtensionStats {
  extension: string;
  size: number;
  count: number;
  percentage: number;
}

export interface DiskUsageInfo {
  path: string;
  name: string;
  total_size: number;
  total_file_count: number;
  own_size: number;
  own_file_count: number;
  children: DiskUsageEntry[];
  extensions: ExtensionStats[];
  largest_files: DiskUsageEntry[];
}

export type DiskScanStatusBackend =
  | "idle"
  | "scanning"
  | "completed"
  | "cancelled"
  | "error";

export type DiskScanPhaseBackend =
  | "idle"
  | "starting"
  | "scanning"
  | "finalizing"
  | "done";

export interface DiskUsageProgress {
  job_id: string;
  files_found: number;
  dirs_found: number;
  bytes_found: number;
  errors: number;
  skipped: number;
  elapsed_ms: number;
  current_path: string;
  phase: DiskScanPhaseBackend;
  engine: string;
}

export interface DiskUsageComplete {
  job_id: string;
  root: string;
  total_size: number;
  total_files: number;
  total_dirs: number;
  errors: number;
  skipped: number;
  elapsed_ms: number;
  engine: string;
}

export interface DiskUsageScanState {
  scan_id: string | null;
  root_path: string;
  status: DiskScanStatusBackend;
  phase: DiskScanPhaseBackend;
  engine: string;
  current_path: string;
  elapsed_ms: number;
  files_scanned: number;
  dirs_scanned: number;
  bytes_scanned: number;
  errors_count: number;
  skipped_count: number;
  message: string;
  final_summary: DiskUsageComplete | null;
}

export async function scanDiskUsage(root: string): Promise<string> {
  return invoke<string>("scan_disk_usage", { root });
}

export async function cancelDiskUsageScan(): Promise<boolean> {
  return invoke<boolean>("cancel_disk_usage_scan");
}

export async function getDiskUsageScanState(): Promise<DiskUsageScanState> {
  return invoke<DiskUsageScanState>("get_disk_usage_scan_state");
}

export async function getDiskUsage(path: string): Promise<DiskUsageInfo> {
  return invoke<DiskUsageInfo>("get_disk_usage", { path });
}

export type RenameRule =
  | { type: "prefix"; text: string }
  | { type: "suffix"; text: string }
  | { type: "remove"; text: string; case_sensitive: boolean; stem_only?: boolean }
  | { type: "remove_range"; start: number; end: number }
  | {
      type: "replace";
      from: string;
      to: string;
      case_sensitive: boolean;
      stem_only?: boolean;
    }
  | {
      type: "regex_replace";
      pattern: string;
      replacement: string;
      stem_only?: boolean;
    }
  | {
      type: "numbering";
      start: number;
      step: number;
      padding: number;
      position: string;
      separator?: string;
    }
  | { type: "case_change"; mode: string }
  | { type: "ext_case"; mode: string }
  | { type: "ext_change"; new_ext: string }
  | { type: "remove_ends"; first: number; last: number };

export interface RenamePreviewItem {
  original_path: string;
  original_name: string;
  new_name: string;
  new_path: string;
  status: string;
  message: string | null;
}

export async function previewRename(
  paths: string[],
  rules: RenameRule[],
): Promise<RenamePreviewItem[]> {
  return invoke<RenamePreviewItem[]>("preview_rename", { paths, rules });
}

export async function applyRename(
  paths: string[],
  rules: RenameRule[],
): Promise<number> {
  return invoke<number>("apply_rename", { paths, rules });
}

export async function undoRename(): Promise<number> {
  return invoke<number>("undo_rename");
}

export interface IndexRootConfig {
  id: string;
  path: string;
  display_name: string;
  enabled: boolean;
}

export interface AppSettings {
  default_root: string;
  excluded_patterns: string[];
  indexed_roots: IndexRootConfig[];
  case_sensitive: boolean;
  match_path: boolean;
  auto_load_cache: boolean;
}

export interface CachedRootMeta {
  id: string;
  path: string;
  canonical_path: string;
  item_count: number;
}

export interface CacheInfo {
  schema_version: number;
  root: string;
  timestamp: number;
  entry_count: number;
  roots: string[];
  root_meta: CachedRootMeta[];
}

export async function getSettings(): Promise<AppSettings> {
  return invoke<AppSettings>("get_settings");
}

export async function saveSettings(settings: AppSettings): Promise<void> {
  return invoke<void>("save_settings", { settings });
}

export async function getCacheInfo(): Promise<CacheInfo | null> {
  return invoke<CacheInfo | null>("get_cache_info");
}

export type LoadCachedIndexResponse =
  | { status: "loaded"; info: CacheInfo }
  | { status: "skipped_indexing" }
  | { status: "skipped_empty" };

export async function loadCachedIndex(): Promise<LoadCachedIndexResponse> {
  return invoke<LoadCachedIndexResponse>("load_cached_index");
}

export async function clearCache(): Promise<void> {
  return invoke<void>("clear_cache");
}

export async function rescanDiskDir(path: string): Promise<DiskUsageInfo> {
  return invoke<DiskUsageInfo>("rescan_disk_dir", { path });
}

export async function moveToTrash(paths: string[]): Promise<void> {
  return invoke<void>("move_to_trash", { paths });
}

export async function renamePath(from: string, to: string): Promise<void> {
  return invoke<void>("rename_path", { from, to });
}

export async function copyPaths(srcs: string[], destDir: string): Promise<void> {
  return invoke<void>("copy_paths", { srcs, destDir });
}

export async function movePaths(srcs: string[], destDir: string): Promise<void> {
  return invoke<void>("move_paths", { srcs, destDir });
}

export async function openPath(path: string): Promise<void> {
  return invoke<void>("open_path", { path });
}

export async function revealInFolder(path: string): Promise<void> {
  return invoke<void>("reveal_in_folder", { path });
}

// helpers
export function formatSize(bytes: number, decimals = 1): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes === 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(decimals)} KB`;
  if (bytes < 1024 * 1024 * 1024)
    return `${(bytes / (1024 * 1024)).toFixed(decimals)} MB`;
  if (bytes < 1024 * 1024 * 1024 * 1024)
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(decimals)} GB`;
  return `${(bytes / (1024 * 1024 * 1024 * 1024)).toFixed(decimals)} TB`;
}

export function formatDate(epoch: number): string {
  if (!epoch) return "—";
  const d = new Date(epoch * 1000);
  const yyyy = d.getFullYear();
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  const hh = String(d.getHours()).padStart(2, "0");
  const mi = String(d.getMinutes()).padStart(2, "0");
  return `${yyyy}-${mm}-${dd} ${hh}:${mi}`;
}

export function formatElapsed(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  const m = Math.floor(ms / 60_000);
  const s = Math.floor((ms % 60_000) / 1000);
  return `${m}m ${s}s`;
}

export function fileExtension(name: string): string {
  const i = name.lastIndexOf(".");
  if (i <= 0) return "";
  return name.slice(i + 1).toLowerCase();
}
