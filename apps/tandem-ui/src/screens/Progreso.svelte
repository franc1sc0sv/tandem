<script lang="ts">
  // Cambios en progreso (`F-Transacciones`): los pasos de una transacción uno por uno, lo que
  // dijo el monitor y la vuelta atrás visible si algo falla. Ocupa toda la ventana.
  import { useStore } from "../lib/context";
  import { formatHash } from "../lib/routes";
  import Button from "../ui/Button.svelte";
  import Card from "../ui/Card.svelte";
  import type { RowIcon } from "../lib/txn";

  const store = useStore();
  const view = $derived(store.runView());
  const back = formatHash({ screen: "monitores" });

  // Sin ninguna transacción que mostrar (se abrió la dirección a pelo): de vuelta a Monitores.
  $effect(() => {
    if (store.txnReady && store.run === null) store.go({ screen: "monitores" });
  });

  const WORD: Record<RowIcon, string> = { done: "listo", run: "en curso", bad: "falló", todo: "pendiente", back: "deshaciendo" };
</script>

<div class="wrap">
  <div class="top">
    <h1>Cambios en progreso</h1>
    <span class="mut sub">Todo lo que toca a otra máquina es una transacción: pasos visibles, un tiempo máximo y vuelta atrás si algo falla.</span>
  </div>
  {#if view}
    <div class="grid">
      <Card tag="section" hover class="txn" label={view.title} style="--card-pad:18px">
        <div class="hd"><h2>{view.title}</h2><span class="fill"></span><span class="mono mut el">{view.elapsed}</span></div>
        <div aria-live="polite" class="rows">
          {#each view.rows as r (r.key)}
            <div class="s" class:bad={r.tone === "bad"}>
              <span class="i {r.icon}" role="img" aria-label={WORD[r.icon]}></span><span>{r.text}</span><span class="fill"></span><span class="mut d">{r.detail}</span>
            </div>
          {/each}
        </div>
        <p class="msg {view.message.tone}">{view.message.text}</p>
        <div class="acts"><Button icon="arrow-left" size="md" motion="back" href={back} style="--btn-fs:12.5px;--btn-px:13px">Volver a Monitores</Button></div>
      </Card>
    </div>
  {/if}
</div>

<style>
  .wrap {
    width: 100%;
    min-height: 860px;
    box-sizing: border-box;
    padding: 28px 32px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    font-family: "Inter", system-ui, sans-serif;
    color: #e8e8eb;
    font-size: 14px;
  }
  .top {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  h1 {
    margin: 0;
    font-size: 20px;
    font-weight: 650;
  }
  .sub {
    font-size: 12.5px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(360px, 1fr));
    gap: 14px;
  }
  .grid :global(.card.txn) {
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-width: 0;
    overflow: visible;
  }
  .hd {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  h2 {
    margin: 0;
    font-size: 14.5px;
    font-weight: 600;
  }
  .el {
    font-size: 12px;
  }
  .fill {
    flex: 1;
  }
  .rows {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .s {
    display: flex;
    align-items: center;
    gap: 12px;
    font-size: 13.5px;
    padding: 7px 0;
    animation: pr-up 0.35s ease-out both;
  }
  .s.bad {
    color: #f0b1aa;
  }
  .d {
    font-size: 12px;
  }
  .i {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    flex: none;
    box-sizing: border-box;
    display: grid;
    place-items: center;
  }
  .run {
    border: 2px solid #31343b;
    border-top-color: #d9a75a;
    animation: pr-sp 0.8s linear infinite;
  }
  .done {
    background: #1e3a2e;
  }
  .bad .i,
  .i.bad {
    background: #3a1e1e;
  }
  .todo {
    border: 1.5px solid #31343b;
  }
  .back {
    border: 2px solid #31343b;
    border-top-color: #9cc0f0;
    animation: pr-sp 0.8s linear infinite reverse;
  }
  .msg {
    margin: 0;
    font-size: 13px;
  }
  .msg.mut {
    color: #9a9da6;
  }
  .msg.ok {
    color: #9fd8b9;
  }
  .msg.warn {
    color: #e6c48f;
  }
  .acts {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  @keyframes pr-sp {
    to {
      transform: rotate(360deg);
    }
  }
  @keyframes pr-up {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
</style>
