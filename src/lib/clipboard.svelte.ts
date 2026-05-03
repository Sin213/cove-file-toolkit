let _state = $state<{ paths: string[]; mode: "copy" | "cut" | null }>({ paths: [], mode: null });

export function setClipboard(paths: string[], mode: "copy" | "cut") {
  _state = { paths, mode };
}

export function clearClipboard() {
  _state = { paths: [], mode: null };
}

export function getClipboard() {
  return _state;
}

export function hasClipboard() {
  return _state.paths.length > 0 && _state.mode !== null;
}
