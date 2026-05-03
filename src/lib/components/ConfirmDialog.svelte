<script lang="ts">
  interface Props {
    title: string;
    message: string;
    confirmLabel?: string;
    danger?: boolean;
    onConfirm: () => void;
    onCancel: () => void;
  }

  let {
    title,
    message,
    confirmLabel = "Confirm",
    danger = false,
    onConfirm,
    onCancel,
  }: Props = $props();

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") onCancel();
    if (e.key === "Enter") onConfirm();
  }
</script>

<svelte:window onkeydown={onKey} />

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="overlay" onclick={onCancel} role="presentation">
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" tabindex="-1">
    <div class="d-title">{title}</div>
    <div class="d-message">{message}</div>
    <div class="d-actions">
      <button class="btn ghost" onclick={onCancel}>Cancel</button>
      <button class="btn" class:danger onclick={onConfirm}>{confirmLabel}</button>
    </div>
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
    z-index: 300;
  }
  .dialog {
    background: var(--bg-2);
    border: 1px solid var(--border-strong);
    border-radius: 8px;
    padding: 16px 20px;
    width: 360px;
    max-width: 90vw;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.6);
  }
  .d-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--text-strong);
    margin-bottom: 8px;
  }
  .d-message {
    font-size: 12.5px;
    color: var(--text-muted);
    line-height: 1.5;
    margin-bottom: 16px;
  }
  .d-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .btn {
    height: 28px;
    padding: 0 12px;
    border-radius: 4px;
    font-size: 12px;
    font-weight: 600;
    border: 1px solid transparent;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: var(--accent);
    color: #fff;
  }
  .btn:hover {
    background: var(--accent-hover);
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
  .btn.danger {
    background: var(--danger);
  }
  .btn.danger:hover {
    background: #d73a4a;
  }
</style>
