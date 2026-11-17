<script lang="ts" module>
  export const WIZARD_STEPS = ["Esta máquina", "La otra máquina", "Permisos", "Teclado y mouse", "Acomodo y prueba de cruce", "Listo"];
  /** Pasos que todavía no tienen contenido (T-119): se ven en la lista pero no se entra. */
  export const WIZARD_LATER = new Set([3, 4]);
</script>

<script lang="ts">
  // Asistente (diseño de `V3-Asistente` y `F-Asis3`): lista de pasos a la izquierda y el paso
  // actual a la derecha. Ya funcionan «Esta máquina», «La otra máquina» (emparejar con el
  // código de 6 dígitos) y «Permisos» (macOS, T-090) con la lógica de `pairing.ts`,
  // `setup.ts` y `permissions.ts`. T-119 trae «Teclado y mouse» y «Acomodo».
  import { tick } from "svelte";
  import { useStore } from "../lib/context";
  import { enter, leave } from "../lib/motion";
  import { isActive, needsDismiss, pairScreen } from "../lib/pairing";
  import { PERMISSION_INFO } from "../lib/setup";
  import Button from "../ui/Button.svelte";
  import Icon from "../ui/Icon.svelte";
  import { machineIcon } from "../ui/icons";
  import Logo from "./Logo.svelte";

  const store = useStore();
  let main: HTMLElement | undefined = $state();
  let askUnpair = $state(false);

  const desk = $derived(store.desk);
  const step = $derived(store.wizardStep);
  const here = $derived(desk.machines[desk.here]);
  const other = $derived(desk.machines[desk.here === "a" ? "b" : "a"]);
  const otherName = $derived(other?.name ?? "la otra máquina");
  const pair = $derived(pairScreen(otherName, desk.pair.peer, desk.pair.pairing, askUnpair));
  const perms = $derived(desk.permissions);
  const missing = $derived(perms.rows.filter((r) => r.state === "missing").length);

  $effect(() => {
    if (store.wizardOpen) void tick().then(() => main?.focus());
  });

  /** Siguiente paso con contenido. */
  function nextOf(i: number, dir: 1 | -1): number {
    let n = i + dir;
    while (WIZARD_LATER.has(n)) n += dir;
    return Math.max(0, Math.min(WIZARD_STEPS.length - 1, n));
  }

  /** Al salir del paso de emparejar, un resultado ya mostrado deja al agente en reposo. */
  function leavePair() {
    askUnpair = false;
    if (step === 1 && needsDismiss(desk.pair.pairing)) void store.pairAction("cancel_pairing");
  }

  function goTo(i: number) {
    if (WIZARD_LATER.has(i)) return;
    leavePair();
    store.wizardStep = i;
  }

  function close() {
    // Cerrar con un emparejamiento vivo lo cancela (y avisa a la otra máquina).
    if (isActive(desk.pair.pairing) || needsDismiss(desk.pair.pairing)) void store.pairAction("cancel_pairing");
    if (step === WIZARD_STEPS.length - 1 || step === 2) void store.finishSetup();
    askUnpair = false;
    store.wizardOpen = false;
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") close();
  }

  const PAIR_LEAD = "Las dos se emparejan una vez con un código que ves en las dos pantallas.";
</script>

{#if store.wizardOpen}
  <div class="wiz" role="dialog" aria-modal="true" aria-label="Asistente" tabindex="-1" {onkeydown} in:enter={{ duration: 250 }} out:leave>
    <aside aria-label="Pasos" class="side">
      <div class="brand"><Logo size={22} />Tandem</div>
      <ol>
        {#each WIZARD_STEPS as s, i (s)}
          <li>
            <button
              type="button"
              class="stl"
              class:done={i < step && !WIZARD_LATER.has(i)}
              class:cur={i === step}
              class:later={WIZARD_LATER.has(i)}
              aria-current={i === step ? "step" : undefined}
              disabled={WIZARD_LATER.has(i)}
              onclick={() => goTo(i)}><i>{i < step && !WIZARD_LATER.has(i) ? "✓" : i + 1}</i>{s}</button
            >
          </li>
        {/each}
      </ol>
      <span class="fill"></span>
      <Button icon="x" variant="ghost" size="sm" onclick={close}>Cerrar</Button>
    </aside>

    <main bind:this={main} tabindex="-1">
      {#key step}
        <div class="body" in:enter={{ y: 6, duration: 350 }}>
          <div class="head">
            <span class="mut sm">Paso {step + 1} de {WIZARD_STEPS.length}{step === 2 && here ? ` · ${here.name}` : ""}</span>
            {#if step === 0}
              <h1>Esta máquina</h1>
              <p class="mut lead">Tandem corre en las dos máquinas. Primero, esta.</p>
            {:else if step === 1}
              <h1>La otra máquina</h1>
              <p class="mut lead">{PAIR_LEAD}</p>
            {:else if step === 2}
              <h1>{perms.needed ? "Permisos de macOS" : "Permisos"}</h1>
              {#if perms.needed}
                <p class="mut lead">Se dan a <span class="mono tm">tandemd</span>, no a la ventana. Tandem los detecta solo en cuanto los concedes.</p>
              {/if}
            {:else}
              <h1>Listo</h1>
              <p class="mut lead">Esto es lo que quedó.</p>
            {/if}
          </div>

          {#if step === 0}
            {#if here}
              <div class="found here">
                <Icon name={machineIcon(here.os)} size={22} color={here.color} />
                <div class="col"><b>{here.name}</b><span class="mut xs">{here.detail}</span></div>
              </div>
            {:else}
              <div class="perm"><span class="dot wait"></span><div class="col"><b>Sin conexión con tandemd</b><span class="mut xs">{desk.troubleText}</span></div></div>
            {/if}
          {:else if step === 1}
            <section class="panel" aria-label="Emparejar">
              {#if pair.kind === "intro"}
                <span class="pt">Sin emparejar</span>
                <p class="mut note">Abre Tandem también en {pair.other} y pulsa «Buscar» en las dos.</p>
                <div class="row"><Button icon="search" variant="pri" onclick={() => store.pairAction("start_pairing")}>Buscar {pair.other}</Button></div>
              {:else if pair.kind === "searching"}
                <div class="row"><span class="spin" aria-hidden="true"></span><span class="pt">Buscando a {pair.other}…</span></div>
                <p class="mut note">{pair.hint}</p>
                <div class="row"><Button icon="x" onclick={() => store.pairAction("cancel_pairing")}>Cancelar</Button></div>
              {:else if pair.kind === "compare"}
                <div class="found">
                  <Icon name={other ? machineIcon(other.os) : "tower"} size={22} color={other?.color ?? "#a99be8"} />
                  <div class="col"><b>{pair.peer}</b><span class="mut xs">{other?.detail ?? "encontrada en tu red"}</span></div>
                </div>
                <span class="label" id="code-label">¿Es el mismo código que muestra {pair.peer}?</span>
                <div class="code" role="group" aria-labelledby="code-label">
                  {#each pair.digits[0].split("") as d, i (i)}<span class="dg">{d}</span>{/each}<span class="gap"></span>{#each pair.digits[1].split("") as d, i (i)}<span
                      class="dg">{d}</span
                    >{/each}
                </div>
                {#if pair.confirmed}
                  <div class="row"><span class="spin" aria-hidden="true"></span><span class="mut">Esperando a que confirme {pair.peer}…</span></div>
                {:else}
                  <div class="row">
                    <Button icon="check" variant="pri" onclick={() => store.pairAction("confirm_pairing")}>Es el mismo</Button>
                    <Button icon="x" variant="danger" onclick={() => store.pairAction("cancel_pairing")}>No coincide</Button>
                  </div>
                {/if}
              {:else if pair.kind === "done" || pair.kind === "paired"}
                <div class="found ok">
                  <Icon name={other ? machineIcon(other.os) : "tower"} size={22} color={other?.color ?? "#a99be8"} />
                  <div class="col"><b>{pair.kind === "done" ? pair.peer : pair.name}</b><span class="okt xs">Emparejada{desk.link.kind ? " · se ven" : ""}</span></div>
                  <span class="fill"></span>
                  <span class="tick"><Icon name="check" size={17} stroke={2.6} color="#6cc59a" /></span>
                </div>
                {#if pair.kind === "paired"}
                  <div class="row"><Button icon="refresh" variant="danger" size="sm" motion="spin" onclick={() => (askUnpair = true)}>Cambiar la otra máquina</Button></div>
                {/if}
              {:else if pair.kind === "unpair"}
                <span class="pt">¿Desemparejar {pair.name}?</span>
                <p class="mut note">Dejan de compartir teclado, mouse y túneles hasta que vuelvas a emparejar.</p>
                <div class="row">
                  <Button icon="x" variant="danger" onclick={async () => { askUnpair = false; await store.pairAction("unpair"); }}>Desemparejar</Button>
                  <Button icon="arrow-left" variant="ghost" motion="back" onclick={() => (askUnpair = false)}>No</Button>
                </div>
              {:else if pair.kind === "failed"}
                <div class="perm"><span class="dot bad"></span><div class="col"><b>No se emparejaron</b><span class="mut xs">{pair.reason}</span></div></div>
                <div class="row"><Button icon="refresh" variant="pri" motion="spin" onclick={() => store.pairAction("start_pairing")}>Reintentar</Button></div>
              {/if}
            </section>
          {:else if step === 2}
            {#if perms.needed}
              {#each perms.rows as p (p.id)}
                <div class="perm" class:granted={p.state === "granted"}>
                  <span class="dot {p.state === 'granted' ? 'got' : p.state === 'missing' ? 'wait' : 'unk'}"></span>
                  <div class="col grow">
                    <b>{PERMISSION_INFO[p.id].title}</b>
                    <span class="mut why">{PERMISSION_INFO[p.id].why}</span>
                    <span class="st {p.state}">{p.state === "granted" ? "Concedido" : p.state === "missing" ? "Falta: se comprueba solo en cuanto lo concedes" : "macOS lo pregunta la primera vez"}</span>
                  </div>
                  {#if p.state === "missing"}
                    <Button icon="external" size="sm" motion="lift" onclick={() => store.openPermission(p.id)}>Abrir ajustes</Button>
                  {:else if p.state === "granted"}
                    <span class="tick"><Icon name="check" size={17} stroke={2.6} color="#6cc59a" /></span>
                  {/if}
                </div>
              {/each}
              {#if missing > 0}
                {#if perms.binary}<p class="mono mut bin">{perms.binary}</p>{/if}
                <div class="row">
                  <Button icon="shield" variant="pri" size="sm" onclick={() => store.requestPermissions()}>Pedir permisos</Button>
                  <Button icon="refresh" size="sm" motion="spin" onclick={() => store.restartAgent()}>Reiniciar agente</Button>
                </div>
              {/if}
            {:else}
              <div class="perm granted">
                <span class="dot got"></span>
                <div class="col grow"><b>Esta máquina no necesita permisos</b><span class="mut why">Windows deja capturar e inyectar el teclado y el mouse sin pedir nada.</span></div>
                <span class="tick"><Icon name="check" size={17} stroke={2.6} color="#6cc59a" /></span>
              </div>
            {/if}
          {:else}
            <div class="perm granted"><span class="dot got"></span><div class="col grow"><b>{here?.name ?? "Esta máquina"}</b><span class="mut why">{here?.detail ?? ""}</span></div></div>
            <div class="perm" class:granted={desk.pair.peer.kind === "paired"}>
              <span class="dot {desk.pair.peer.kind === 'paired' ? 'got' : 'wait'}"></span>
              <div class="col grow"><b>{desk.pair.peer.kind === "paired" ? `Emparejada con ${desk.pair.peer.name}` : "Sin emparejar"}</b></div>
            </div>
            <div class="perm" class:granted={missing === 0}>
              <span class="dot {missing === 0 ? 'got' : 'wait'}"></span>
              <div class="col grow"><b>{missing === 0 ? "Permisos en orden" : `Falta${missing === 1 ? "" : "n"} ${missing} permiso${missing === 1 ? "" : "s"}`}</b></div>
            </div>
          {/if}

          <div class="foot">
            {#if step > 0}<Button icon="arrow-left" variant="ghost" motion="back" size="xl" onclick={() => goTo(nextOf(step, -1))}>Atrás</Button>{/if}
            <span class="fill"></span>
            {#if step === 2 && missing > 0}<span class="mut sm">Falta{missing === 1 ? "" : "n"} {missing}</span>{/if}
            {#if step < WIZARD_STEPS.length - 1}
              <Button icon="arrow-right" variant="pri" motion="go" iconAfter size="xl" onclick={() => goTo(nextOf(step, 1))}>Continuar</Button>
            {:else}
              <Button icon="check" variant="pri" size="xl" onclick={close}>Terminar</Button>
            {/if}
          </div>
        </div>
      {/key}
    </main>
  </div>
{/if}

<style>
  .wiz {
    position: fixed;
    inset: 0;
    z-index: 25;
    display: flex;
    flex-wrap: wrap;
    background: #0e0f12;
    color: #e8e8eb;
    font-size: 14px;
    outline: none;
    overflow: auto;
  }
  .side {
    flex: 1 1 240px;
    max-width: 300px;
    box-sizing: border-box;
    padding: 28px;
    border-right: 1px solid #1f2126;
    background: #0b0c0e;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 9px;
    padding-bottom: 22px;
    font-weight: 650;
    font-size: 16px;
  }
  ol {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .stl {
    font: inherit;
    width: calc(100% + 20px);
    border: 0;
    background: transparent;
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 12px;
    font-size: 13.5px;
    color: #9a9da6;
    padding: 8px 10px;
    margin: 0 -10px;
    border-radius: 9px;
    text-align: left;
    font-weight: 400;
    transition:
      background 0.2s,
      color 0.2s;
  }
  .stl:hover:not(:disabled) {
    background: #17181c;
    color: #e8e8eb;
  }
  .stl i {
    width: 26px;
    height: 26px;
    border-radius: 50%;
    border: 1.5px solid #31343b;
    color: #9a9da6;
    display: grid;
    place-items: center;
    font-style: normal;
    font-size: 12px;
    flex: none;
    transition: transform 0.25s cubic-bezier(0.3, 1.5, 0.5, 1);
  }
  .stl:hover:not(:disabled) i {
    transform: scale(1.1);
  }
  .stl.done {
    color: #c4c6cc;
  }
  .stl.done i {
    border-color: #6cc59a;
    color: #6cc59a;
  }
  .stl.cur {
    color: #e8e8eb;
    font-weight: 600;
    background: #15161a;
  }
  .stl.cur i {
    border-color: #e8e8eb;
    color: #e8e8eb;
  }
  .stl.later {
    opacity: 0.45;
    cursor: default;
  }
  .stl:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  main {
    flex: 999 1 560px;
    min-width: 0;
    outline: none;
    display: flex;
  }
  .body {
    box-sizing: border-box;
    padding: 48px 56px;
    display: flex;
    flex-direction: column;
    gap: 20px;
    max-width: 860px;
    width: 100%;
    min-height: 100%;
  }
  .head {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  h1 {
    margin: 0;
    font-size: 26px;
    font-weight: 650;
  }
  .lead {
    margin: 0;
    line-height: 1.55;
    max-width: 600px;
  }
  .tm {
    font-size: 12.5px;
    color: #c4c6cc;
  }
  .sm {
    font-size: 12.5px;
  }
  .xs {
    font-size: 12px;
  }
  .fill {
    flex: 1;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .col.grow {
    flex: 1;
    min-width: 0;
    gap: 5px;
  }
  .col b,
  .found b {
    font-weight: 600;
  }
  .panel {
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 20px;
    border-radius: 14px;
    background: #15161a;
    border: 1px solid #25272d;
    animation: fdi 0.35s ease-out;
  }
  .pt {
    font-size: 13px;
    font-weight: 600;
  }
  .note {
    margin: 0;
    font-size: 12.5px;
    line-height: 1.5;
  }
  .label {
    font-size: 13px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .found {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 14px;
    border-radius: 10px;
    border: 1px solid #a99be8;
    background: #1d1a2b;
  }
  .found.here {
    border-color: #31343b;
    background: #15161a;
  }
  .found.ok {
    border-color: #2c4a3c;
    background: #121a16;
  }
  .okt {
    color: #6cc59a;
  }
  .code {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .gap {
    width: 8px;
  }
  .dg {
    width: 46px;
    height: 58px;
    border-radius: 10px;
    background: #111215;
    border: 1px solid #31343b;
    font-family: "JetBrains Mono", monospace;
    font-size: 26px;
    color: #e8e8eb;
    display: grid;
    place-items: center;
    box-sizing: border-box;
  }
  .spin {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 2px solid #31343b;
    border-top-color: #7aa7e6;
    animation: sp 0.8s linear infinite;
    flex: none;
  }
  .perm {
    display: flex;
    gap: 14px;
    align-items: flex-start;
    padding: 16px;
    border-radius: 12px;
    border: 1px solid #25272d;
    background: #15161a;
    transition:
      border-color 0.4s,
      background 0.4s;
  }
  .perm.granted {
    border-color: #2c4a3c;
    background: #121a16;
  }
  .dot {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    margin-top: 4px;
    flex: none;
    transition: background 0.4s;
    background: #4a4d55;
  }
  .dot.wait {
    background: #d9a75a;
    animation: wiz-pl 1.6s ease-in-out infinite;
  }
  .dot.got {
    background: #6cc59a;
    animation: wiz-pop 0.4s cubic-bezier(0.2, 1.6, 0.4, 1);
  }
  .dot.bad {
    background: #e07a6f;
  }
  .why {
    font-size: 13px;
    line-height: 1.5;
  }
  .st {
    font-size: 12.5px;
  }
  .st.granted {
    color: #6cc59a;
  }
  .st.missing {
    color: #e6c48f;
  }
  .st.unknown {
    color: #9a9da6;
  }
  .tick {
    width: 36px;
    height: 36px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: #1e3a2e;
    flex: none;
    animation: wiz-pop 0.4s cubic-bezier(0.2, 1.6, 0.4, 1);
  }
  .bin {
    margin: 0;
    font-size: 11.5px;
    word-break: break-all;
  }
  .foot {
    display: flex;
    gap: 10px;
    margin-top: auto;
    align-items: center;
  }
  @keyframes fdi {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  @keyframes wiz-pl {
    0%,
    100% {
      box-shadow: 0 0 0 0 rgba(217, 167, 90, 0.5);
    }
    50% {
      box-shadow: 0 0 0 6px rgba(217, 167, 90, 0);
    }
  }
  @keyframes wiz-pop {
    from {
      transform: scale(0.3);
    }
    to {
      transform: scale(1);
    }
  }
</style>
