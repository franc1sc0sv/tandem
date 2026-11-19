<script lang="ts">
  // Step 3 (`F-Setup3`): the macOS permissions `tandemd` needs, requested and truly verified
  // (T-090): the agent rereads them by itself and the card turns to granted.
  import { onMount } from "svelte";
  import { useStore } from "../../lib/context";
  import { permCards } from "../../lib/wizard";
  import Button from "../../ui/Button.svelte";
  import Icon from "../../ui/Icon.svelte";

  interface Props {
    /** The user already opened some panel: offers to restart the agent. */
    onOpened: () => void;
  }
  let { onOpened }: Props = $props();

  const store = useStore();
  const perms = $derived(store.desk.permissions);
  const cards = $derived(permCards(perms.rows));
  const here = $derived(store.desk.machines[store.desk.here]);

  // Entering the step with pending permissions requests them: macOS shows its dialog in the name of tandemd.
  onMount(() => {
    if (perms.rows.some((r) => r.state === "missing")) void store.requestPermissions();
  });

  async function grant(id: (typeof cards)[number]["id"]) {
    onOpened();
    await store.openPermission(id);
  }
</script>

<div class="wz-head">
  <span class="wz-mut wz-sm">Step 3 of 6{here ? ` · ${here.name}` : ""}</span>
  <h1>{perms.needed ? "macOS permissions" : "Permissions"}</h1>
  {#if perms.needed}
    <p class="wz-mut wz-lead" style="max-width:600px">
      They are granted to <span class="wz-mono" style="font-size:12.5px;color:#c4c6cc">tandemd</span>, not to the window. Tandem detects them on its own as soon as you grant them: no need to come back here.
    </p>
  {/if}
</div>
{#if perms.needed}
  {#each cards as p (p.id)}
    <div class="wz-perm" class:granted={p.granted}>
      <span class="wz-dot {p.granted ? 'got' : p.missing ? 'wait' : ''}"></span>
      <div style="display:flex;flex-direction:column;gap:5px;flex:1;min-width:0">
        <b style="font-weight:600">{p.title}</b>
        <span class="wz-mut" style="font-size:13px;line-height:1.5">{p.why}</span>
        <span style="font-size:12.5px;color:{p.granted ? '#9fd8b9' : p.missing ? '#e6c48f' : '#9a9da6'}"
          >{p.granted ? "Granted · checked just now" : p.missing ? "Waiting… it is checked automatically" : "Not checked · macOS asks the first time it is used"}</span
        >
      </div>
      {#if !p.granted}
        <Button icon="external" size="sm" motion="lift" class="wz-sbtn" onclick={() => grant(p.id)}>Open settings</Button>
      {:else}
        <span class="wz-tick"><Icon name="check" size={17} stroke={2.6} color="#6cc59a" /></span>
      {/if}
    </div>
  {/each}
{:else}
  <div class="wz-perm granted">
    <span class="wz-dot got"></span>
    <div style="display:flex;flex-direction:column;gap:5px;flex:1;min-width:0">
      <b style="font-weight:600">This machine needs no permissions</b>
      <span class="wz-mut" style="font-size:13px;line-height:1.5">Windows lets you capture and inject the keyboard and mouse without asking for anything.</span>
    </div>
    <span class="wz-tick"><Icon name="check" size={17} stroke={2.6} color="#6cc59a" /></span>
  </div>
{/if}
