<script lang="ts">
  import { useStore } from "../lib/context";
  import { enter } from "../lib/motion";
  import { formatHash } from "../lib/routes";
  import { onMount } from "svelte";
  import type { ShortcutsView } from "../lib/model";
  import { isMacPlatform, localAction, windowCommand } from "../lib/shortcuts";
  import Arrangement from "../screens/Arrangement.svelte";
  import CrossTest from "../screens/CrossTest.svelte";
  import Settings from "../screens/Settings.svelte";
  import Scene from "../screens/Scene.svelte";
  import Machine from "../screens/Machine.svelte";
  import Monitors from "../screens/Monitors.svelte";
  import Progress from "../screens/Progress.svelte";
  import Desk from "../screens/Desk.svelte";
  import Network from "../screens/Network.svelte";
  import States from "../screens/States.svelte";
  import Trouble from "../screens/Trouble.svelte";
  import Tunnels from "../screens/Tunnels.svelte";
  import Palette from "./Palette.svelte";
  import Sidebar from "./Sidebar.svelte";
  import Toasts from "./Toasts.svelte";
  import Wizard from "./Wizard.svelte";

  const store = useStore();
  const mac = isMacPlatform();
  /** Shortcuts que el sistema no dejó registrar: los atiende la ventana cuando está al frente. */
  let keys: ShortcutsView | null = null;
  onMount(() => {
    store.shortcuts().then(
      (v) => (keys = v),
      () => {},
    );
  });
  const key = $derived(formatHash(store.route));

  // ⌘K / Ctrl+K abre y cierra la palette desde cualquier pantalla; ⌘, / Ctrl+, va a Settings.
  function onkeydown(e: KeyboardEvent) {
    if (keys && !e.defaultPrevented) {
      const action = localAction(e, keys);
      if (action && windowCommand(action).kind === "hide_main") {
        e.preventDefault();
        void store.hideMain();
        return;
      }
    }
    const mod = mac ? e.metaKey : e.ctrlKey;
    if (!mod || e.altKey || e.shiftKey) return;
    const k = e.key.toLowerCase();
    if (k === "k") {
      e.preventDefault();
      store.togglePalette();
    } else if (k === ",") {
      e.preventDefault();
      store.go({ screen: "settings" });
    }
  }
</script>

<svelte:window {onkeydown} onhashchange={() => store.syncFromLocation()} />

<div class="shell" class:dim={store.paletteOpen} inert={store.paletteOpen || store.wizardOpen}>
  {#if store.route.screen === "crossing"}
    <!-- La prueba de crossing ocupa toda la ventana, con su ruta de vuelta (F-CrossingTest). -->
    {#key key}<CrossTest />{/key}
  {:else if store.route.screen === "scene"}
    <!-- El editor de scenes y el progress también ocupan toda la ventana (F-NewScene, F-Transactions). -->
    {#key key}<Scene id={store.route.id} />{/key}
  {:else if store.route.screen === "progress"}
    {#key key}<Progress />{/key}
  {:else}
  <Sidebar />
  {#key key}
    <main class="main" in:enter={{ y: 8, duration: 280 }}>
      {#if store.desk.trouble !== "none" && store.route.screen === "desk"}
        <Trouble />
      {:else if store.route.screen === "desk"}
        <Desk />
      {:else if store.route.screen === "tunnels"}
        <Tunnels />
      {:else if store.route.screen === "monitors"}
        <Monitors />
      {:else if store.route.screen === "arrangement"}
        <Arrangement />
      {:else if store.route.screen === "network"}
        <Network />
      {:else if store.route.screen === "machine"}
        <Machine machine={store.route.machine} />
      {:else if store.route.screen === "states"}
        <States />
      {:else}
        <Settings />
      {/if}
    </main>
  {/key}
  {/if}
</div>
<Palette />
<Wizard />
<Toasts />

<style>
  /* La caja de los mockups: barra lateral y contenido en un flex que se envuelve. */
  .shell {
    width: 100%;
    min-height: 100vh;
    display: flex;
    flex-wrap: wrap;
    font-family: "Inter", system-ui, sans-serif;
    color: #e8e8eb;
    font-size: 14px;
    background: #0e0f12;
    transition: opacity 0.25s;
  }
  /* Con la palette abierta, la app queda atrás tenue (F-Palette). */
  .shell.dim {
    opacity: 0.22;
  }
  .main {
    flex: 999 1 640px;
    min-width: 0;
    box-sizing: border-box;
    padding: 20px 28px 28px;
    display: flex;
    flex-direction: column;
  }
</style>
