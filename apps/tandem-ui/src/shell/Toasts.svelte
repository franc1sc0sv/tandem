<script lang="ts">
  import { useStore } from "../lib/context";
  import { leave } from "../lib/motion";
  import { TONE_COLOR } from "../lib/notices";
  import { TOAST_MS } from "../lib/store/store.svelte";
  import Icon from "../ui/Icon.svelte";

  const store = useStore();
</script>

<!-- Notices inside the app (F-Notices): top right, they stack and go away by themselves in 6 s
     except those that ask for a decision. -->
<div class="toasts" role="region" aria-label="Notices">
  {#each store.toasts as t (t.id)}
    <div class="toast" role="status" out:leave>
      <span class="dot" style:background={TONE_COLOR[t.tone]}></span>
      <div class="col">
        <b>{t.text}</b>
        {#if t.body}<span class="body">{t.body}</span>{/if}
        {#if t.progress}
          <span class="bar" role="progressbar" aria-label="Progress" aria-valuemin="0" aria-valuemax="100" aria-valuenow={t.progress.total > 0 ? Math.floor((t.progress.done / t.progress.total) * 100) : 0}>
            <span class="fill" style:width="{t.progress.total > 0 ? (t.progress.done / t.progress.total) * 100 : 0}%"></span>
          </span>
        {/if}
        {#if t.action}
          <span class="acts">
            <button
              type="button"
              class="act"
              onclick={() => {
                t.action?.run();
                store.dismiss(t.id);
              }}>{t.action.label}</button
            >
          </span>
        {/if}
      </div>
      <button type="button" class="x" aria-label="Close" onclick={() => store.dismiss(t.id)}><Icon name="x" size={14} stroke={2.2} /></button>
      {#if t.auto}<span class="life" style:animation-duration="{TOAST_MS}ms"></span>{/if}
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    top: 20px;
    right: 20px;
    z-index: 30;
    display: flex;
    flex-direction: column;
    gap: 10px;
    align-items: flex-end;
  }
  .toast {
    position: relative;
    overflow: hidden;
    width: 390px; /* 360 of content + padding and border, like the mockup */
    max-width: 100%;
    box-sizing: border-box;
    border-radius: 12px;
    background: #1b1c21;
    border: 1px solid #31343b;
    box-shadow: 0 14px 34px rgba(0, 0, 0, 0.45);
    padding: 12px 14px;
    display: flex;
    gap: 12px;
    align-items: flex-start;
    font-size: 14px;
    animation: sl 0.45s cubic-bezier(0.2, 0.9, 0.25, 1.15);
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    margin-top: 5px;
    flex: none;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 6px;
    flex: 1;
    min-width: 0;
  }
  b {
    font-weight: 600;
    font-size: 13.5px;
  }
  .body {
    color: #9a9da6;
    font-size: 12.5px;
    line-height: 1.45;
  }
  .acts {
    display: flex;
    gap: 6px;
  }
  .act {
    font: inherit;
    font-size: 12.5px;
    padding: 5px 10px;
    border-radius: 8px;
    border: 1px solid #31343b;
    background: transparent;
    color: #e8e8eb;
    cursor: pointer;
    min-height: 30px;
    transition:
      border-color 0.2s,
      background 0.2s,
      transform 0.2s;
  }
  .act:hover {
    border-color: #4a4e58;
    background: #25262c;
    transform: translateY(-1px);
  }
  .act:focus-visible,
  .x:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
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
  .bar {
    display: block;
    height: 4px;
    margin-top: 8px;
    border-radius: 2px;
    background: #2a2c33;
    overflow: hidden;
  }
  .bar .fill {
    display: block;
    height: 100%;
    background: #7aa7e6;
    transition: width 0.3s ease-out;
  }
  .life {
    position: absolute;
    left: 0;
    bottom: 0;
    height: 2px;
    background: #7aa7e6;
    animation: life 6s linear forwards;
  }
  @keyframes sl {
    from {
      transform: translateX(40px);
      opacity: 0;
    }
    to {
      transform: none;
      opacity: 1;
    }
  }
  @keyframes life {
    from {
      width: 100%;
    }
    to {
      width: 0;
    }
  }
</style>
