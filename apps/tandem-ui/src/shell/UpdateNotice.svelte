<script lang="ts">
  // "Update available: vX.Y.Z" (T-348): one button that runs "Update all" (the other machine
  // first, then this one), its progress and, if it failed, why. The result after the restart
  // is a notice ("Updated to …", from the store). Settings → Updates has the full row.
  import { useStore } from "../lib/context";
  import { updateNotice } from "../lib/updates";
  import Button from "../ui/Button.svelte";

  const store = useStore();
  /** Pressed and its reply has not come yet: it is not pressed twice. */
  let starting = $state(false);
  /** What the command itself rejected with (the view's own error comes from the run). */
  let error = $state<string | null>(null);
  const notice = $derived(store.update ? updateNotice(store.update, starting, error) : null);

  async function update() {
    if (!notice || notice.busy) return;
    error = null;
    starting = true;
    try {
      await store.updateAll();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      starting = false;
    }
  }
</script>

{#if notice}
  <section class="upd" class:bad={notice.failed} aria-label="Update">
    <div class="t"><span class="dot"></span>{notice.title}</div>
    <p class="d" role="status">{notice.detail}</p>
    <Button icon="download" variant="pri" size="sm" style="--btn-h:30px;--btn-px:12px;--btn-gap:7px;width:100%;justify-content:center" disabled={notice.busy} onclick={update}>{notice.action}</Button>
  </section>
{/if}

<style>
  .upd {
    margin: 0 0 8px;
    padding: 11px 12px 12px;
    border: 1px solid #1f2126;
    border-radius: 10px;
    background: #121317;
    display: flex;
    flex-direction: column;
    gap: 7px;
    animation: fd 0.3s;
  }
  .t {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12.5px;
    font-weight: 600;
    color: #e8e8eb;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
    background: #7aa7e6;
  }
  .bad .dot {
    background: var(--amber);
  }
  .d {
    margin: 0;
    font-size: 11.5px;
    line-height: 1.45;
    color: #9a9da6;
  }
  .bad .d {
    color: var(--amber);
  }
</style>
