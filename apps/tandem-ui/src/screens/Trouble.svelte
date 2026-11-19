<script lang="ts">
  // Trouble states (`V3-States`): base de T-112; T-120 trae el diseño completo de cada uno.
  import { useStore } from "../lib/context";
  import type { Trouble } from "../lib/store/types";
  import PageHeader from "../shell/PageHeader.svelte";
  import Button from "../ui/Button.svelte";
  import Card from "../ui/Card.svelte";
  import Icon from "../ui/Icon.svelte";
  import type { IconName } from "../ui/icons";

  const store = useStore();
  const desk = $derived(store.desk);
  const peer = $derived(desk.machines[desk.here === "a" ? "b" : "a"]);
  const ICON: Record<Trouble, IconName> = { none: "check", "no-agent": "warn", "peer-off": "power", "link-down": "wifi", permissions: "lock" };
</script>

<div class="page">
  <PageHeader title="Desk" />
  <Card tag="section" class="st" label={desk.troubleText}>
    <span class="ic"><Icon name={ICON[desk.trouble]} size={16} stroke={2.2} color="#9a9da6" /></span>
    <b>{desk.troubleText}</b>
    <span class="fill"></span>
    {#if desk.trouble === "peer-off" && peer && store.can("power")}
      <Button icon="power" variant="pri" onclick={() => store.setPower(peer.id, true)}>Power on {peer.name}</Button>
    {:else if desk.trouble === "link-down" && store.can("measure")}
      <Button icon="refresh" motion="spin" onclick={() => store.measureLink()}>Test again</Button>
    {:else if desk.trouble === "permissions"}
      <Button icon="arrow-right" motion="go" iconAfter onclick={() => store.openWizard(2)}>Review permissions</Button>
    {/if}
  </Card>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 16px;
    flex: 1;
  }
  .page :global(.card.st) {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .ic {
    width: 32px;
    height: 32px;
    border-radius: 9px;
    display: grid;
    place-items: center;
    background: #1b1c21;
  }
  .fill {
    flex: 1;
  }
</style>
