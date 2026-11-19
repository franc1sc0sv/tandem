<script lang="ts">
  interface Props {
    checked: boolean;
    /** Accessible name. */
    label: string;
    disabled?: boolean;
    /**
     * Fixed, not momentary (for example the agent cannot turn that module off): it cannot be
     * toggled but stays focusable, is not dimmed and reads like the mockup's switch. Unlike
     * `disabled`, it does not use the native attribute.
     */
    locked?: boolean;
    /** Why it is locked, as the tooltip and the accessible description. */
    hint?: string;
    onchange?: (checked: boolean) => void;
  }

  let { checked = $bindable(), label, disabled = false, locked = false, hint, onchange }: Props = $props();
  const hintId = $props.id();

  function toggle() {
    if (disabled || locked) return;
    checked = !checked;
    onchange?.(checked);
  }
</script>

<!-- A <button role="switch">: Space and Enter toggle it by themselves (F-Settings). Locked: still a focusable button, read-only. -->
<button
  type="button"
  class="sw"
  class:off={!checked}
  class:locked={locked && !disabled}
  role="switch"
  aria-checked={checked}
  aria-label={label}
  aria-readonly={locked ? "true" : undefined}
  aria-disabled={locked ? "true" : undefined}
  aria-describedby={locked && hint ? hintId : undefined}
  title={locked ? hint : undefined}
  {disabled}
  onclick={toggle}
></button>
{#if locked && hint}<span id={hintId} class="sr-only">{hint}</span>{/if}

<style>
  .sw {
    width: 38px;
    height: 22px;
    border-radius: 11px;
    position: relative;
    border: 0;
    cursor: pointer;
    flex: none;
    background: #2f6a52;
    transition: background 0.25s;
  }
  .sw::after {
    content: "";
    position: absolute;
    top: 3px;
    left: 19px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #e8e8eb;
    transition: left 0.3s cubic-bezier(0.3, 1.4, 0.5, 1);
  }
  .sw.off {
    background: #31343b;
  }
  .sw.off::after {
    left: 3px;
  }
  .sw:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .sw:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .sw.locked {
    cursor: default;
  }
</style>
