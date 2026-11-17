<script lang="ts">
  // Galería de componentes (solo en builds con modo demo): el arnés compara sus estilos
  // calculados con los de los mismos elementos en los mockups (`scripts/style-parity.ts`).
  import Bar from "./ui/Bar.svelte";
  import Button from "./ui/Button.svelte";
  import Card from "./ui/Card.svelte";
  import Chip from "./ui/Chip.svelte";
  import Kbd from "./ui/Kbd.svelte";
  import Ring from "./ui/Ring.svelte";
  import Segmented from "./ui/Segmented.svelte";
  import Switch from "./ui/Switch.svelte";

  const three = [
    { value: "i", label: "Inicio" },
    { value: "c", label: "Centro" },
    { value: "f", label: "Fin" },
  ];
  const two = [
    { value: "a", label: "MacBook Pro", icon: "laptop" as const, iconColor: "#7aa7e6", accent: "#4f7cbc" },
    { value: "b", label: "PC-Escritorio", icon: "tower" as const, iconColor: "#a99be8", accent: "#6a5fa6" },
  ];
</script>

<div class="gal">
  <div data-part="btn-md"><Button icon="check">Botón</Button></div>
  <div data-part="btn-pri"><Button icon="check" variant="pri">Primario</Button></div>
  <div data-part="btn-ghost"><Button icon="x" variant="ghost">Fantasma</Button></div>
  <div data-part="btn-sm"><Button icon="check" size="sm">Chico</Button></div>
  <div data-part="btn-danger"><Button icon="trash" variant="danger" size="sm" style="--btn-h:34px;--btn-px:13px">Peligro</Button></div>
  <div data-part="btn-xl"><Button icon="arrow-right" variant="pri" size="xl">Grande</Button></div>
  <div data-part="seg-md"><Segmented options={three} value="c" label="Alineación" /></div>
  <div data-part="seg-lg"><Segmented options={two} value="a" label="Máquina" size="lg" /></div>
  <div data-part="sw-on"><Switch checked={true} label="Encendido" /></div>
  <div data-part="sw-off"><Switch checked={false} label="Apagado" /></div>
  <div data-part="chip"><Chip dot={false}>Trabajo</Chip></div>
  <div data-part="chip-count"><Chip count dot={false}>3</Chip></div>
  <div data-part="card"><Card>Tarjeta</Card></div>
  <div data-part="kbd"><Kbd>⌃⌥1</Kbd></div>
  <div data-part="kbd-raised"><Kbd raised>⌃⌥1</Kbd></div>
  <div data-part="bar"><Bar percent={50} color="#7aa7e6" /></div>
  <div data-part="ring"><Ring value={0.5} color="#6cc59a" /></div>
</div>

<style>
  .gal {
    display: flex;
    flex-wrap: wrap;
    gap: 16px;
    padding: 24px;
    font-family: "Inter", system-ui, sans-serif;
    font-size: 14px;
    color: #e8e8eb;
  }
  .gal > div {
    width: 300px;
  }
</style>
