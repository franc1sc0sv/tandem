<script lang="ts">
  // Paso 2 (`V3-Setup`): unirse a un puesto o crear uno. Quien crea muestra el código de 6
  // dígitos; quien se une lo escribe, y así prueba que ve la pantalla de the other machine antes
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
  const otherName = $derived(other?.name ?? "the other machine");
  const pair = $derived(pairScreen(otherName, desk.pair.peer, desk.pair.pairing, askUnpair));

  let mode = $state<"join" | "new">("join");
  let digits = $state<string[]>(["", "", "", "", "", ""]);
  let wrong = $state(false);
  let boxes: HTMLInputElement[] = $state([]);

  const peerColor = $derived(other?.color ?? "#a99be8");
  const detail = $derived.by(() => {
    if (!other) return "found on your network";
    const os = other.os === "macos" ? "macOS" : "Windows";
    const via = desk.link.kind ? ` · over ${LINK_NAME[desk.link.kind]}` : "";
    return `${os} · desk “${desk.deskName}” · 1 machine${via}`;
  });

  // Se busca sola al entrar: las dos máquinas tienen que estar en este paso para encontrarse.
  onMount(() => {
    if (desk.pair.peer.kind === "unpaired" && desk.pair.pairing.kind === "idle") void store.pairAction("start_pairing");
  });

  // Quien crea el puesto no confirma a mano: the other machine lo hace al escribir el código.
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
  <span class="wz-mut wz-sm">Step 2 of 6</span>
  <h1>Does this machine join a desk?</h1>
  <p class="wz-mut wz-lead">A desk is the group of machines that share keyboard, mouse, monitors and work. This one is “{here?.name ?? "this machine"}”.</p>
</div>

<div role="radiogroup" aria-label="Create or join" style="display:flex;gap:12px;flex-wrap:wrap">
  <button type="button" role="radio" class="wz-opt" aria-checked={mode === "join"} onclick={() => (mode = "join")} style={mode === "join" ? OPT_ON : ""}>
    <span class="wz-oi"><Icon name="login" size={19} color="#7aa7e6" /></span>
    <b style="font-weight:600">Join a desk</b>
    <span class="wz-mut wz-sm">You already have Tandem on another machine.</span>
  </button>
  <button type="button" role="radio" class="wz-opt" aria-checked={mode === "new"} onclick={() => (mode = "new")} style={mode === "new" ? OPT_ON : ""}>
    <span class="wz-oi"><Icon name="grid-plus" size={19} color="#a99be8" /></span>
    <b style="font-weight:600">Create a new desk</b>
    <span class="wz-mut wz-sm">This is the first machine. The others join later.</span>
  </button>
</div>

{#if pair.kind === "done" || pair.kind === "paired"}
  <section class="wz-panel" aria-label="Paired">
    <div class="wz-found ok">
      <Icon name={other ? machineIcon(other.os) : "tower"} size={22} color={peerColor} />
      <div class="wz-col"><b>{pair.kind === "done" ? pair.peer : pair.name}</b><span class="wz-xs" style="color:#6cc59a">Paired{desk.link.kind ? " · they see each other" : ""}</span></div>
      <span class="wz-fill"></span>
      <span class="wz-tick"><Icon name="check" size={17} stroke={2.6} color="#6cc59a" /></span>
    </div>
    {#if pair.kind === "paired"}
      <div class="wz-row"><Button icon="refresh" variant="danger" size="sm" motion="spin" onclick={() => onAsk(true)}>Change the other machine</Button></div>
    {/if}
  </section>
{:else if pair.kind === "unpair"}
  <section class="wz-panel" aria-label="Unpair">
    <span class="wz-pt">Unpair {pair.name}?</span>
    <p class="wz-mut wz-note">They stop sharing keyboard, mouse and tunnels until you pair again.</p>
    <div class="wz-row">
      <Button
        icon="x"
        variant="danger"
        onclick={async () => {
          onAsk(false);
          await store.pairAction("unpair");
          await store.pairAction("start_pairing");
        }}>Unpair</Button
      >
      <Button icon="arrow-left" variant="ghost" motion="back" onclick={() => onAsk(false)}>No</Button>
    </div>
  </section>
{:else if pair.kind === "failed"}
  <section class="wz-panel" aria-label="They did not pair">
    <span class="wz-pt">They did not pair</span>
    <p class="wz-mut wz-note">{pair.reason}</p>
    <div class="wz-row"><Button icon="refresh" variant="pri" motion="spin" onclick={() => store.pairAction("start_pairing")}>Retry</Button></div>
  </section>
{:else if mode === "join"}
  <section class="wz-panel" aria-label="Join">
    {#if pair.kind === "compare"}
      <span class="wz-pt">Found on your network</span>
      <div class="wz-found" style="border-color:{peerColor};background:{fillOf(peerColor)}">
        <Icon name={other ? machineIcon(other.os) : "tower"} size={22} color={peerColor} />
        <div class="wz-col"><b>{pair.peer}</b><span class="wz-mut wz-xs">{detail}</span></div>
      </div>
      <label for="wz-code-1" style="font-size:13px">Enter the code {pair.peer} shows</label>
      <div class="wz-code" role="group" aria-label="Pairing code" id="wz-code">
        {#each digits as d, i (i)}
          {#if i === 3}<span style="width:8px"></span>{/if}
          <input
            id="wz-code-{i + 1}"
            class="wz-dg"
            class:bad={wrong}
            aria-label="Digit {i + 1}"
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
        <p role="alert" class="wz-note" style="color:#e6c48f">That is not the code {pair.peer} shows. Check it and type it again.</p>
      {/if}
      {#if pair.confirmed}
        <div class="wz-row"><span class="wz-spin" aria-hidden="true"></span><span class="wz-mut">Waiting for {pair.peer} to confirm…</span></div>
      {/if}
      <p class="wz-mut wz-note">
        On joining, this machine trusts every machine of the desk and they trust it, with administrator access for remote compute. You can remove it later from any machine; the other one forgets its key only if connected, otherwise once it also leaves the desk.
      </p>
    {:else}
      <div class="wz-row"><span class="wz-spin" aria-hidden="true"></span><span class="wz-pt">Looking for {otherName} on your network…</span></div>
      <p class="wz-mut wz-note">Open Tandem on {otherName} and choose “Create a new desk”: the code to type here appears there.</p>
      <div class="wz-row"><Button icon="search" variant="pri" size="sm" onclick={() => store.pairAction("start_pairing")}>Look for {otherName}</Button></div>
    {/if}
  </section>
{:else}
  <section class="wz-panel" aria-label="Create" style="gap:12px">
    {#if pair.kind === "compare"}
      <span class="wz-pt" id="wz-new-code">Code of {here?.name ?? "this machine"}</span>
      <div class="wz-code" role="group" aria-labelledby="wz-new-code">
        {#each pair.digits[0].split("") as d, i (i)}<span class="wz-dg" style="display:grid;place-items:center">{d}</span>{/each}<span style="width:8px"></span>{#each pair.digits[1].split("") as d, i (i)}<span
            class="wz-dg"
            style="display:grid;place-items:center">{d}</span
          >{/each}
      </div>
      {#if pair.confirmed}
        <div class="wz-row"><span class="wz-spin" aria-hidden="true"></span><span class="wz-mut">Waiting for {pair.peer} to enter the code…</span></div>
      {/if}
    {:else}
      <div class="wz-row"><span class="wz-spin" aria-hidden="true"></span><span class="wz-pt">Waiting for {otherName}…</span></div>
    {/if}
    <p class="wz-mut wz-note">Then, on the other machine, choose “Join a desk” and enter the code that will appear here.</p>
  </section>
{/if}
