<script lang="ts">
  // The three indicators of `V3-Network`: latency ring (p50 and p99 of the input), bandwidth
  // ring and the 60 s chart with the peak marked and the note of the last change.
  import { LINK_NAME } from "../../lib/machines";
  import { bandwidthKpi, chartGeometry, chartTarget, formatMs, formatRate, latencyKpi, latencySeries, pathNote } from "../../lib/network";
  import type { NetworkView } from "../../lib/store/types";
  import Card from "../../ui/Card.svelte";
  import Ring from "../../ui/Ring.svelte";

  interface Props {
    network: NetworkView;
  }

  let { network }: Props = $props();

  const TONE = { ok: "#6cc59a", warn: "#d9a75a", bad: "#e07a6f" } as const;

  const latency = $derived(latencyKpi(network));
  const bandwidth = $derived(bandwidthKpi(network));
  const series = $derived(latencySeries(network));
  const chart = $derived(chartGeometry(series, chartTarget(network)));
  const note = $derived(pathNote(network, chart.peak !== null));
  const last = $derived([...series].reverse().find((v) => v !== null) ?? null);
  const chartLabel = $derived(
    chart.hasData ? `Latency over the last 60 s: now ${formatMs(last)}${chart.peak ? `, peak of ${formatMs(chart.peak.ms)}` : ""}` : "Latency over the last 60 s: no samples",
  );
</script>

<section class="kpis">
  <Card class="kpi" hover delay={0.06}>
    <Ring value={latency.load} color={TONE[latency.tone]} />
    <div class="col">
      <span class="mut lbl">Keyboard and mouse</span>
      <b class="mono big">{formatMs(latency.p50)}</b>
      <span class="mut mono sub">{latency.p99 === null ? "no input" : `p99 ${formatMs(latency.p99)}`}</span>
    </div>
  </Card>
  <Card class="kpi" hover delay={0.1}>
    <Ring value={bandwidth.fill} color="#7aa7e6" />
    <div class="col">
      <span class="mut lbl">Bandwidth</span>
      <b class="mono big">{formatRate(bandwidth.gbps)}</b>
      <span class="mut mono sub">{bandwidth.gbps === null ? "not measured" : `over ${LINK_NAME[bandwidth.via ?? "usb4"]}`}</span>
    </div>
  </Card>
  <Card class="chart" delay={0.14}>
    <div class="top">
      <span class="mut lbl">Latency · 60 s</span>
      <span class="fill"></span>
      {#if chart.targetMs !== null}<span class="mono mut tgt">target {formatMs(chart.targetMs)}</span>{/if}
    </div>
    <svg viewBox="0 0 400 74" width="100%" height="74" preserveAspectRatio="none" role="img" aria-label={chartLabel}>
      <defs>
        <linearGradient id="network-la" x1="0" x2="0" y1="0" y2="1"><stop offset="0" stop-color="#7aa7e6" stop-opacity=".3"></stop><stop offset="1" stop-color="#7aa7e6" stop-opacity="0"></stop></linearGradient>
      </defs>
      {#if chart.targetY !== null}<line x1="0" y1={chart.targetY} x2="400" y2={chart.targetY} stroke="#2a2c33" stroke-dasharray="3 5"></line>{/if}
      <path d={chart.area} fill="url(#network-la)"></path>
      <path class="ln" d={chart.line} fill="none" stroke="#7aa7e6" stroke-width="1.8"></path>
      {#if chart.peak}<circle class="pk" cx={chart.peak.x} cy={chart.peak.y} r="4" fill="#d9a75a"></circle>{/if}
    </svg>
    {#if note}
      <span class="note" class:info={note.tone === "info"}>
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" aria-hidden="true"><path d="M12 3 2 20h20z"></path></svg>{note.text}
      </span>
    {:else if !chart.hasData}
      <span class="note info">No samples yet</span>
    {/if}
  </Card>
</section>

<style>
  .kpis {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) minmax(0, 1.6fr);
    gap: 12px;
  }
  .kpis :global(.card.kpi) {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .kpis :global(.card.chart) {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .lbl {
    font-size: 12px;
  }
  .big {
    font-size: 22px;
    font-weight: 500;
  }
  .sub {
    font-size: 11.5px;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .fill {
    flex: 1;
  }
  .tgt {
    font-size: 11px;
  }
  .note {
    font-size: 11.5px;
    color: #e6c48f;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .note.info {
    color: #9a9da6;
  }
  /* The line is drawn on entering (`dr`); the peak pulses (`pk`). */
  .ln {
    stroke-dasharray: 900;
    stroke-dashoffset: 900;
    animation: dr 2s ease-out 0.2s forwards;
  }
  .pk {
    animation: pk 2s ease-in-out infinite;
  }
  @keyframes dr {
    to {
      stroke-dashoffset: 0;
    }
  }
  @keyframes pk {
    0%,
    100% {
      r: 4;
    }
    50% {
      r: 7;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .ln {
      stroke-dashoffset: 0;
    }
  }
</style>
