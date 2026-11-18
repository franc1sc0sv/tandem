<script lang="ts">
  import { tick } from "svelte";
  import { buildCommands, filterCommands, flatten, move, type Command } from "../lib/commands";
  import { useStore } from "../lib/context";
  import { enter, leave } from "../lib/motion";
  import { isMacPlatform } from "../lib/shortcuts";
  import Icon from "../ui/Icon.svelte";

  const store = useStore();
  const mac = isMacPlatform();

  let query = $state("");
  let index = $state(0);
  let input: HTMLInputElement | undefined = $state();
  let list: HTMLDivElement | undefined = $state();
  let returnFocus: HTMLElement | null = null;

  const commands = $derived(buildCommands(store.desk, store, mac));
  const groups = $derived(filterCommands(commands, query));
  const flat = $derived(flatten(groups));

  $effect(() => {
    if (store.paletteOpen) {
      returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      query = "";
      index = 0;
      void tick().then(() => input?.focus());
    } else if (returnFocus) {
      returnFocus.focus();
      returnFocus = null;
    }
  });

  function close() {
    store.togglePalette(false);
  }

  function run(c: Command | undefined) {
    if (!c) return;
    close();
    void c.run();
  }

  async function onkeydown(e: KeyboardEvent) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      index = move(index, e.key === "ArrowDown" ? 1 : -1, flat.length);
      await tick();
      list?.querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: "nearest" });
    } else if (e.key === "Enter") {
      e.preventDefault();
      run(flat[index]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "Tab") {
      // La paleta es modal: el foco se queda en la búsqueda.
      e.preventDefault();
    }
  }

  function oninput() {
    index = 0;
  }
</script>

{#if store.paletteOpen}
  <div class="veil" role="presentation" onclick={close} in:enter={{ duration: 200 }} out:leave></div>
  <div class="wrap">
    <div role="dialog" aria-modal="true" aria-label="Command palette" class="pal" out:leave>
      <div class="top">
        <Icon name="search" size={18} stroke={2} color="#9a9da6" />
        <input
          bind:this={input}
          bind:value={query}
          aria-label="Search"
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-activedescendant={flat[index] ? `cmd-${flat[index].id}` : undefined}
          aria-autocomplete="list"
          placeholder="Scene, machine, job or action…"
          autocomplete="off"
          spellcheck="false"
          {onkeydown}
          {oninput}
        />
        <span class="kbd">esc</span>
      </div>
      <div class="list" id="palette-list" role="listbox" aria-label="Commands" bind:this={list}>
        {#each groups as g (g.name)}
          <div class="grp" role="presentation">{g.name}</div>
          {#each g.items as c (c.id)}
            {@const i = flat.indexOf(c)}
            <button
              type="button"
              class="it"
              id="cmd-{c.id}"
              role="option"
              aria-selected={i === index}
              tabindex="-1"
              onmousemove={() => (index = i)}
              onclick={() => run(c)}
              ><span class="dot" style="background:{c.color}"></span><span class="t">{c.title}</span>{#if c.detail}<span class="mut d">{c.detail}</span>{/if}{#if c.key}<span
                  class="kbd">{c.key}</span
                >{/if}</button
            >
          {/each}
        {/each}
        {#if flat.length === 0}
          <div class="mut empty">Nothing matches “{query}”.</div>
        {/if}
      </div>
      <div class="foot mut"><span><span class="kbd">↑↓</span> move</span><span><span class="kbd">↵</span> run</span><span class="fill"></span><span>Type to filter</span></div>
    </div>
  </div>
{/if}

<style>
  /* F-Palette: velo, diálogo de 640 px y lista agrupada. */
  .veil {
    position: fixed;
    inset: 0;
    background: rgba(8, 9, 11, 0.55);
    z-index: 20;
  }
  .wrap {
    position: fixed;
    inset: 0;
    z-index: 21;
    box-sizing: border-box;
    padding: 90px 24px 24px;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    pointer-events: none;
  }
  .pal {
    pointer-events: auto;
    position: relative;
    width: min(640px, 100%);
    border-radius: 16px;
    background: #17181c;
    border: 1px solid #31343b;
    box-shadow: 0 30px 70px rgba(0, 0, 0, 0.55);
    overflow: hidden;
    font-size: 14px;
    animation: pal-pop 0.25s cubic-bezier(0.2, 0.9, 0.3, 1.1);
  }
  .top {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 14px 16px;
    border-bottom: 1px solid #25272d;
  }
  input {
    flex: 1;
    font: inherit;
    font-size: 15px;
    color: #e8e8eb;
    background: transparent;
    border: 0;
    outline: none;
  }
  .list {
    max-height: 520px;
    overflow: auto;
    padding: 6px;
  }
  .grp {
    font-size: 11px;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: #80838d;
    padding: 10px 12px 4px;
  }
  .it {
    font: inherit;
    width: 100%;
    text-align: left;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border-radius: 9px;
    border: 0;
    background: transparent;
    color: #e8e8eb;
    cursor: pointer;
    min-height: 42px;
    font-size: 13.5px;
    animation: pal-up 0.2s ease-out both;
  }
  .it:hover {
    background: #1f2026;
  }
  .it[aria-selected="true"] {
    background: #22324a;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
  }
  .t {
    flex: 1;
  }
  .d {
    font-size: 12px;
  }
  .kbd {
    font-family: "JetBrains Mono", monospace;
    font-size: 11px;
    color: #9a9da6;
    border: 1px solid #31343b;
    border-radius: 5px;
    padding: 1px 6px;
  }
  .empty {
    padding: 24px;
    text-align: center;
    font-size: 13px;
  }
  .foot {
    display: flex;
    gap: 14px;
    padding: 10px 16px;
    border-top: 1px solid #25272d;
    font-size: 12px;
  }
  .fill {
    flex: 1;
  }
  @keyframes pal-pop {
    from {
      opacity: 0;
      transform: translateY(-10px) scale(0.98);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  @keyframes pal-up {
    from {
      opacity: 0;
      transform: translateY(4px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
</style>
