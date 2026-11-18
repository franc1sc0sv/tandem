<script lang="ts">
  // Paso 5 (`F-Asis5`): el acomodo que Tandem leyó, que se puede mover arrastrando, y la
  // prueba de cruce (T-117): se lleva el mouse por el borde marcado y se juzga con el cruce
  // que informa el motor de entrada.
  import { onDestroy } from "svelte";
  import { exitSide, fillOf, mainOf, placeOther } from "../../lib/acomodo";
  import { useStore } from "../../lib/context";
  import { CROSS_TIMEOUT_MS, onSample, onTick, remaining, start, type CrossPhase } from "../../lib/crossing";
  import { SKETCH, arrowSide, crossVerdict, dropSide } from "../../lib/wizard";
  import type { Side } from "../../lib/model";
  import Button from "../../ui/Button.svelte";
  import Icon from "../../ui/Icon.svelte";

  const store = useStore();
  const desk = $derived(store.desk);
  const here = $derived(desk.here);
  const peer = $derived(here === "a" ? "b" : "a");
  const hereM = $derived(desk.machines[here]);
  const peerM = $derived(desk.machines[peer]);
  const target = $derived(mainOf(desk.layout.screens, peer));
  const expected: Side = $derived(exitSide(desk.layout.arrangement, here));
  const sketch = $derived(SKETCH[expected]);
  const limit = store.crossTimeoutMs ?? CROSS_TIMEOUT_MS;
  const seconds = Math.round(CROSS_TIMEOUT_MS / 1000);

  /** `null` = sin probar todavía. */
  let phase = $state<CrossPhase | null>(null) as CrossPhase | null;
  let now = $state(Date.now());
  let stop: (() => void) | null = null;
  let timer: ReturnType<typeof setInterval> | undefined;
  let hereBox: HTMLElement | undefined = $state();

  const hereName = $derived(hereM?.name ?? "esta máquina");
  const peerName = $derived(peerM?.name ?? "la otra máquina");
  const peerLabel = $derived(target && target.short !== peerName ? `${target.short} · ${peerName}` : peerName);
  const verdict = $derived(phase ? crossVerdict(phase, expected, hereName, peerName, seconds) : null);
  const passed = $derived(phase?.kind === "passed");
  const waiting = $derived(phase?.kind === "waiting");
  const failed = $derived(phase !== null && !waiting && !passed);

  async function begin() {
    stop?.();
    stop = null;
    store.wizardCrossed = false;
    phase = start(Date.now());
    clearInterval(timer);
    timer = setInterval(() => {
      now = Date.now();
      if (phase) phase = onTick(phase, now, limit);
    }, 250);
    try {
      stop = await store.watchCrossing((s) => {
        if (!phase) return;
        phase = onSample(phase, here, expected, s);
        if (phase.kind === "passed") store.wizardCrossed = true;
      });
    } catch (e) {
      phase = { kind: "unavailable", reason: e instanceof Error ? e.message : String(e) };
    }
    if (phase) phase = onTick(phase, Date.now(), limit);
  }

  onDestroy(() => {
    clearInterval(timer);
    stop?.();
  });

  /** Mover la otra máquina a `side` guarda el acomodo y vuelve a empezar la prueba. */
  async function place(side: Side) {
    if (side === expected) return;
    const r = await store.setArrangement(placeOther(desk.layout.arrangement, here, side));
    if (r.ok) {
      stop?.();
      stop = null;
      clearInterval(timer);
      store.wizardCrossed = false;
      phase = null;
    }
  }

  function drop(e: PointerEvent) {
    const r = hereBox?.getBoundingClientRect();
    if (!r) return;
    void place(dropSide(e.clientX - (r.left + r.width / 2), e.clientY - (r.top + r.height / 2)));
  }

  function arrow(e: KeyboardEvent) {
    const side = arrowSide(e.key);
    if (!side) return;
    e.preventDefault();
    void place(side);
  }

  const edgeColor = $derived(passed ? "background:#6cc59a" : failed ? "background:#d9a75a" : "background:#7aa7e6;opacity:.7");
  const doneAt = $derived(`${sketch.done};${sketch.axis === "x" ? "top:150px" : "left:50%"}`);
  const hereColor = $derived(hereM?.color ?? "#7aa7e6");
  const peerColor = $derived(peerM?.color ?? "#a99be8");
</script>

<div class="wz-head">
  <span class="wz-mut wz-sm">Paso 5 de 6</span>
  <h1>¿Dónde está cada pantalla?</h1>
  <p class="wz-mut wz-lead" style="max-width:620px">
    Este es el acomodo que Tandem leyó. Arrastra si no coincide con tu escritorio y prueba: lleva el mouse de {hereName} a {peerName} por el borde marcado.
  </p>
</div>
<div class="wz-canvas" role="group" aria-label="Acomodo de las pantallas">
  <div class="wz-scr" bind:this={hereBox} style="{sketch.here};border-color:{hereColor};background:{fillOf(hereColor)}">{hereName}</div>
  <div
    class="wz-scr drag"
    role="button"
    tabindex="0"
    aria-label="Mover {peerName}: arrastra o usa las flechas"
    style="{sketch.other};border-color:{peerColor};background:{fillOf(peerColor)}"
    onpointerdown={(e) => e.currentTarget.setPointerCapture?.(e.pointerId)}
    onpointerup={drop}
    onkeydown={arrow}
  >
    {peerLabel}
  </div>
  <div class="wz-edge" class:glow={passed} style="{sketch.edge};{edgeColor}"></div>
  {#if waiting}
    <span
      class="wz-cur go {sketch.axis}"
      style="--p0:{sketch.path[0]}{sketch.axis === 'x' ? '%' : 'px'};--p1:{sketch.path[1]}{sketch.axis === 'x' ? '%' : 'px'};--p2:{sketch.path[2]}{sketch.axis === 'x' ? '%' : 'px'}"
    ></span>
  {/if}
  {#if passed}<span class="wz-cur" style={doneAt}></span>{/if}
</div>
<div style="min-height:64px" aria-live="polite">
  {#if phase === null}
    <div class="wz-row">
      <Button icon="cursor" variant="pri" size="xl" onclick={begin}>Probar cruce</Button><span class="wz-mut" style="font-size:13px">Necesitas el mouse a mano.</span>
    </div>
  {:else if waiting && verdict}
    <div class="wz-verdict">
      <span style="font-size:13.5px">{verdict.text}</span>
      <span class="wz-mono wz-mut wz-xs">{remaining(phase, now, limit)} s</span>
    </div>
  {:else if passed && verdict}
    <div class="wz-banner ok">
      <Icon name="check" size={18} stroke={2.4} color="#6cc59a" /><span style="font-size:13.5px">{verdict.text}</span>
    </div>
  {:else if verdict}
    <div class="wz-banner warn">
      <span style="font-size:13.5px;color:#e6c48f">{verdict.text}</span><span class="wz-fill"></span>
      <Button icon="refresh" size="sm" class="wz-sbtn" onclick={begin}>Probar de nuevo</Button>
    </div>
  {/if}
</div>
