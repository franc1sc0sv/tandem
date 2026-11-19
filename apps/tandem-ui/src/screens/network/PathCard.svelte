<script lang="ts">
  // The card of a path (`V3-Network`): its icon, its state and the latency and bandwidth bars.
  import { LINK_NAME } from "../../lib/machines";
  import { stateLabel, bandwidthBar, barColors, formatMs, formatRate, isUp, latencyBar } from "../../lib/network";
  import type { LinkKind, PathView } from "../../lib/store/types";
  import Bar from "../../ui/Bar.svelte";
  import Card from "../../ui/Card.svelte";
  import Icon from "../../ui/Icon.svelte";
  import type { IconName } from "../../ui/icons";

  interface Props {
    path: PathView;
    delay: number;
  }

  let { path, delay }: Props = $props();

  const ICON: Record<LinkKind, IconName> = { usb4: "bolt", ethernet: "ethernet", wifi: "wifi" };
  const TINT: Record<LinkKind, string> = { usb4: "#7aa7e6", ethernet: "#5fbfb0", wifi: "#d9a75a" };
  /** What each path is called on its card (on the map and the chart, just "USB-C"). */
  const TITLE: Record<LinkKind, string> = { usb4: "USB-C cable", ethernet: "Ethernet", wifi: "Wi-Fi" };
  /** Icon background when the path responds; no cable is gray. */
  const WELL: Record<LinkKind, string> = { usb4: "#16202e", ethernet: "#12201d", wifi: "#1a1712" };

  const up = $derived(isUp(path));
  const colors = $derived(barColors(path.state));
  const style = $derived(path.state === "primary" ? "border-color:#2c4a3c" : path.state === "unplugged" ? "border-style:dashed" : "");
</script>

<Card tag="article" hover class="pc" {delay} {style}>
  <div class="head">
    <span class="pi" style="background:{up ? WELL[path.kind] : '#1b1c21'}"><Icon name={ICON[path.kind]} size={18} stroke={2} color={up ? TINT[path.kind] : "#80838d"} /></span>
    <div class="who">
      <b>{TITLE[path.kind]}</b>
      {#if path.detail}<span class="mono mut det">{path.detail}</span>{/if}
    </div>
    <span class="fill"></span>
    <span class="st {path.state}"><span class="d"></span>{stateLabel(path)}</span>
  </div>
  <div class="met" class:off={!up}>
    <span class="mut">Latency</span>
    <Bar percent={up ? latencyBar(path.rttMs) : 0} color={colors.latency} label={up ? `${LINK_NAME[path.kind]} latency` : undefined} />
    <span class="mono val">{up ? formatMs(path.rttMs) : "—"}</span>
    <span class="mut">Bandwidth</span>
    <Bar percent={up ? bandwidthBar(path.kind, path.gbps) : 0} color={colors.bandwidth} label={up ? `${LINK_NAME[path.kind]} bandwidth` : undefined} />
    <span class="mono val">{up ? formatRate(path.gbps) : "—"}</span>
  </div>
</Card>

<style>
  :global(.card.pc) {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  :global(.card.pc.lift:hover) {
    transform: translateY(-3px);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .who {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .who b {
    font-weight: 600;
  }
  .det {
    font-size: 11px;
  }
  .fill {
    flex: 1;
  }
  .pi {
    width: 38px;
    height: 38px;
    border-radius: 11px;
    display: grid;
    place-items: center;
    flex: none;
    transition: transform 0.3s cubic-bezier(0.3, 1.5, 0.5, 1);
  }
  :global(.card.pc:hover) .pi {
    transform: rotate(-8deg) scale(1.08);
  }
  /* Uniform status chip: Primary green with pulse, Backup amber, No cable dotted. */
  .st {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 26px;
    padding: 0 11px;
    border-radius: 999px;
    font-size: 12px;
    font-weight: 500;
    white-space: nowrap;
    border: 1px solid;
  }
  .st .d {
    width: 7px;
    height: 7px;
    border-radius: 50%;
  }
  .st.primary {
    color: #9fd8b9;
    border-color: #2c4a3c;
    background: #121a16;
  }
  .st.primary .d {
    background: #6cc59a;
    animation: pg 1.8s infinite;
  }
  .st.backup {
    color: #e6c48f;
    border-color: #4a3e26;
    background: #1a1712;
  }
  .st.backup .d {
    background: #d9a75a;
  }
  .st.unplugged {
    color: #80838d;
    border-color: #2a2c33;
    border-style: dashed;
    background: transparent;
  }
  .st.unplugged .d {
    background: #4a4d55;
  }
  .st.down {
    color: #f0b1aa;
    border-color: #5a3532;
    background: #1c1516;
  }
  .st.down .d {
    background: #e07a6f;
  }
  .met {
    display: grid;
    grid-template-columns: 78px minmax(0, 1fr) 70px;
    gap: 8px 10px;
    align-items: center;
    font-size: 12px;
  }
  .met.off {
    opacity: 0.45;
  }
  .val {
    text-align: right;
  }
  @keyframes pg {
    0%,
    100% {
      box-shadow: 0 0 0 0 rgba(108, 197, 154, 0.55);
    }
    50% {
      box-shadow: 0 0 0 4px rgba(108, 197, 154, 0);
    }
  }
</style>
