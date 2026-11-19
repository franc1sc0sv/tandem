<script lang="ts">
  // Changes in progress (`F-Transactions`): los pasos de una transacción uno por uno, lo que
  // dijo the monitor y la vuelta atrás visible si algo falla. Ocupa toda la ventana.
  import { useStore } from "../lib/context";
  import { formatHash } from "../lib/routes";
  import Button from "../ui/Button.svelte";
  import Card from "../ui/Card.svelte";
  import Icon from "../ui/Icon.svelte";
  import { energyView, measureKind, measureView } from "../lib/progress";
  import type { RowIcon } from "../lib/txn";

  const store = useStore();
  const view = $derived(store.runView());
  const energy = $derived(
    energyView(
      store.powerRun,
      (m) => store.desk.machines[m]?.name ?? m,
      store.desk.machines[store.desk.here]?.name ?? "this machine",
      store.clock,
    ),
  );
  const kind = $derived(measureKind(store.desk.network, store.measured));
  const meter = $derived(kind ? measureView(store.desk.network, kind) : null);
  const back = formatHash({ screen: "monitors" });

  // Sin ninguna transacción que mostrar (se abrió la dirección a pelo): de vuelta a Monitors.
  $effect(() => {
    if (store.txnReady && store.run === null && store.powerRun === null && !meter) store.go({ screen: "monitors" });
  });

  const WORD: Record<RowIcon, string> = { done: "done", run: "in progress", bad: "failed", todo: "pending", back: "rolling back" };
</script>

<div class="wrap">
  <div class="top">
    <h1>Changes in progress</h1>
    <span class="mut sub">Everything that touches another machine is a transaction: visible steps, a time limit and rollback if something fails.</span>
  </div>
  {#if view || energy || meter}
    <div class="grid">
      {#if view}
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
        <div class="acts"><Button icon="arrow-left" size="md" motion="back" href={back} style="--btn-fs:12.5px;--btn-px:13px">Back to Monitors</Button></div>
      </Card>
      {/if}
      {#if energy}
        <Card tag="section" class="txn" label={energy.title} style="--card-pad:18px">
          <h2>{energy.title}</h2>
          <div class="wv" aria-hidden="true">
            <span class="wave"></span><span class="wave b"></span><span class="wave c"></span>
            <span class="pw"><Icon name="power" size={22} stroke={2.2} linecap="round" /></span>
          </div>
          <div aria-live="polite" class="rows">
            {#each energy.rows as r (r.key)}
              <div class="s" class:dim={r.dim}>
                <span class="i {r.icon}" role="img" aria-label={WORD[r.icon]}>{#if r.icon === "done"}<Icon name="check" size={10} stroke={3} />{/if}</span><span>{r.text}</span><span class="fill"></span><span class="mut d" class:mono={r.mono}>{r.detail}</span>
              </div>
            {/each}
          </div>
        </Card>
      {/if}
      {#if meter}
        <Card tag="section" class="txn" label={meter.title} style="--card-pad:18px">
          <h2>{meter.title}</h2>
          <span class="mut sub">3 s test over the data path. It does not slow the keyboard or mouse.</span>
          <div class="big"><b class="mono">{meter.value}</b><span class="u">{meter.unit}</span><span class="fill"></span><span class="mono mut lat">{meter.latency}</span></div>
          <div class="meter" role="img" aria-label={meter.aria}><span style="--w:{meter.percent}%"></span></div>
          <div class="mono mut scale"><span>0</span><span>{meter.caption}</span></div>
          <div class="oth">
            {#each meter.others as o (o.name)}<span class="mut">{o.name}</span><span class="mono">{o.text}</span>{/each}
          </div>
        </Card>
      {/if}
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
  .s.dim {
    color: #9a9da6;
  }
  .wv {
    position: relative;
    height: 150px;
    display: grid;
    place-items: center;
  }
  .wave {
    position: absolute;
    left: 50%;
    top: 50%;
    width: 40px;
    height: 40px;
    margin: -20px;
    border-radius: 50%;
    border: 2px solid #d9a75a;
    animation: pr-wv 1.8s ease-out infinite;
  }
  .wave.b {
    animation-delay: 0.6s;
  }
  .wave.c {
    animation-delay: 1.2s;
  }
  .pw {
    width: 54px;
    height: 54px;
    border-radius: 50%;
    background: #1b1c21;
    border: 1px solid #5a4a2c;
    display: grid;
    place-items: center;
    position: relative;
    color: #d9a75a;
  }
  .big {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 12px 0;
  }
  .big b {
    font-size: 40px;
    font-weight: 500;
    animation: pr-up 0.5s ease-out;
  }
  .u {
    font-size: 16px;
  }
  .lat {
    font-size: 12.5px;
  }
  .meter {
    height: 10px;
    border-radius: 5px;
    background: #24262c;
    overflow: hidden;
  }
  .meter span {
    display: block;
    height: 100%;
    width: 0;
    background: linear-gradient(90deg, #7aa7e6, #a99be8);
    animation: pr-fl 3s cubic-bezier(0.2, 0.8, 0.2, 1) forwards;
  }
  .scale {
    display: flex;
    justify-content: space-between;
    font-size: 11px;
  }
  .oth {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
    font-size: 12.5px;
    padding-top: 6px;
  }
  .done :global(svg) {
    color: #6cc59a;
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
  @media (prefers-reduced-motion: reduce) {
    .meter span {
      width: var(--w);
    }
  }
  @keyframes pr-sp {
    to {
      transform: rotate(360deg);
    }
  }
  @keyframes pr-wv {
    from {
      transform: scale(0.5);
      opacity: 0.9;
    }
    to {
      transform: scale(3.4);
      opacity: 0;
    }
  }
  @keyframes pr-fl {
    to {
      width: var(--w);
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
