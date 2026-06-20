import App from "./App.svelte";
import { mount } from "svelte";
import { getCurrentWindow } from "@tauri-apps/api/window";

const app = mount(App, { target: document.getElementById("app")! });

// Wire up the SE-corner resize grip: mousedown hands off to Tauri so the
// native window manager drives the drag.
const grip = document.querySelector<HTMLElement>(".resize-grip");
if (grip) {
  grip.addEventListener("mousedown", async (e) => {
    e.preventDefault();
    try {
      await getCurrentWindow().startResizing("SouthEast");
    } catch {
      // ignore — Tauri may not be available outside the Tauri runtime
    }
  });
}

// Edge + corner handles: 6px-wide bands on each side + 8px corners.
// These overlay the visible 4px border so the user can grab anywhere on
// the frame, not just exactly on the pixel line.
document.querySelectorAll<HTMLElement>(".edge-handle").forEach((el) => {
  el.addEventListener("mousedown", async (e) => {
    e.preventDefault();
    const edge = el.dataset.edge;
    if (!edge) return;
    try {
      await getCurrentWindow().startResizing(edge as any);
    } catch {
      // ignore
    }
  });
});

export default app;
