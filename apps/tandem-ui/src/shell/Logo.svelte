<script lang="ts">
  interface Props {
    size?: number;
    stroke?: number;
  }

  let { size = 20, stroke = 2.4 }: Props = $props();
</script>

<!-- Dos anillos: los colores de marca, no los de cada máquina. -->
<svg width={size} height={size} viewBox="0 0 32 32" fill="none" aria-hidden="true"
  ><circle cx="11.5" cy="16" r="8" stroke="#7aa7e6" stroke-width={stroke}></circle><circle cx="20.5" cy="16" r="8" stroke="#a99be8" stroke-width={stroke}></circle></svg
>
