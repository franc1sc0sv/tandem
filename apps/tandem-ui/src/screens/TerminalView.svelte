<script lang="ts">
  // A process's live output: plain text for a piped process (the look of `V3-Compute`) and a
  // terminal emulator (xterm.js) for a terminal process, whose bytes carry escape sequences. The
  // output is pushed by the agent as it arrives and only appended: the view is created once per process (the parent keys it by
  // tunnel and process) and nothing else (desk updates, link switches, the process list refreshing)
  // touches it. If the watch fails it is asked for again from where it was, never from zero.
  import { onMount } from "svelte";
  import { useStore } from "../lib/context";
  import type { LiveChunk, OutputCursor } from "../lib/store/types";
  import { plainScreen, terminalScreen, type Screen } from "../lib/term";
  import { continues } from "../lib/tunnels";

  let { tunnel, process }: { tunnel: string; process: number } = $props();
  const store = useStore();
  let host: HTMLElement;
  /** The emulator replaced the plain screen (it draws its own padding, colors and cursor). */
  let emulated = $state(false);

  /** How long to wait before asking again after a failure, or between empty answers when polling. */
  const RETRY_MS = 1000;
  const IDLE_POLL_MS = 250;

  onMount(() => {
    let screen: Screen = plainScreen(host);
    let wrote = false;
    let cursor: OutputCursor = { stdout: 0, stderr: 0 };
    let alive = true;
    let stop: (() => void) | null = null;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const feed = (c: LiveChunk) => {
      if (!alive || !continues(cursor, c)) return;
      if (c.pty && !wrote && !emulated) {
        const term = terminalScreen(host);
        if (term) {
          screen.dispose();
          screen = term;
          emulated = true;
        }
      }
      wrote = true;
      screen.setPty(c.pty);
      if (c.bytes.length > 0) screen.write(c.bytes);
      cursor = c.next;
    };
    const watch = async () => {
      try {
        const s = await store.watchOutput(tunnel, process, cursor, feed);
        if (alive) stop = s;
        else s();
      } catch {
        if (alive) timer = setTimeout(watch, RETRY_MS);
      }
    };
    // A source that does not push: ask again right away while there is output, a little later
    // when there is none.
    const encoder = new TextEncoder();
    const poll = async () => {
      let next = IDLE_POLL_MS;
      try {
        const c = await store.output(tunnel, process, cursor);
        if (!alive) return;
        if (c.text !== "") {
          feed({ bytes: encoder.encode(c.text), from: cursor, next: c.next, done: c.done, pty: false });
          next = 0;
        }
        if (c.done) return;
      } catch {
        next = RETRY_MS;
      }
      if (alive) timer = setTimeout(poll, next);
    };
    void (store.pushesOutput ? watch() : poll());
    const resized = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => screen.fit());
    resized?.observe(host);
    return () => {
      alive = false;
      clearTimeout(timer);
      stop?.();
      resized?.disconnect();
      screen.dispose();
    };
  });
</script>

<div class="term" class:mono={!emulated} class:emulated bind:this={host} data-testid="terminal"></div>

<style>
  .term {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 12px 14px;
    font-size: 12px;
    line-height: 1.7;
    color: #c4c6cc;
    white-space: pre-wrap;
    background: #0a0b0d;
  }
  .term.emulated {
    padding: 8px 10px;
    white-space: normal;
  }
  .term :global(.caret) {
    display: inline-block;
    width: 7px;
    height: 14px;
    background: #c4c6cc;
    vertical-align: -2px;
    animation: tu-bl 1s steps(1) infinite;
  }
  @keyframes tu-bl {
    50% {
      opacity: 0;
    }
  }
</style>
