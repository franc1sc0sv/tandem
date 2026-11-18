<script lang="ts">
  // Paso 4 (`F-Asis4`): teclado y mouse sin dueño. Cada máquina lista lo que tiene y marca
  // «Actividad detectada» cuando recibe una tecla o un movimiento: lo oye el agente (cada
  // máquina por su lado) y, en esta, también la ventana.
  import { onDestroy, onMount } from "svelte";
  import { useStore } from "../../lib/context";
  import { NO_ACTIVITY, deviceRows, heard, type Activity } from "../../lib/wizard";
  import type { Machine } from "../../lib/model";

  const store = useStore();
  const desk = $derived(store.desk);
  const ids: Machine[] = $derived(desk.here === "a" ? ["a", "b"] : ["b", "a"]);
  let activity: Activity = $state({ ...NO_ACTIVITY });
  let stop: (() => void) | null = null;
  let gone = false;

  onMount(() => {
    store.watchKeyProbe((m) => (activity = heard(activity, m))).then(
      (off) => (gone ? off() : (stop = off)),
      () => {
        // Sin sonda del agente: queda la actividad que ve esta ventana.
      },
    );
  });
  onDestroy(() => {
    gone = true;
    stop?.();
  });

  const here = () => (activity = heard(activity, desk.here));
</script>

<svelte:window onkeydown={here} onpointermove={here} />

<div class="wz-head">
  <span class="wz-mut wz-sm">Paso 4 de 6</span>
  <h1>Teclado y mouse</h1>
  <p class="wz-mut wz-lead" style="max-width:600px">Usa el teclado y el mouse de cualquier máquina. Tandem sigue al que estés usando.</p>
</div>
<div style="display:flex;gap:16px;flex-wrap:wrap;align-items:stretch">
  {#each ids as id, n (id)}
    {@const m = desk.machines[id]}
    {@const devs = deviceRows(m)}
    {#if n === 1}
      <svg width="70" height="40" viewBox="0 0 70 40" aria-hidden="true" style="align-self:center;flex:none"
        ><path class="wz-flow" d="M2 14h66" stroke={desk.machines[ids[0]]?.color ?? "#7aa7e6"} stroke-width="2" fill="none"></path><path class="wz-flow" d="M68 26H2" stroke={m?.color ?? "#a99be8"} stroke-width="2" fill="none"></path></svg
      >
    {/if}
    <div class="wz-mc" style="animation-delay:{n === 0 ? 0 : 0.1}s" role="group" aria-label={m?.name ?? "Máquina"}>
      <span style="display:flex;align-items:center;gap:8px;font-weight:600"><span style="width:8px;height:8px;border-radius:50%;background:{m?.color ?? '#9a9da6'}"></span>{m?.name ?? "Otra máquina"}</span>
      {#each devs as d, i (d.name)}
        <div class="wz-dev" style="animation-delay:{((n === 0 ? 0.2 : 0.8) + 0.2 * i).toFixed(1)}s">
          <span class="wz-ok"
            ><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="#6cc59a" stroke-width="3" aria-hidden="true"><path d="m4 12 5 5L20 6"></path></svg></span
          >{d.name}{#if d.via}&nbsp;<span class="wz-mut wz-xs">{d.via}</span>{/if}
        </div>
      {:else}
        <div class="wz-dev" style="animation-delay:0.8s"><span class="wz-off"></span><span class="wz-mut">Sin teclado ni mouse ahora</span></div>
        <p class="wz-mut wz-note" style="animation:wz-up 0.4s 1s both">Si conectas el hub aquí, lo detecta solo.</p>
      {/each}
      {#if activity[id]}
        <div class="wz-dev" style="color:#9fd8b9"><span class="wz-ok"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="#6cc59a" stroke-width="3" aria-hidden="true"><path d="m4 12 5 5L20 6"></path></svg></span>Actividad detectada</div>
      {/if}
    </div>
  {/each}
</div>
