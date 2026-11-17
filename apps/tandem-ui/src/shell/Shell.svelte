<script lang="ts">
  import { useStore } from "../lib/context";
  import { enter } from "../lib/motion";
  import { formatHash } from "../lib/routes";
  import { isMacPlatform } from "../lib/shortcuts";
  import Acomodo from "../screens/Acomodo.svelte";
  import Ajustes from "../screens/Ajustes.svelte";
  import Maquina from "../screens/Maquina.svelte";
  import Monitores from "../screens/Monitores.svelte";
  import Puesto from "../screens/Puesto.svelte";
  import Red from "../screens/Red.svelte";
  import Trouble from "../screens/Trouble.svelte";
  import Tuneles from "../screens/Tuneles.svelte";
  import Palette from "./Palette.svelte";
  import Sidebar from "./Sidebar.svelte";
  import Toasts from "./Toasts.svelte";
  import Wizard from "./Wizard.svelte";

  const store = useStore();
  const mac = isMacPlatform();
  const key = $derived(formatHash(store.route));

  // ⌘K / Ctrl+K abre y cierra la paleta desde cualquier pantalla; ⌘, / Ctrl+, va a Ajustes.
  function onkeydown(e: KeyboardEvent) {
    const mod = mac ? e.metaKey : e.ctrlKey;
    if (!mod || e.altKey || e.shiftKey) return;
    const k = e.key.toLowerCase();
    if (k === "k") {
      e.preventDefault();
      store.togglePalette();
    } else if (k === ",") {
      e.preventDefault();
      store.go({ screen: "ajustes" });
    }
  }
</script>

<svelte:window {onkeydown} onhashchange={() => store.syncFromLocation()} />

<div class="shell" class:dim={store.paletteOpen} inert={store.paletteOpen || store.wizardOpen}>
  <Sidebar />
  {#key key}
    <main class="main" in:enter={{ y: 8, duration: 280 }}>
      {#if store.desk.trouble !== "none" && store.route.screen === "puesto"}
        <Trouble />
      {:else if store.route.screen === "puesto"}
        <Puesto />
      {:else if store.route.screen === "tuneles"}
        <Tuneles />
      {:else if store.route.screen === "monitores"}
        <Monitores />
      {:else if store.route.screen === "acomodo"}
        <Acomodo />
      {:else if store.route.screen === "red"}
        <Red />
      {:else if store.route.screen === "maquina"}
        <Maquina machine={store.route.machine} />
      {:else}
        <Ajustes />
      {/if}
    </main>
  {/key}
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
  /* Con la paleta abierta, la app queda atrás tenue (F-Paleta). */
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
