<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { provideStore } from "./lib/context";
  import type { Store } from "./lib/store/store.svelte";
  import Shell from "./shell/Shell.svelte";

  interface Props {
    store: Store;
  }
  let { store }: Props = $props();
  // svelte-ignore state_referenced_locally
  provideStore(store);

  onMount(() => {
    void store.start().then(() => {
      // El demo abre el asistente en el paso pedido (`?wizard=3`), para capturas y pruebas.
      const step = __DEMO__ ? new URLSearchParams(location.search).get("wizard") : null;
      if (step !== null && /^[0-5]$/.test(step)) {
        store.openWizard(Number(step));
        if (new URLSearchParams(location.search).has("crossed")) store.wizardCrossed = true;
      }
      else void store.maybeFirstRun();
    });
  });
  onDestroy(() => store.stop());
</script>

<Shell />
