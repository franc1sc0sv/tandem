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
    void store.start().then(() => store.maybeFirstRun());
  });
  onDestroy(() => store.stop());
</script>

<Shell />
