<script lang="ts">
  interface MenuItem {
    label: string;
    action: () => void;
    danger?: boolean;
    disabled?: boolean;
    separator?: boolean;
  }

  interface Props {
    x: number;
    y: number;
    items: MenuItem[];
    onClose: () => void;
  }

  let { x, y, items, onClose }: Props = $props();

  let menu: HTMLDivElement | undefined = $state();
  let posX = $state(x);
  let posY = $state(y);

  function handleClickOutside(e: MouseEvent) {
    if (!menu) return;
    if (!menu.contains(e.target as Node)) onClose();
  }
  function handleKey(e: KeyboardEvent) {
    if (e.key === "Escape") onClose();
  }

  $effect(() => {
    if (!menu) return;
    const r = menu.getBoundingClientRect();
    if (x + r.width > window.innerWidth) posX = window.innerWidth - r.width - 4;
    if (y + r.height > window.innerHeight)
      posY = window.innerHeight - r.height - 4;
    window.addEventListener("mousedown", handleClickOutside, true);
    window.addEventListener("keydown", handleKey);
    return () => {
      window.removeEventListener("mousedown", handleClickOutside, true);
      window.removeEventListener("keydown", handleKey);
    };
  });
</script>

<div
  class="ctx-menu"
  bind:this={menu}
  style="left: {posX}px; top: {posY}px"
  role="menu"
  tabindex="-1"
>
  {#each items as item}
    {#if item.separator}
      <div class="sep"></div>
    {:else}
      <button
        class="item"
        class:danger={item.danger}
        disabled={item.disabled}
        onclick={() => {
          item.action();
          onClose();
        }}
        role="menuitem"
      >
        {item.label}
      </button>
    {/if}
  {/each}
</div>

<style>
  .ctx-menu {
    position: fixed;
    z-index: 200;
    background: var(--bg-surface);
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    padding: 4px;
    min-width: 180px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
  }
  .item {
    display: block;
    width: 100%;
    text-align: left;
    padding: 6px 12px;
    font-size: 12.5px;
    color: var(--text);
    border-radius: 3px;
  }
  .item:hover:not(:disabled) {
    background: var(--bg-active);
    color: var(--text-strong);
  }
  .item:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .item.danger {
    color: var(--danger);
  }
  .item.danger:hover {
    background: rgba(248, 81, 73, 0.18);
  }
  .sep {
    height: 1px;
    background: var(--border);
    margin: 4px 0;
  }
</style>
