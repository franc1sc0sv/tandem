<script lang="ts">
  // Settings (`F-Settings`): pestañas con íconos. Desk and machines, Shortcuts, Keyboard and mouse,
  // Actualizaciones (T-122) y Avanzado. Todo lo que se edita llega a the other machine con el
  // desk compartido (D4); el nombre del desk todavía no viaja, así que solo se muestra.
  import { onMount } from "svelte";
  import { useStore } from "../lib/context";
  import { SETTINGS_TABS, nextTab, type SettingsTab } from "../lib/settings";
  import { WIZARD_PAIR } from "../lib/store/store.svelte";
  import type { FilesView } from "../lib/store/types";
  import PageHeader from "../shell/PageHeader.svelte";
  import Button from "../ui/Button.svelte";
  import Icon from "../ui/Icon.svelte";
  import Switch from "../ui/Switch.svelte";
  import Shortcuts from "./Shortcuts.svelte";
  import Updates from "./Updates.svelte";

  const store = useStore();
  const desk = $derived(store.desk);
  const machines = $derived((["a", "b"] as const).flatMap((id) => (desk.machines[id] ? [desk.machines[id]] : [])));

  let tab = $state<SettingsTab>("desk");
  let files = $state<FilesView | null>(null);
  let renaming = $state<string | null>(null);
  let draft = $state("");
  let leaving = $state(false);

  // Los botones de los mockups de settings miden 34 px (12.5 px de texto, ícono de 15).
  const B34 = "--btn-h:34px;--btn-px:13px;--btn-gap:7px";

  onMount(async () => {
    try {
      files = await store.files();
    } catch {
      files = null;
    }
  });

  function onTabKey(e: KeyboardEvent) {
    const n = nextTab(tab, e.key);
    if (!n) return;
    e.preventDefault();
    tab = n;
    document.getElementById(`tab-${n}`)?.focus();
  }

  function rename(id: string, name: string) {
    renaming = id;
    draft = name;
  }

  async function commitName(id: "a" | "b") {
    const name = draft.trim();
    renaming = null;
    if (name && name !== desk.machines[id]?.name) await store.editMachine(id, { name });
  }

  async function commitDeskName() {
    const name = draft.trim();
    renaming = null;
    if (name !== desk.deskName) await store.setDeskName(name);
  }

  async function copyMcp() {
    try {
      await navigator.clipboard.writeText("tandem mcp");
      store.toast("Command copied");
    } catch {
      store.toast("Could not copy the command", "bad");
    }
  }

  async function leave() {
    if (!leaving) {
      leaving = true;
      return;
    }
    leaving = false;
    await store.pairAction("unpair");
  }
</script>

<div class="page">
  <PageHeader title="Settings" search={false} />
  <div class="cols">
    <div class="tabs" role="tablist" aria-label="Sections" aria-orientation="vertical" tabindex="-1" onkeydown={onTabKey}>
      {#each SETTINGS_TABS as t (t.id)}
        <button
          type="button"
          role="tab"
          id="tab-{t.id}"
          class="tab"
          aria-selected={tab === t.id}
          aria-controls="pane"
          tabindex={tab === t.id ? 0 : -1}
          onclick={() => (tab = t.id)}><Icon name={t.icon} size={16} stroke={1.8} />{t.name}</button
        >
      {/each}
    </div>
    <div class="sec" id="pane" role="tabpanel" aria-labelledby="tab-{tab}">
      {#key tab}
        <div class="pane">
          {#if tab === "desk"}
            <div class="row first">
              <span class="fill">Desk name</span>
              {#if renaming === "desk"}
                <!-- svelte-ignore a11y_autofocus -->
                <input
                  class="name fill-in"
                  bind:value={draft}
                  aria-label="Desk name"
                  autofocus
                  onkeydown={(e) => {
                    if (e.key === "Enter") void commitDeskName();
                    if (e.key === "Escape") renaming = null;
                  }}
                  onblur={() => void commitDeskName()}
                />
              {:else}
                <button type="button" class="mut deskname" data-testid="desk-name" title="Change the desk name" onclick={() => rename("desk", desk.deskName)}>{desk.deskName}</button>
              {/if}
            </div>
            {#each machines as m (m.id)}
              <div class="row">
                <span class="dot" style:background={m.color}></span>
                {#if renaming === m.id}
                  <!-- svelte-ignore a11y_autofocus -->
                  <input
                    class="name fill-in"
                    bind:value={draft}
                    aria-label="Name of {m.name}"
                    autofocus
                    onkeydown={(e) => {
                      if (e.key === "Enter") void commitName(m.id);
                      if (e.key === "Escape") renaming = null;
                    }}
                    onblur={() => void commitName(m.id)}
                  />
                {:else}
                  <span class="fill">{m.name}{#if m.here}&nbsp;<span class="mut sub">· this machine</span>{/if}</span>
                {/if}
                <Button icon="edit" size="sm" iconOnly label="Rename" style={B34} onclick={() => rename(m.id, m.name)} />
              </div>
            {/each}
            <div class="row">
              <span class="fill">Change the other machine</span>
              <Button icon="refresh" variant="pri" size="sm" style={B34} onclick={() => store.openWizard(WIZARD_PAIR)}>Pair again</Button>
            </div>
            <div class="row">
              <span class="fill">Run setup again</span>
              <Button icon="refresh" size="sm" motion="spin" style={B34} onclick={() => store.openWizard(0)}>Open</Button>
            </div>
            <div class="row">
              <span class="fill col">Leave desk<span class="mut sub">This machine forgets the other machines’ keys and, if they are connected, they forget its key; if not, they do once they leave too.</span></span>
              <Button icon="logout" variant="danger" size="sm" style={B34} onclick={leave}>{leaving ? "Confirm" : "Leave"}</Button>
            </div>
          {:else if tab === "shortcuts"}
            <Shortcuts />
          {:else if tab === "teclado"}
            <div class="row first">
              <span class="fill col">Swap Cmd and Ctrl between macOS and Windows<span class="mut sub">Copy is ⌘C on the Mac and Ctrl+C on the PC, with the same finger.</span></span>
              <Switch checked={desk.prefs.cmdCtrl} label="Swap Cmd and Ctrl" onchange={(on) => store.setPrefs({ cmdCtrl: on, dragFiles: desk.prefs.dragFiles })} />
            </div>
            <div class="row">
              <span class="fill col">Drag between machines<span class="mut sub">With a button held down, the cursor does not cross the edge.</span></span>
              <Switch checked={desk.prefs.dragFiles} label="Drag between machines" onchange={(on) => store.setPrefs({ cmdCtrl: desk.prefs.cmdCtrl, dragFiles: on })} />
            </div>
            <div class="row">
              <span class="fill">Shared clipboard</span>
              <Switch
                checked={desk.prefs.clipboard}
                label="Shared clipboard"
                onchange={async (on) => {
                  for (const m of machines) await store.setModule(m.id, "clipboard", on);
                }}
              />
            </div>
          {:else if tab === "upd"}
            <Updates />
          {:else}
            <div class="row first">
              <span class="fill col">Configuration<span class="mono mut path">{files?.config ?? "—"}</span></span>
              <Button icon="file" size="sm" style={B34} onclick={() => store.openFiles("config")}>Show file</Button>
            </div>
            <div class="row">
              <span class="fill">Agent logs</span>
              <Button icon="folder" size="sm" style={B34} onclick={() => store.openFiles("logs")}>Open folder</Button>
            </div>
            <div class="row">
              <span class="fill col">MCP server<span class="mono mut path">tandem mcp</span></span>
              <Button icon="copy" size="sm" style={B34} onclick={copyMcp}>Copy command</Button>
            </div>
            <div class="row">
              <span class="fill">Trouble states (reference view)</span>
              <Button icon="eye" size="sm" style={B34} onclick={() => store.go({ screen: "states" })}>View</Button>
            </div>
          {/if}
        </div>
      {/key}
    </div>
  </div>
</div>

<style>
  .deskname {
    all: unset;
    cursor: text;
  }
  .deskname:focus-visible {
    outline: 2px solid currentColor;
    outline-offset: 2px;
  }
  .page {
    display: flex;
    flex-direction: column;
    gap: 18px;
    flex: 1;
  }
  .cols {
    display: flex;
    gap: 28px;
    flex-wrap: wrap;
    align-items: flex-start;
  }
  .tabs {
    flex: 0 0 200px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .tab {
    font: inherit;
    text-align: left;
    font-size: 13.5px;
    padding: 9px 12px;
    border-radius: 9px;
    border: 0;
    background: transparent;
    color: #a3a6af;
    cursor: pointer;
    min-height: 38px;
    display: flex;
    align-items: center;
    gap: 10px;
    transition:
      background 0.2s,
      color 0.2s;
  }
  .tab :global(svg) {
    flex: none;
    opacity: 0.8;
  }
  .tab:hover {
    background: #17181c;
    color: #e8e8eb;
  }
  .tab[aria-selected="true"] {
    background: #1c1d22;
    color: #e8e8eb;
  }
  .tab:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .sec {
    flex: 1 1 560px;
    min-width: 0;
    max-width: 760px;
  }
  .pane {
    animation: fd 0.3s ease-out;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 14px 0;
    border-top: 1px solid #1f2126;
    font-size: 13.5px;
    flex-wrap: wrap;
    transition: background 0.2s;
  }
  .row:hover {
    background: rgba(255, 255, 255, 0.015);
  }
  .row.first {
    border-top: 0;
    /* La altura de la fila del mockup, que trae un campo aquí; el nombre es texto porque no se edita en esta fila. */
    min-height: 34px;
  }
  .fill {
    flex: 1;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .sub {
    font-size: 12.5px;
  }
  .path {
    font-size: 12px;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }
  .name {
    font: inherit;
    font-size: 13.5px;
    color: #e8e8eb;
    background: #111215;
    border: 1px solid #31343b;
    border-radius: 9px;
    padding: 8px 12px;
  }
  .name:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .fill-in {
    flex: 1;
  }
</style>
