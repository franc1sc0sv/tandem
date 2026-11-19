<script lang="ts">
  // Crossing test (F-CrossingTest): asks to take the cursor to the other machine through the edge of the
  // saved arrangement and judges it with the crossings the input engine reports.
  import { onDestroy, onMount } from "svelte";
  import { SIDE_BY, SIDE_PHRASE, exitSide, fillOf, isVertical, mainOf, type Side } from "../lib/arrangement";
  import { useStore } from "../lib/context";
  import { CROSS_TIMEOUT_MS, formatMs, onSample, onTick, start, type CrossPhase } from "../lib/crossing";
  import { formatHash } from "../lib/routes";
  import Button from "../ui/Button.svelte";

  const store = useStore();
  const desk = $derived(store.desk);
  const here = $derived(desk.here);
  const peer = $derived(here === "a" ? "b" : "a");
  const from = $derived(mainOf(desk.layout.screens, here));
  const target = $derived(mainOf(desk.layout.screens, peer));
  const hereM = $derived(desk.machines[here]);
  const peerM = $derived(desk.machines[peer]);
  const expected: Side = $derived(exitSide(desk.layout.arrangement, here));
  const limit = store.crossTimeoutMs ?? CROSS_TIMEOUT_MS;
  /** What the user is told: the 20 s of the test (the demo can shorten it). */
  const seconds = Math.round(CROSS_TIMEOUT_MS / 1000);

  let phase: CrossPhase = $state(start(Date.now()));
  /** Each attempt restarts the countdown ring. */
  let attempt = $state(0);
  let stop: (() => void) | null = null;
  let timer: ReturnType<typeof setInterval> | undefined;

  async function begin() {
    stop?.();
    stop = null;
    phase = start(Date.now());
    attempt += 1;
    try {
      stop = await store.watchCrossing((s) => (phase = onSample(phase, here, expected, s)));
    } catch (e) {
      phase = { kind: "unavailable", reason: e instanceof Error ? e.message : String(e) };
    }
    phase = onTick(phase, Date.now(), limit);
  }

  onMount(() => {
    void begin();
    timer = setInterval(() => (phase = onTick(phase, Date.now(), limit)), 250);
  });
  onDestroy(() => {
    clearInterval(timer);
    stop?.();
  });

  const arrangement = formatHash({ screen: "arrangement" });
  const short = (name: string | undefined) => name?.split(" ")[0] ?? "";

  // The drawing: the cursor's machine and the other one, on the side the arrangement says.
  interface Scene {
    from: string;
    to: string;
    edge: string;
    arrow: string;
    turn: number;
    burst: string;
    dot: string;
  }
  const SCENES: Record<Side, Scene> = {
    right: {
      from: "left:6%;top:120px;width:34%;height:200px",
      to: "left:40%;top:50px;width:52%;height:330px",
      edge: "left:calc(40% - 3px);top:120px;width:6px;height:200px",
      arrow: "left:calc(40% - 90px);top:200px",
      turn: 0,
      burst: "left:calc(40% + 30px);top:198px",
      dot: "left:calc(40% + 36px);top:204px",
    },
    left: {
      from: "left:60%;top:120px;width:34%;height:200px",
      to: "left:8%;top:50px;width:52%;height:330px",
      edge: "left:calc(60% - 3px);top:120px;width:6px;height:200px",
      arrow: "left:calc(60% + 20px);top:200px",
      turn: 180,
      burst: "left:calc(60% - 54px);top:198px",
      dot: "left:calc(60% - 48px);top:204px",
    },
    up: {
      from: "left:33%;top:250px;width:34%;height:150px",
      to: "left:24%;top:30px;width:52%;height:220px",
      edge: "left:33%;top:247px;width:34%;height:6px",
      arrow: "left:calc(50% - 35px);top:285px",
      turn: -90,
      burst: "left:calc(50% - 12px);top:200px",
      dot: "left:calc(50% - 6px);top:206px",
    },
    down: {
      from: "left:33%;top:30px;width:34%;height:150px",
      to: "left:24%;top:180px;width:52%;height:220px",
      edge: "left:33%;top:177px;width:34%;height:6px",
      arrow: "left:calc(50% - 35px);top:105px",
      turn: 90,
      burst: "left:calc(50% - 12px);top:206px",
      dot: "left:calc(50% - 6px);top:212px",
    },
  };
  const scene = $derived(SCENES[expected]);
  const vertical = $derived(isVertical(expected));
</script>

<div class="cross">
  <nav aria-label="Path" class="mut crumbs"><a href={arrangement}>Arrangement</a> › Crossing test</nav>
  <div class="stage">
    <div class="desk" style="{scene.from};border:1.5px solid {hereM?.color ?? '#7aa7e6'};background:{fillOf(hereM?.color ?? '#7aa7e6')}">
      <b>{from?.name ?? hereM?.name ?? ""}</b><span class="mut">the cursor is here</span>
    </div>
    <div class="desk" style="{scene.to};border:1.5px solid {peerM?.color ?? '#a99be8'};background:{fillOf(peerM?.color ?? '#a99be8')}">
      <b>{target && target.short !== peerM?.name ? `${target.short} · ${peerM?.name ?? ""}` : (peerM?.name ?? "")}</b><span class="mut"
        >{phase.kind === "passed" ? "the cursor arrived here" : "waiting for the cursor"}</span
      >
    </div>
    {#if phase.kind === "waiting"}
      <div class="edge-glow" class:h={!vertical} style={scene.edge}></div>
      <span class="turn" style="{scene.arrow};transform:rotate({scene.turn}deg)">
        <svg class="arr" width="70" height="40" viewBox="0 0 70 40" aria-hidden="true"
          ><path d="M4 20h54M44 6l14 14-14 14" fill="none" stroke="#e8e8eb" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"></path></svg
        >
      </span>
    {:else if phase.kind === "passed"}
      <div class="edge-ok" style={scene.edge}></div>
      <span class="burst" style={scene.burst}></span>
      <span class="cur" style={scene.dot}></span>
    {/if}
  </div>

  <section aria-live="polite" class="result">
    {#if phase.kind === "waiting"}
      {#key attempt}
        <svg width="40" height="40" viewBox="0 0 40 40" aria-hidden="true" class="ring"
          ><circle cx="20" cy="20" r="18" fill="none" stroke="#25272d" stroke-width="3"></circle><circle
            class="cd"
            style="animation-duration:{limit / 1000}s"
            cx="20"
            cy="20"
            r="18"
            fill="none"
            stroke="#7aa7e6"
            stroke-width="3"
          ></circle></svg
        >
      {/key}
      <div class="txt">
        <b>Move the mouse {SIDE_PHRASE[expected]} the {short(hereM?.name)}</b><span class="mut sub"
          >You have {seconds} seconds. The glowing edge is where it should cross.</span
        >
      </div>
      <span class="fill"></span>
    {:else if phase.kind === "passed"}
      {@const s = phase.sample}
      <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="#6cc59a" stroke-width="2.2" aria-hidden="true"
        ><circle cx="12" cy="12" r="10"></circle><path d="m7.5 12.5 3 3 6-6.5"></path></svg
      >
      <div class="txt">
        <b>Crossed fine</b><span class="mut sub"
          >It entered {peerM?.name ?? "the other machine"}{#if s.of > 0}{" "}{vertical ? "at the right height" : "at the right place"} ({vertical ? "y" : "x"} = {s.at} of {s.of}){/if}.{#if s.ms !== null}{" "}Crossing latency: {formatMs(s.ms)}.{/if}</span
        >
      </div>
      <span class="fill"></span>
      <Button icon="refresh" motion="spin" size="lg" onclick={begin}>Test again</Button>
      <Button icon="check" variant="pri" size="lg" href={arrangement}>Done</Button>
    {:else}
      <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="#d9a75a" stroke-width="2.2" aria-hidden="true"
        ><path d="M12 3 2 20h20z"></path><path d="M12 10v4M12 17h.01"></path></svg
      >
      <div class="txt">
        {#if phase.kind === "timeout"}
          <b class="warn">Did not cross in {seconds} s</b><span class="mut sub"
            >Maybe {target?.ask ?? "the other screen"} is on another side of your desk. Move it in the arrangement and test again.</span
          >
        {:else if phase.kind === "wrong-edge"}
          <b class="warn">Crossed at another edge</b><span class="mut sub"
            >Crossed at {SIDE_BY[phase.sample.edge]}, not {SIDE_BY[expected]}: the arrangement does not match your desk. Move it in the arrangement and test again.</span
          >
        {:else}
          <b class="warn">The crossing cannot be tested</b><span class="mut sub">{phase.reason}</span>
        {/if}
      </div>
      <span class="fill"></span>
      <Button icon="edit" size="lg" href={arrangement}>Edit arrangement</Button>
      {#if phase.kind !== "unavailable"}<Button icon="refresh" motion="spin" variant="pri" size="lg" onclick={begin}>Test again</Button>{/if}
    {/if}
  </section>
</div>

<style>
  .cross {
    width: 100%;
    min-height: 100vh;
    box-sizing: border-box;
    padding: 24px 32px;
    display: flex;
    flex-direction: column;
    gap: 18px;
    font-family: "Inter", system-ui, sans-serif;
    color: #e8e8eb;
    font-size: 14px;
    background: #0e0f12;
  }
  .mut {
    color: #9a9da6;
  }
  .crumbs {
    font-size: 12.5px;
  }
  .crumbs a {
    color: #9cc0f0;
  }
  .stage {
    position: relative;
    height: 430px;
    border-radius: 16px;
    border: 1px solid #1f2126;
    background-color: #0b0c0e;
    background-image: radial-gradient(#1c1e23 1px, transparent 1px);
    background-size: 20px 20px;
    overflow: hidden;
  }
  .desk {
    position: absolute;
    border-radius: 12px;
    box-sizing: border-box;
    padding: 14px;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    gap: 4px;
    font-size: 13px;
  }
  .desk b {
    font-weight: 600;
  }
  .desk .mut {
    font-size: 12px;
  }
  /* The edge the cursor should cross through pulses. */
  .edge-glow {
    position: absolute;
    border-radius: 3px;
    background: linear-gradient(180deg, transparent, #7aa7e6, transparent);
    animation: eg 1.4s ease-in-out infinite;
  }
  .edge-glow.h {
    background: linear-gradient(90deg, transparent, #7aa7e6, transparent);
  }
  .edge-ok {
    position: absolute;
    border-radius: 3px;
    background: #6cc59a;
  }
  .turn {
    position: absolute;
    width: 70px;
    height: 40px;
  }
  .arr {
    display: block;
    animation: nud 1.2s ease-in-out infinite;
  }
  .burst {
    position: absolute;
    width: 24px;
    height: 24px;
    border-radius: 50%;
    border: 2px solid #6cc59a;
    animation: bs 1.2s ease-out infinite;
  }
  .cur {
    position: absolute;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: #e8e8eb;
  }
  .result {
    border-radius: 14px;
    background: #15161a;
    border: 1px solid #25272d;
    padding: 18px 20px;
    display: flex;
    gap: 16px;
    align-items: center;
    flex-wrap: wrap;
    animation: in 0.35s ease-out;
  }
  .ring {
    transform: rotate(-90deg);
    flex: none;
  }
  .cd {
    stroke-dasharray: 113;
    animation: cnt 20s linear forwards;
  }
  .txt {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .txt b {
    font-weight: 600;
  }
  .txt b.warn {
    color: #e6c48f;
  }
  .sub {
    font-size: 12.5px;
  }
  .fill {
    flex: 1;
  }
  @keyframes eg {
    0%,
    100% {
      opacity: 0.25;
    }
    50% {
      opacity: 1;
    }
  }
  @keyframes nud {
    0%,
    100% {
      transform: translateX(0);
    }
    50% {
      transform: translateX(10px);
    }
  }
  @keyframes cnt {
    to {
      stroke-dashoffset: 113;
    }
  }
  @keyframes bs {
    from {
      transform: scale(0.4);
      opacity: 1;
    }
    to {
      transform: scale(3);
      opacity: 0;
    }
  }
  @keyframes in {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
</style>
