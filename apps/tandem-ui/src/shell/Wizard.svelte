<script lang="ts">
  // Asistente de configuración (`F-Setup1`…`F-Setup6`): la lista de pasos to the left of y el paso
  // actual to the right of. Cada paso vive en `wizard/` y habla con el agente por el store. Se
  // abre solo la primera vez y se puede reabrir desde Settings.
  import { tick } from "svelte";
  import { useStore } from "../lib/context";
  import { enter, leave } from "../lib/motion";
  import { isActive, needsDismiss } from "../lib/pairing";
  import { STEP_PAIR, STEP_PERMISSIONS, WIZARD_STEPS, clampStep, missingOf, permCards, permFooter, stepMark } from "../lib/wizard";
  import Button from "../ui/Button.svelte";
  import Icon from "../ui/Icon.svelte";
  import Logo from "./Logo.svelte";
  import StepArrange from "./wizard/StepArrange.svelte";
  import StepDone from "./wizard/StepDone.svelte";
  import StepInput from "./wizard/StepInput.svelte";
  import StepMachine from "./wizard/StepMachine.svelte";
  import StepPair from "./wizard/StepPair.svelte";
  import StepPermissions from "./wizard/StepPermissions.svelte";
  import "./wizard/wizard.css";

  const store = useStore();
  let main: HTMLElement | undefined = $state();
  let askUnpair = $state(false);
  /** El usuario ya abrió un panel de Settings del Sistema: ofrece reiniciar el agente. */
  let opened = $state(false);

  const desk = $derived(store.desk);
  const step = $derived(store.wizardStep);
  const perms = $derived(desk.permissions);
  const missing = $derived(perms.needed ? missingOf(permCards(perms.rows)) : 0);
  const last = WIZARD_STEPS.length - 1;

  $effect(() => {
    if (store.wizardOpen) void tick().then(() => main?.focus());
  });

  /** Al salir del paso de emparejar, un resultado ya mostrado deja al agente en reposo. */
  function leavePair() {
    askUnpair = false;
    if (step === STEP_PAIR && (needsDismiss(desk.pair.pairing) || isActive(desk.pair.pairing))) void store.pairAction("cancel_pairing");
  }

  function goTo(i: number) {
    if (i === step) return;
    leavePair();
    store.wizardStep = clampStep(i);
  }

  function close() {
    leavePair();
    if (step >= STEP_PERMISSIONS) void store.finishSetup();
    store.wizardOpen = false;
  }

  function finish() {
    close();
    store.go({ screen: "desk" });
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") close();
  }
</script>

{#if store.wizardOpen}
  <div class="wiz" role="dialog" aria-modal="true" aria-label="Setup" tabindex="-1" {onkeydown} in:enter={{ duration: 250 }} out:leave>
    <aside aria-label="Steps" class="side">
      <div class="brand"><Logo size={22} />Tandem</div>
      <ol>
        {#each WIZARD_STEPS as s, i (s)}
          {@const mark = stepMark(i, step)}
          <li>
            <button type="button" class="stl" class:done={mark === "done"} class:cur={mark === "cur"} aria-current={mark === "cur" ? "step" : undefined} onclick={() => goTo(i)}
              ><i>{mark === "done" ? "✓" : i + 1}</i>{s}</button
            >
          </li>
        {/each}
      </ol>
      <span class="fill"></span>
      <button type="button" class="wz-close" aria-label="Close" onclick={close}><Icon name="x" size={16} stroke={2.2} /></button>
    </aside>

    <main bind:this={main} tabindex="-1" class:wide={step === 3 || step === 4}>
      {#key step}
        <div class="body" class:tight={step === 2 || step === 4} class:arrange={step === 4} in:enter={{ y: 6, duration: 350 }}>
          {#if step === 0}
            <StepMachine />
          {:else if step === 1}
            <StepPair {askUnpair} onAsk={(on) => (askUnpair = on)} />
          {:else if step === 2}
            <StepPermissions onOpened={() => (opened = true)} />
          {:else if step === 3}
            <StepInput />
          {:else if step === 4}
            <StepArrange />
          {:else}
            <StepDone />
          {/if}

          <div class="foot">
            {#if step > 0 && step < last}<Button icon="arrow-left" variant="ghost" motion="back" size="xl" onclick={() => goTo(step - 1)}>Back</Button>{/if}
            <span class="fill"></span>
            {#if step === 2 && perms.needed}
              {#if opened && missing > 0}<Button icon="refresh" variant="ghost" size="sm" motion="spin" onclick={() => store.restartAgent()}>Restart agent</Button>{/if}
              <span class="mut sm">{permFooter(missing)}</span>
            {/if}
            {#if step === 4}<Button icon="skip" variant="ghost" size="xl" onclick={() => goTo(step + 1)}>Skip</Button>{/if}
            {#if step < last}
              <Button
                icon="arrow-right"
                variant="pri"
                motion="go"
                iconAfter
                size="xl"
                class="wz-next {(step === 2 && missing > 0) || (step === 4 && !store.wizardCrossed) ? 'wait' : ''}"
                onclick={() => goTo(step + 1)}>Continue</Button
              >
            {:else}
              <Button icon="arrow-right" variant="pri" motion="go" iconAfter size="xl" class="wz-fin" onclick={finish}>Go to desk</Button>
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
    font-family: "Inter", system-ui, sans-serif;
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
  .stl:hover {
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
  .stl:hover i {
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
  .stl:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  /* Como el mockup: el ancho máximo es de `main`, y la lista de pasos crece hasta 300 px. */
  main {
    flex: 999 1 560px;
    min-width: 0;
    max-width: 860px;
    outline: none;
    display: flex;
  }
  main.wide {
    max-width: 900px;
  }
  .body {
    box-sizing: border-box;
    padding: 48px 56px;
    display: flex;
    flex-direction: column;
    gap: 22px;
    width: 100%;
    min-height: 100%;
  }
  /* `F-Setup3` y `F-Setup5` separan sus bloques 20 px; los demás, 22. */
  .body.tight {
    gap: 20px;
  }
  .fill {
    flex: 1;
  }
  /* El mockup no lo dibuja; es la salida con el mouse a mitad del asistente (mockups/README.md). */
  .wz-close {
    align-self: flex-start;
    width: 32px;
    height: 32px;
    flex: none;
    display: grid;
    place-items: center;
    padding: 0;
    border: 1px solid transparent;
    border-radius: 8px;
    background: transparent;
    color: #9a9da6;
    cursor: pointer;
  }
  .wz-close:hover {
    background: #16181c;
    color: #e8e9ec;
  }
  .wz-close:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .mut {
    color: #9a9da6;
  }
  .sm {
    font-size: 12.5px;
  }
  .foot {
    display: flex;
    gap: 10px;
    margin-top: auto;
    align-items: center;
  }
</style>
