<script lang="ts">
  import type { Snippet } from "svelte";

  interface Props {
    /** Retraso de la animación de entrada, en segundos (las tarjetas entran escalonadas). */
    delay?: number;
    /** Con `href` la tarjeta entera es un link y se eleva al pasar. */
    href?: string;
    /** Se eleva al pasar aunque no sea link. */
    hover?: boolean;
    label?: string;
    tag?: "div" | "section" | "article";
    class?: string;
    style?: string;
    children: Snippet;
  }

  let { delay = 0, href, hover = false, label, tag = "div", class: cls = "", style = "", children }: Props = $props();
  const st = $derived(`${delay ? `animation-delay:${delay}s;` : ""}${style}`);
</script>

<!-- Sin línea de color arriba (DESIGN §12): el acento va en el contenido. -->
{#if href}
  <a class="card lift {cls}" {href} style={st} aria-label={label}>{@render children()}</a>
{:else}
  <svelte:element this={tag} class="card {cls}" class:lift={hover} style={st} aria-label={label}>{@render children()}</svelte:element>
{/if}

<style>
  .card {
    display: block;
    box-sizing: border-box;
    background: #15161a;
    border: 1px solid #25272d;
    border-radius: 14px;
    padding: var(--card-pad, 16px);
    animation: card-up 0.45s cubic-bezier(0.2, 0.8, 0.2, 1) both;
    position: relative;
    overflow: hidden;
    color: inherit;
    text-decoration: none;
  }
  .lift {
    transition:
      border-color 0.25s,
      transform 0.25s;
  }
  .lift:hover {
    border-color: #3a3d45;
    transform: translateY(-2px);
  }
  a.card:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  @keyframes card-up {
    from {
      opacity: 0;
      transform: translateY(10px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
</style>
