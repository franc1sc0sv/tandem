<script lang="ts">
  // Paso 2 (`V3-Asistente`): unirse a un puesto o crear uno. Quien crea muestra el código de 6
  // dígitos; quien se une lo escribe, y así prueba que ve la pantalla de la otra máquina antes
  // de confirmar. Las dos máquinas se emparejan una vez (D1: exactamente dos).
  import { onMount } from "svelte";
  import { fillOf } from "../../lib/acomodo";
  import { useStore } from "../../lib/context";
  import { LINK_NAME } from "../../lib/machines";
  import { codeMatches, isComplete, setDigit } from "../../lib/pairing";
  import { pairScreen } from "../../lib/pairing";
  import Button from "../../ui/Button.svelte";
  import Icon from "../../ui/Icon.svelte";
  import { machineIcon } from "../../ui/icons";

  interface Props {
    /** Pedir confirmar antes de desemparejar (lo lleva el asistente, que cancela al salir del paso). */
    askUnpair: boolean;
    onAsk: (on: boolean) => void;
  }
  let { askUnpair, onAsk }: Props = $props();

  const store = useStore();
  const desk = $derived(store.desk);
  const here = $derived(desk.machines[desk.here]);
  const other = $derived(desk.machines[desk.here === "a" ? "b" : "a"]);
  const otherName = $derived(other?.name ?? "la otra máquina");
  const pair = $derived(pairScreen(otherName, desk.pair.peer, desk.pair.pairing, askUnpair));

  let mode = $state<"join" | "new">("join");
  let digits = $state<string[]>(["", "", "", "", "", ""]);
  let wrong = $state(false);
  let boxes: HTMLInputElement[] = $state([]);

  const peerColor = $derived(other?.color ?? "#a99be8");
  const detail = $derived.by(() => {
    if (!other) return "encontrada en tu red";
    const os = other.os === "macos" ? "macOS" : "Windows";
    const via = desk.link.kind ? ` · por ${LINK_NAME[desk.link.kind]}` : "";
    return `${os} · puesto «${desk.deskName}» · 1 máquina${via}`;
  });

  // Se busca sola al entrar: las dos máquinas tienen que estar en este paso para encontrarse.
  onMount(() => {
    if (desk.pair.peer.kind === "unpaired" && desk.pair.pairing.kind === "idle") void store.pairAction("start_pairing");
  });

  // Quien crea el puesto no confirma a mano: la otra máquina lo hace al escribir el código.
  $effect(() => {
    if (mode === "new" && pair.kind === "compare" && !pair.confirmed) void store.pairAction("confirm_pairing");
  });

  // Un código nuevo empieza con las casillas vacías.
  $effect(() => {
    if (pair.kind !== "compare") {
      digits = ["", "", "", "", "", ""];
      wrong = false;
    }
  });

  function type(i: number, value: string) {
    const clean = value.replace(/\D/g, "");
    // Pegar el código entero llena las seis casillas.
    const next = clean.length > 1 ? clean.slice(0, 6).split("").concat(Array(6).fill("")).slice(0, 6) : setDigit(digits, i, clean);
    digits = next;
    wrong = false;
    const at = clean.length > 1 ? Math.min(clean.length, 5) : clean ? i + 1 : i;
    if (clean) boxes[Math.min(at, 5)]?.focus();
    if (isComplete(next) && pair.kind === "compare") {
      if (codeMatches(next, pair.digits.join(""))) void store.pairAction("confirm_pairing");
      else wrong = true;
    }
  }

  function key(i: number, e: KeyboardEvent) {
    if (e.key === "Backspace" && !digits[i] && i > 0) boxes[i - 1]?.focus();
  }

  const OPT_ON = "border-color:#7aa7e6;background:#16202e";
</script>

<div class="wz-head">
  <span class="wz-mut wz-sm">Paso 2 de 6</span>
  <h1>¿Esta máquina se une a un puesto?</h1>
  <p class="wz-mut wz-lead">Un puesto es el grupo de máquinas que comparten teclado, mouse, monitores y trabajo. Esta es «{here?.name ?? "esta máquina"}».</p>
</div>

<div role="radiogroup" aria-label="Crear o unirse" style="display:flex;gap:12px;flex-wrap:wrap">
  <button type="button" role="radio" class="wz-opt" aria-checked={mode === "join"} onclick={() => (mode = "join")} style={mode === "join" ? OPT_ON : ""}>
    <span class="wz-oi"><Icon name="login" size={19} color="#7aa7e6" /></span>
    <b style="font-weight:600">Unirme a un puesto</b>
    <span class="wz-mut wz-sm">Ya tienes Tandem en otra máquina.</span>
  </button>
  <button type="button" role="radio" class="wz-opt" aria-checked={mode === "new"} onclick={() => (mode = "new")} style={mode === "new" ? OPT_ON : ""}>
    <span class="wz-oi"><Icon name="grid-plus" size={19} color="#a99be8" /></span>
    <b style="font-weight:600">Crear un puesto nuevo</b>
    <span class="wz-mut wz-sm">Es la primera máquina. Las demás se unen después.</span>
  </button>
</div>

{#if pair.kind === "done" || pair.kind === "paired"}
  <section class="wz-panel" aria-label="Emparejadas">
    <div class="wz-found ok">
      <Icon name={other ? machineIcon(other.os) : "tower"} size={22} color={peerColor} />
      <div class="wz-col"><b>{pair.kind === "done" ? pair.peer : pair.name}</b><span class="wz-xs" style="color:#6cc59a">Emparejada{desk.link.kind ? " · se ven" : ""}</span></div>
      <span class="wz-fill"></span>
      <span class="wz-tick"><Icon name="check" size={17} stroke={2.6} color="#6cc59a" /></span>
    </div>
    {#if pair.kind === "paired"}
      <div class="wz-row"><Button icon="refresh" variant="danger" size="sm" motion="spin" onclick={() => onAsk(true)}>Cambiar la otra máquina</Button></div>
    {/if}
  </section>
{:else if pair.kind === "unpair"}
  <section class="wz-panel" aria-label="Desemparejar">
    <span class="wz-pt">¿Desemparejar {pair.name}?</span>
    <p class="wz-mut wz-note">Dejan de compartir teclado, mouse y túneles hasta que vuelvas a emparejar.</p>
    <div class="wz-row">
      <Button
        icon="x"
        variant="danger"
        onclick={async () => {
          onAsk(false);
          await store.pairAction("unpair");
          await store.pairAction("start_pairing");
        }}>Desemparejar</Button
      >
      <Button icon="arrow-left" variant="ghost" motion="back" onclick={() => onAsk(false)}>No</Button>
    </div>
  </section>
{:else if pair.kind === "failed"}
  <section class="wz-panel" aria-label="No se emparejaron">
    <span class="wz-pt">No se emparejaron</span>
    <p class="wz-mut wz-note">{pair.reason}</p>
    <div class="wz-row"><Button icon="refresh" variant="pri" motion="spin" onclick={() => store.pairAction("start_pairing")}>Reintentar</Button></div>
  </section>
{:else if mode === "join"}
  <section class="wz-panel" aria-label="Unirse">
    {#if pair.kind === "compare"}
      <span class="wz-pt">Encontrada en tu red</span>
      <div class="wz-found" style="border-color:{peerColor};background:{fillOf(peerColor)}">
        <Icon name={other ? machineIcon(other.os) : "tower"} size={22} color={peerColor} />
        <div class="wz-col"><b>{pair.peer}</b><span class="wz-mut wz-xs">{detail}</span></div>
      </div>
      <label for="wz-code-1" style="font-size:13px">Escribe el código que muestra {pair.peer}</label>
      <div class="wz-code" role="group" aria-label="Código de emparejamiento" id="wz-code">
        {#each digits as d, i (i)}
          {#if i === 3}<span style="width:8px"></span>{/if}
          <input
            id="wz-code-{i + 1}"
            class="wz-dg"
            class:bad={wrong}
            aria-label="Dígito {i + 1}"
            inputmode="numeric"
            autocomplete="off"
            maxlength="6"
            value={d}
            disabled={pair.confirmed}
            bind:this={boxes[i]}
            oninput={(e) => type(i, e.currentTarget.value)}
            onkeydown={(e) => key(i, e)}
          />
        {/each}
      </div>
      {#if wrong}
        <p role="alert" class="wz-note" style="color:#e6c48f">No es el código que muestra {pair.peer}. Revísalo y vuelve a escribirlo.</p>
      {/if}
      {#if pair.confirmed}
        <div class="wz-row"><span class="wz-spin" aria-hidden="true"></span><span class="wz-mut">Esperando a que confirme {pair.peer}…</span></div>
      {/if}
      <p class="wz-mut wz-note">
        Al entrar, esta máquina confía en todas las del puesto y ellas en ella, con acceso de administrador para el cómputo remoto. Puedes sacarla después desde cualquier máquina; la otra olvida su llave solo si está conectada, si no, cuando también salga del puesto.
      </p>
    {:else}
      <div class="wz-row"><span class="wz-spin" aria-hidden="true"></span><span class="wz-pt">Buscando a {otherName} en tu red…</span></div>
      <p class="wz-mut wz-note">Abre Tandem en {otherName} y elige «Crear un puesto nuevo»: ahí aparece el código que se escribe aquí.</p>
      <div class="wz-row"><Button icon="search" variant="pri" size="sm" onclick={() => store.pairAction("start_pairing")}>Buscar {otherName}</Button></div>
    {/if}
  </section>
{:else}
  <section class="wz-panel" aria-label="Crear" style="gap:12px">
    {#if pair.kind === "compare"}
      <span class="wz-pt" id="wz-new-code">Código de {here?.name ?? "esta máquina"}</span>
      <div class="wz-code" role="group" aria-labelledby="wz-new-code">
        {#each pair.digits[0].split("") as d, i (i)}<span class="wz-dg" style="display:grid;place-items:center">{d}</span>{/each}<span style="width:8px"></span>{#each pair.digits[1].split("") as d, i (i)}<span
            class="wz-dg"
            style="display:grid;place-items:center">{d}</span
          >{/each}
      </div>
      {#if pair.confirmed}
        <div class="wz-row"><span class="wz-spin" aria-hidden="true"></span><span class="wz-mut">Esperando a que {pair.peer} escriba el código…</span></div>
      {/if}
    {:else}
      <div class="wz-row"><span class="wz-spin" aria-hidden="true"></span><span class="wz-pt">Esperando a {otherName}…</span></div>
    {/if}
    <p class="wz-mut wz-note">Después, en la otra máquina, elige «Unirme a un puesto» y escribe el código que aparecerá aquí.</p>
  </section>
{/if}
