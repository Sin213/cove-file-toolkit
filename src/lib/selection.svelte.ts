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

export function setRenameFiles(
  files: Array<{ name: string; path: string }>,
): void {
  _renameFiles = files;
}

export function clearRenameFiles(): void {
  _renameFiles = [];
}
