import type { FileItem } from "./ipc";

let selected = $state<FileItem[]>([]);
let _renameFiles = $state<Array<{ name: string; path: string }>>([]);

export function getSelection(): FileItem[] {
  return selected;
}

export function setSelection(items: FileItem[]): void {
  selected = items;
}

export function clearSelection(): void {
  selected = [];
}

export function getRenameFiles(): Array<{ name: string; path: string }> {
  return _renameFiles;
}

// Selection model: track only explicitly-unchecked paths. Default is "checked"
// so incoming files apply immediately without waiting for an $effect to run
// after the first render (which previously caused "Apply 0" on mount).
let _renameUnchecked = $state<Set<string>>(new Set());

export function setRenameFiles(
  files: Array<{ name: string; path: string }>,
): void {
  _renameFiles = files;
  // New file batch — clear any stale exclusions so all incoming files are checked.
  _renameUnchecked = new Set();
}

export function clearRenameFiles(): void {
  _renameFiles = [];
  _renameUnchecked = new Set();
}

export function isRenameChecked(path: string): boolean {
  return !_renameUnchecked.has(path);
}

export function toggleRenameChecked(path: string): void {
  const next = new Set(_renameUnchecked);
  if (next.has(path)) next.delete(path);
  else next.add(path);
  _renameUnchecked = next;
}

export function setAllRenameChecked(checked: boolean): void {
  if (checked) {
    _renameUnchecked = new Set();
  } else {
    _renameUnchecked = new Set(_renameFiles.map((f) => f.path));
  }
}

export function getRenameCheckedPaths(): string[] {
  return _renameFiles.map((f) => f.path).filter((p) => !_renameUnchecked.has(p));
}
