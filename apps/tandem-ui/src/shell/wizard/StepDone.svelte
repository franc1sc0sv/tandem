<script lang="ts">
  // Paso 6 (`F-Asis6`): el resumen de lo que se comprobó de verdad y el comando que agrega
  // Tandem a Claude Code.
  import { useStore } from "../../lib/context";
  import { MCP_COMMAND, deskTitle, summaryRows } from "../../lib/wizard";
  import Icon from "../../ui/Icon.svelte";

  const store = useStore();
  const desk = $derived(store.desk);
  const title = $derived(deskTitle(desk));
  const rows = $derived(summaryRows(desk, { crossed: store.wizardCrossed }));
  const hereColor = $derived(desk.machines[desk.here]?.color ?? "#7aa7e6");
  const peerColor = $derived(desk.machines[desk.here === "a" ? "b" : "a"]?.color ?? "#a99be8");
  let copied = $state(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(MCP_COMMAND);
      copied = true;
      store.toast("Comando copiado");
      setTimeout(() => (copied = false), 1800);
    } catch {
      store.toast("No se pudo copiar: selecciónalo y cópialo a mano", "bad");
    }
  }
</script>

<svg width="76" height="76" viewBox="0 0 80 80" aria-hidden="true"
  ><circle class="wz-ring" cx="40" cy="40" r="36" fill="none" stroke="#6cc59a" stroke-width="3"></circle><path
    class="wz-tickd"
    d="M25 41l10 10 20-22"
    fill="none"
    stroke="#6cc59a"
    stroke-width="4"
    stroke-linecap="round"
    stroke-linejoin="round"></path></svg
>
<div class="wz-head">
  <h1 style="font-size:28px">Tu puesto está listo</h1>
  <p class="wz-mut wz-lead" style="max-width:600px">Esto se comprobó de verdad, no se supuso.</p>
</div>
<section aria-label="Resumen" class="wz-list">
  <div class="wz-sum first" style="animation-delay:.3s">
    <span style="width:8px;height:8px;border-radius:50%;background:{hereColor}"></span>{title.here}{title.peer ? " y" : ""}
    {#if title.peer}<span style="width:8px;height:8px;border-radius:50%;background:{peerColor}"></span>{title.peer} en «{title.desk}»{:else}en «{title.desk}»{/if}<span class="wz-fill"></span><span class="wz-mut wz-sm">{title.peer ? "2 máquinas" : "1 máquina"}</span>
  </div>
  {#each rows as r, i (r.id)}
    <div class="wz-sum" style="animation-delay:{(0.5 + 0.2 * i).toFixed(1)}s" data-ok={r.ok}>
      {r.label}<span class="wz-fill"></span><span class="wz-sm" class:wz-mono={r.mono} class:wz-mut={!r.mono} style={r.ok ? "" : "color:#e6c48f"}>{r.note}</span>
    </div>
  {/each}
</section>
<div class="wz-mcp">
  <b style="font-weight:600;font-size:13.5px">Para usarlo desde Claude Code</b>
  <span style="display:flex;align-items:center;gap:10px"
    ><span class="wz-mono" style="font-size:12.5px;color:#c4c6cc;flex:1">{MCP_COMMAND}</span><button type="button" class="wz-copy" aria-label="Copiar comando" title="Copiar" onclick={copy}
      ><Icon name={copied ? "check" : "copy"} size={15} color={copied ? "#6cc59a" : "currentColor"} /></button
    ></span
  >
</div>
