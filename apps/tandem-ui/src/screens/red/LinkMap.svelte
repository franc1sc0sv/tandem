<script lang="ts" module>
  import type { Os } from "../../lib/model";

  export interface MapNode {
    name: string;
    os: Os;
    color: string;
  }
</script>

<script lang="ts">
  // El mapa de enlaces de `V3-Red`: las dos máquinas con su halo y un camino por enlace. El
  // que responde fluye (rápido el cable, lento el Wi-Fi); el que no, va punteado.
  import { LINK_NAME } from "../../lib/machines";
  import { describePath, formatMs, isUp } from "../../lib/network";
  import type { LinkKind, NetworkView } from "../../lib/store/types";
  import Card from "../../ui/Card.svelte";
  import Icon from "../../ui/Icon.svelte";
  import { machineIcon, type IconName } from "../../ui/icons";

  interface Props {
    network: NetworkView;
    a: MapNode;
    b: MapNode;
  }

  let { network, a, b }: Props = $props();

  // Cada camino tiene su ruta: USB-C arriba, Ethernet recto y Wi-Fi abajo.
  const ROUTE: Record<LinkKind, string> = {
    usb4: "M0 85 C 120 85, 120 30, 200 30 S 280 85, 400 85",
    ethernet: "M0 85 L 400 85",
    wifi: "M0 85 C 120 85, 120 140, 200 140 S 280 85, 400 85",
  };
  const TOP: Record<LinkKind, number> = { usb4: 18, ethernet: 50, wifi: 82 };
  const BASE: Record<LinkKind, number> = { usb4: 10, ethernet: 8, wifi: 8 };
  const ICON: Record<LinkKind, IconName> = { usb4: "bolt", ethernet: "ethernet", wifi: "wifi" };
  /** Color de cada enlace cuando responde. */
  const TINT: Record<LinkKind, string> = { usb4: "#7aa7e6", ethernet: "#5fbfb0", wifi: "#d9a75a" };
  /** Borde del rótulo del camino que lleva el tráfico. */
  const EDGE: Record<LinkKind, string> = { usb4: "#3a4a66", ethernet: "#2c4a47", wifi: "#4a3e26" };

  const label = $derived(`Tres caminos: ${network.paths.map(describePath).join(", ")}`);
</script>

<Card tag="section" label="Mapa de enlaces" class="map">
  {@render node(a)}
  <div class="paths" role="img" aria-label={label}>
    <svg width="100%" height="170" viewBox="0 0 400 170" preserveAspectRatio="none" aria-hidden="true">
      <defs>
        <linearGradient id="red-g1" x1="0" x2="1"><stop offset="0" stop-color={a.color}></stop><stop offset="1" stop-color={b.color}></stop></linearGradient>
      </defs>
      {#each network.paths as p (p.kind)}
        {#if isUp(p)}
          <path d={ROUTE[p.kind]} fill="none" stroke="#1f2126" stroke-width={BASE[p.kind]} stroke-linecap="round"></path>
          <path
            class="flow {p.kind}"
            d={ROUTE[p.kind]}
            fill="none"
            stroke={p.kind === "usb4" ? "url(#red-g1)" : TINT[p.kind]}
            stroke-opacity={p.kind === "usb4" ? 1 : p.kind === "wifi" ? 0.7 : 0.8}
            stroke-width={p.kind === "usb4" ? 3.5 : p.kind === "wifi" ? 2 : 2.5}
          ></path>
        {:else}
          <path d={ROUTE[p.kind]} fill="none" stroke={p.state === "down" ? "#5a3532" : "#2a2c33"} stroke-width="2" stroke-dasharray="4 7"></path>
        {/if}
      {/each}
    </svg>
    {#each network.paths as p (p.kind)}
      {@const up = isUp(p)}
      <span class="ptag" class:dead={!up} style="top:{TOP[p.kind]}%;{p.state === 'primary' ? `border-color:${EDGE[p.kind]}` : ''}">
        <Icon name={ICON[p.kind]} size={13} stroke={2} color={up ? TINT[p.kind] : "#80838d"} />
        {#if !up}
          <span class="mut">{LINK_NAME[p.kind]} · {p.state === "down" ? "caído" : "sin cable"}</span>
        {:else}
          {#if p.state === "primary"}<b>{LINK_NAME[p.kind]}</b>{:else}<span>{LINK_NAME[p.kind]}</span>{/if}
          {#if p.rttMs !== null}<span class="mono mut">{formatMs(p.rttMs)}</span>{/if}
        {/if}
      </span>
    {/each}
  </div>
  {@render node(b)}
</Card>

{#snippet node(m: MapNode)}
  <div class="node" style="--c:{m.color}">
    <span class="nic"><Icon name={machineIcon(m.os)} size={27} stroke={1.6} color={m.color} /></span>
    <b>{m.name}</b>
  </div>
{/snippet}

<style>
  :global(.card.map) {
    display: flex;
    align-items: center;
    gap: 8px;
    --card-pad: 20px 24px;
  }
  .node {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    flex: none;
    width: 120px;
    text-align: center;
  }
  .node b {
    font-size: 13px;
    font-weight: 600;
  }
  .nic {
    width: 60px;
    height: 60px;
    border-radius: 17px;
    display: grid;
    place-items: center;
    background: #111215;
    border: 1px solid #2a2c33;
    position: relative;
  }
  /* El halo de cada máquina respira (`halo`). */
  .nic::after {
    content: "";
    position: absolute;
    inset: -6px;
    border-radius: 21px;
    border: 1px solid var(--c);
    opacity: 0.35;
    animation: halo 2.6s ease-in-out infinite;
  }
  .paths {
    flex: 1;
    min-width: 0;
    position: relative;
    height: 170px;
  }
  .ptag {
    position: absolute;
    left: 50%;
    transform: translate(-50%, -50%);
    display: flex;
    align-items: center;
    gap: 7px;
    height: 28px;
    padding: 0 11px;
    border-radius: 999px;
    background: #0e0f12;
    border: 1px solid #2a2c33;
    font-size: 12px;
    white-space: nowrap;
    transition: transform 0.25s;
  }
  .ptag.dead {
    opacity: 0.6;
  }
  .ptag:hover {
    transform: translate(-50%, -50%) scale(1.05);
  }
  .ptag b {
    font-weight: 600;
  }
  .flow {
    stroke-linecap: round;
  }
  /* El flujo de cada camino: un trazo corriendo; los períodos son múltiplos del punteado para
     que el ciclo no dé un salto. */
  .flow.usb4 {
    stroke-dasharray: 3 12;
    animation: fa-usb 0.8s linear infinite;
  }
  .flow.ethernet {
    stroke-dasharray: 3 14;
    animation: fa-eth 0.8s linear infinite;
  }
  .flow.wifi {
    stroke-dasharray: 2 16;
    animation: fa-wifi 2.4s linear infinite;
  }
  @keyframes fa-usb {
    to {
      stroke-dashoffset: -30;
    }
  }
  @keyframes fa-eth {
    to {
      stroke-dashoffset: -34;
    }
  }
  @keyframes fa-wifi {
    to {
      stroke-dashoffset: -36;
    }
  }
  @keyframes halo {
    0%,
    100% {
      transform: scale(1);
      opacity: 0.35;
    }
    50% {
      transform: scale(1.06);
      opacity: 0.08;
    }
  }
</style>
