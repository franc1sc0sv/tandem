<script lang="ts" module>
  export const WIZARD_STEPS = ["Esta máquina", "La otra máquina", "Permisos", "Teclado y mouse", "Acomodo", "Listo"];
</script>

<script lang="ts">
  // Contenedor del asistente: lo abren Ajustes y la paleta. Por ahora muestra los seis pasos
  // de v0.3 (F-Asis1 … F-Asis6) y se recorre; T-119 pone el contenido y la verificación de
  // cada paso.
  import { tick } from "svelte";
  import { useStore } from "../lib/context";
  import { enter, leave } from "../lib/motion";
  import Button from "../ui/Button.svelte";
  import Icon from "../ui/Icon.svelte";

  const store = useStore();
  let step = $state(0);
  let dialog: HTMLElement | undefined = $state();

  $effect(() => {
    if (store.wizardOpen) {
      step = 0;
      void tick().then(() => dialog?.focus());
    }
  });

  function close() {
    store.wizardOpen = false;
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") close();
  }
</script>

{#if store.wizardOpen}
  <div class="veil" role="presentation" onclick={close} in:enter out:leave></div>
  <div class="wrap">
    <div bind:this={dialog} role="dialog" aria-modal="true" aria-label="Asistente" class="wiz" tabindex="-1" {onkeydown} in:enter={{ y: 10, duration: 300 }} out:leave>
      <ol class="steps">
        {#each WIZARD_STEPS as s, i (s)}
          <li class:cur={i === step} class:done={i < step} aria-current={i === step ? "step" : undefined}>
            <span class="n">{#if i < step}<Icon name="check" size={13} stroke={2.6} />{:else}{i + 1}{/if}</span>{s}
          </li>
        {/each}
      </ol>
      <h2>{WIZARD_STEPS[step]}</h2>
      <div class="foot">
        <Button icon="x" variant="ghost" onclick={close}>Cerrar</Button>
        <span class="fill"></span>
        <Button icon="arrow-left" motion="back" disabled={step === 0} onclick={() => (step -= 1)}>Atrás</Button>
        {#if step < WIZARD_STEPS.length - 1}
          <Button icon="arrow-right" variant="pri" motion="go" iconAfter onclick={() => (step += 1)}>Siguiente</Button>
        {:else}
          <Button icon="check" variant="pri" onclick={close}>Terminar</Button>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .veil {
    position: fixed;
    inset: 0;
    background: rgba(8, 9, 11, 0.7);
    z-index: 20;
  }
  .wrap {
    position: fixed;
    inset: 0;
    z-index: 21;
    display: grid;
    place-items: center;
    pointer-events: none;
  }
  .wiz {
    pointer-events: auto;
    width: min(720px, calc(100% - 48px));
    box-sizing: border-box;
    border-radius: 16px;
    background: #15161a;
    border: 1px solid #31343b;
    box-shadow: 0 30px 70px rgba(0, 0, 0, 0.55);
    padding: 22px 24px;
    display: flex;
    flex-direction: column;
    gap: 18px;
    outline: none;
  }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .steps li {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 12.5px;
    color: #80838d;
    padding: 5px 10px 5px 6px;
    border-radius: 999px;
    border: 1px solid #25272d;
    transition:
      color 0.25s,
      border-color 0.25s;
  }
  .steps li.cur {
    color: #e8e8eb;
    border-color: #4f7cbc;
  }
  .steps li.done {
    color: #c4c6cc;
  }
  .n {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    font-size: 11px;
    background: #1b1c21;
  }
  .done .n {
    background: #1d3a2e;
    color: #6cc59a;
  }
  .cur .n {
    background: #22324a;
    color: #c3d8f6;
  }
  h2 {
    margin: 0;
    font-size: 20px;
    font-weight: 650;
  }
  .foot {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .fill {
    flex: 1;
  }
</style>
