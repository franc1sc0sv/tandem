<script lang="ts">
  import type { Snippet } from "svelte";

  interface Props {
    /** Delay of the entrance animation, in seconds (the cards enter staggered). */
    delay?: number;
    /** With `href` the whole card is a link and lifts on hover. */
    href?: string;
    /** Lifts on hover even if it is not a link. */
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

<!-- No color line on top (DESIGN §12): the accent goes in the content. -->
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
