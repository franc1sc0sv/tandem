<script lang="ts">
  import { useStore } from "../lib/context";
  import { enter, leave } from "../lib/motion";
  import Icon from "../ui/Icon.svelte";

  const store = useStore();
</script>

<!-- Avisos cortos de lo que pasó con una acción (F-Avisos). -->
<div class="toasts" role="status" aria-live="polite">
  {#each store.toasts as t (t.id)}
    <div class="toast {t.tone}" in:enter={{ x: 24, duration: 350 }} out:leave>
      <Icon name={t.tone === "ok" ? "check" : "warn"} size={16} stroke={2} color={t.tone === "ok" ? "#6cc59a" : "#e07a6f"} />
      <span class="tx">{t.text}</span>
      <button type="button" class="x" aria-label="Cerrar aviso" onclick={() => store.dismiss(t.id)}><Icon name="x" size={14} stroke={2.2} /></button>
      <span class="life"></span>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 20px;
    bottom: 20px;
    z-index: 30;
    display: flex;
    flex-direction: column;
    gap: 10px;
    align-items: flex-end;
  }
  .toast {
    position: relative;
    overflow: hidden;
    width: 360px;
    max-width: 100%;
    box-sizing: border-box;
    border-radius: 12px;
    background: #1b1c21;
    border: 1px solid #31343b;
    box-shadow: 0 14px 34px rgba(0, 0, 0, 0.45);
    padding: 12px 14px;
    display: flex;
    gap: 12px;
    align-items: center;
    font-size: 13px;
  }
  .tx {
    flex: 1;
  }
  .x {
    font: inherit;
    border: 0;
    background: transparent;
    color: #9a9da6;
    cursor: pointer;
    width: 28px;
    height: 28px;
    border-radius: 7px;
    flex: none;
    display: grid;
    place-items: center;
    transition:
      background 0.2s,
      color 0.2s,
      transform 0.25s;
  }
  .x:hover {
    background: #25272d;
    color: #e8e8eb;
    transform: rotate(90deg);
  }
  .x:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .life {
    position: absolute;
    left: 0;
    bottom: 0;
    height: 2px;
    background: #7aa7e6;
    animation: toast-life 4s linear forwards;
  }
  .bad .life {
    background: #e07a6f;
  }
  @keyframes toast-life {
    from {
      width: 100%;
    }
    to {
      width: 0;
    }
  }
</style>
