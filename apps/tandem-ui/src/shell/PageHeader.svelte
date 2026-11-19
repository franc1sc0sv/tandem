<script lang="ts">
  import type { Snippet } from "svelte";
  import { useStore } from "../lib/context";
  import { isMacPlatform } from "../lib/shortcuts";
  import Icon from "../ui/Icon.svelte";
  import Kbd from "../ui/Kbd.svelte";

  interface Props {
    title: string;
    /** El acceso a la palette («Search or run… ⌘K»), como en `V3-Desk`. */
    search?: boolean;
    actions?: Snippet;
  }

  let { title, search = true, actions }: Props = $props();
  const store = useStore();
  const combo = isMacPlatform() ? "⌘K" : "Ctrl+K";
</script>

<header class="head">
  <h1>{title}</h1>
  <span class="fill"></span>
  {#if actions}{@render actions()}{/if}
  {#if search}
    <button type="button" class="find" aria-haspopup="dialog" aria-keyshortcuts={isMacPlatform() ? "Meta+K" : "Control+K"} onclick={() => store.togglePalette(true)}>
      <Icon name="search" size={15} stroke={2} />Search or run…<span class="fill"></span><Kbd>{combo}</Kbd>
    </button>
  {/if}
</header>

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  h1 {
    margin: 0;
    font-size: 20px;
    font-weight: 650;
  }
  .fill {
    flex: 1;
  }
  /* Campo de búsqueda que abre la palette (V3-Desk). */
  .find {
    font: inherit;
    font-size: 13px;
    padding: 8px 14px;
    border-radius: 9px;
    border: 1px solid #31343b;
    background: #1b1c21;
    cursor: pointer;
    min-height: 36px;
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 260px;
    color: #9a9da6;
    box-sizing: border-box;
    text-align: left;
    transition:
      border-color 0.2s,
      color 0.2s;
  }
  .find:hover {
    border-color: #4a4e58;
    color: #c4c6cc;
  }
  .find:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
</style>
