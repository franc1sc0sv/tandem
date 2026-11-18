<script lang="ts">
  import { useStore } from "../lib/context";
  import type { Machine } from "../lib/model";
  import { formatHash, type Route } from "../lib/routes";
  import Chip from "../ui/Chip.svelte";
  import Icon from "../ui/Icon.svelte";
  import type { IconName } from "../ui/icons";
  import Logo from "./Logo.svelte";

  const store = useStore();

  const MAIN: { route: Route; label: string; icon: IconName }[] = [
    { route: { screen: "puesto" }, label: "Puesto", icon: "puesto" },
    { route: { screen: "tuneles" }, label: "Túneles", icon: "tuneles" },
    { route: { screen: "monitores" }, label: "Monitores", icon: "monitores" },
    { route: { screen: "acomodo" }, label: "Acomodo", icon: "acomodo" },
    { route: { screen: "red" }, label: "Red", icon: "red" },
  ];

  const current = $derived(formatHash(store.route));
  const machines = $derived((["a", "b"] as Machine[]).map((id) => store.desk.machines[id]).filter((m) => m !== null));

  function go(e: MouseEvent, route: Route) {
    e.preventDefault();
    store.go(route);
  }
</script>

<!-- Barra lateral (D16, `V3-Puesto`): las pantallas, una página por máquina con su color y
     Ajustes abajo. Sin «Agregar máquina» (D1). -->
<nav aria-label="Principal" class="side">
  <!-- El nombre del puesto junto al logo solo en las páginas de máquina (`V3-Maquina`); las
       demás pantallas no lo dibujan (`V3-Puesto`, `V3-Red`…). -->
  <div class="brand">
    <Logo size={20} />Tandem{#if store.route.screen === "maquina"}<span class="fill"></span><span class="desk">{store.desk.deskName}</span>{/if}
  </div>
  {#each MAIN as item (item.label)}
    {@const href = formatHash(item.route)}
    <a class="nav" class:on={current === href} {href} aria-current={current === href ? "page" : undefined} onclick={(e) => go(e, item.route)}>
      <Icon name={item.icon} />{item.label}
      {#if item.route.screen === "tuneles" && store.activeTunnels > 0}<span class="fill"></span>{#if current === href}<span class="pdot" title="{store.activeTunnels} activos"></span>{:else}<Chip count dot={false}>{store.activeTunnels}</Chip>{/if}{/if}
    </a>
  {/each}
  {#if machines.length > 0}
    <div class="navh">Máquinas</div>
    {#each machines as m (m.id)}
      {@const route: Route = { screen: "maquina", machine: m.id }}
      {@const href = formatHash(route)}
      <a class="nav" class:on={current === href} {href} aria-current={current === href ? "page" : undefined} onclick={(e) => go(e, route)}>
        <span class="mdot" style="background:{m.color}"></span>{m.name}
      </a>
    {/each}
  {/if}
  <span class="fill"></span>
  <a class="nav" class:on={current === "#/ajustes"} href="#/ajustes" aria-current={current === "#/ajustes" ? "page" : undefined} onclick={(e) => go(e, { screen: "ajustes" })}>
    <Icon name="ajustes" />Ajustes
  </a>
</nav>

<style>
  .side {
    flex: 1 1 220px;
    max-width: 240px;
    box-sizing: border-box;
    padding: 16px 12px;
    border-right: 1px solid #1f2126;
    display: flex;
    flex-direction: column;
    gap: 2px;
    background: #0b0c0e;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 4px 10px 14px;
    font-weight: 650;
    font-size: 15px;
  }
  .desk {
    color: #9a9da6;
    font-size: 11.5px;
    font-weight: 400;
  }
  .nav {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border-radius: 9px;
    color: #a3a6af;
    text-decoration: none;
    font-size: 13.5px;
    min-height: 20px;
    transition:
      background 0.2s,
      color 0.2s;
  }
  .nav:hover {
    background: #17181c;
    color: #e8e8eb;
  }
  .nav.on {
    background: #1c1d22;
    color: #e8e8eb;
  }
  .nav:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: -2px;
  }
  .navh {
    font-size: 11px;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: #80838d;
    padding: 14px 10px 6px;
  }
  .mdot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
  }
  .fill {
    flex: 1;
  }
  /* En la pantalla Túneles el contador pasa a un punto vivo (`V3-Computo`). */
  .pdot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #a99be8;
    animation: sb-pl 1.8s infinite;
  }
  @keyframes sb-pl {
    0%,
    100% {
      box-shadow: 0 0 0 0 rgba(169, 155, 232, 0.6);
    }
    50% {
      box-shadow: 0 0 0 5px rgba(169, 155, 232, 0);
    }
  }
</style>
