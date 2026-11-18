<script lang="ts">
  import type { Snippet } from "svelte";

  interface Props {
    /** `raised`: con el borde de abajo más grueso (V3-Monitors). */
    raised?: boolean;
    style?: string;
    children: Snippet;
  }

  let { raised = false, style = "", children }: Props = $props();
</script>

<kbd class="kbd" class:raised {style}>{@render children()}</kbd>

<style>
  .kbd {
    font-family: "JetBrains Mono", monospace;
    font-size: 11px;
    color: #9a9da6;
    border: 1px solid #31343b;
    border-radius: 5px;
    padding: 1px 5px;
  }
  .raised {
    border-bottom-width: 2px;
    border-radius: 6px;
    padding: 2px 6px;
  }
</style>
