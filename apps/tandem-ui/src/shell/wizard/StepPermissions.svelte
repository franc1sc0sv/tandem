<script lang="ts">
  // Paso 3 (`F-Asis3`): los permisos de macOS que `tandemd` necesita, pedidos y verificados de
  // verdad (T-090): el agente los vuelve a leer solo y la tarjeta pasa a concedida.
  import { onMount } from "svelte";
  import { useStore } from "../../lib/context";
  import { permCards } from "../../lib/wizard";
  import Button from "../../ui/Button.svelte";
  import Icon from "../../ui/Icon.svelte";

  interface Props {
    /** El usuario ya abrió algún panel: ofrece reiniciar el agente. */
    onOpened: () => void;
  }
  let { onOpened }: Props = $props();

  const store = useStore();
  const perms = $derived(store.desk.permissions);
  const cards = $derived(permCards(perms.rows));
  const here = $derived(store.desk.machines[store.desk.here]);

  // Entrar al paso con permisos pendientes los pide: macOS muestra su diálogo a nombre de tandemd.
  onMount(() => {
    if (perms.rows.some((r) => r.state === "missing")) void store.requestPermissions();
  });

  async function grant(id: (typeof cards)[number]["id"]) {
    onOpened();
    await store.openPermission(id);
  }
</script>

<div class="wz-head">
  <span class="wz-mut wz-sm">Paso 3 de 6{here ? ` · ${here.name}` : ""}</span>
  <h1>{perms.needed ? "Permisos de macOS" : "Permisos"}</h1>
  {#if perms.needed}
    <p class="wz-mut wz-lead" style="max-width:600px">
      Se dan a <span class="wz-mono" style="font-size:12.5px;color:#c4c6cc">tandemd</span>, no a la ventana. Tandem los detecta solo en cuanto los concedes: no hace falta volver aquí.
    </p>
  {/if}
</div>
{#if perms.needed}
  {#each cards as p (p.id)}
    <div class="wz-perm" class:granted={p.granted}>
      <span class="wz-dot {p.granted ? 'got' : p.missing ? 'wait' : ''}"></span>
      <div style="display:flex;flex-direction:column;gap:5px;flex:1;min-width:0">
        <b style="font-weight:600">{p.title}</b>
        <span class="wz-mut" style="font-size:13px;line-height:1.5">{p.why}</span>
        <span style="font-size:12.5px;color:{p.granted ? '#9fd8b9' : p.missing ? '#e6c48f' : '#9a9da6'}"
          >{p.granted ? "Concedido · comprobado ahora" : p.missing ? "Esperando… se comprueba solo" : "Sin comprobar · macOS lo pide la primera vez que se use"}</span
        >
      </div>
      {#if !p.granted}
        <Button icon="external" size="sm" motion="lift" class="wz-sbtn" onclick={() => grant(p.id)}>Abrir ajustes</Button>
      {:else}
        <span class="wz-tick"><Icon name="check" size={17} stroke={2.6} color="#6cc59a" /></span>
      {/if}
    </div>
  {/each}
{:else}
  <div class="wz-perm granted">
    <span class="wz-dot got"></span>
    <div style="display:flex;flex-direction:column;gap:5px;flex:1;min-width:0">
      <b style="font-weight:600">Esta máquina no necesita permisos</b>
      <span class="wz-mut" style="font-size:13px;line-height:1.5">Windows deja capturar e inyectar el teclado y el mouse sin pedir nada.</span>
    </div>
    <span class="wz-tick"><Icon name="check" size={17} stroke={2.6} color="#6cc59a" /></span>
  </div>
{/if}
