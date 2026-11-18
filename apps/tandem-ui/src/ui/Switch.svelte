<script lang="ts">
  interface Props {
    checked: boolean;
    /** Nombre accesible. */
    label: string;
    disabled?: boolean;
    onchange?: (checked: boolean) => void;
  }

  let { checked = $bindable(), label, disabled = false, onchange }: Props = $props();

  function toggle() {
    if (disabled) return;
    checked = !checked;
    onchange?.(checked);
  }
</script>

<!-- Un <button role="switch">: Espacio y Enter lo cambian solos (F-Settings). -->
<button type="button" class="sw" class:off={!checked} role="switch" aria-checked={checked} aria-label={label} {disabled} onclick={toggle}></button>

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
</style>
