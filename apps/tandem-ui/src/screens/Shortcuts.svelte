<script lang="ts">
  // Ajustes → Atajos (`F-Ajustes--atajos`): una fila por atajo, que se guarda al grabarlo.
  // La ventana (ui.toml) y el bloqueo y las escenas (el puesto compartido) se editan aquí;
  // la paleta es de la ventana y no se cambia.
  import { onMount } from "svelte";
  import { useStore } from "../lib/context";
  import { clashOf, codesOf, readLockCombo, rowsOf, type AtajoRow, type RowId } from "../lib/atajos";
  import type { Bindings, ShortcutsView } from "../lib/model";
  import { formatAccel, isMacPlatform, readCombo } from "../lib/shortcuts";

  const store = useStore();
  const mac = isMacPlatform();

  let view = $state<ShortcutsView | null>(null);
  let recording: RowId | null = $state(null);
  let error: string | null = $state(null);
  let busy = $state(false);

  const rows = $derived(
    rowsOf({
      toggle: view?.bindings.toggle_window ?? null,
      lock: store.desk.lockShortcut,
      palette: mac ? "Super+KeyK" : "Ctrl+KeyK",
      scenes: store.desk.scenes.map((s) => ({ id: s.id, name: s.name, shortcut: s.shortcut })),
    }),
  );

  onMount(async () => {
    try {
      view = await store.shortcuts();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  });

  function record(r: AtajoRow) {
    if (!r.editable) return;
    error = null;
    recording = recording === r.id ? null : r.id;
  }

  async function apply(r: AtajoRow, accel: string): Promise<string | null> {
    if (r.id === "toggle_window") {
      const next: Bindings = { ...(view?.bindings ?? { toggle_window: null }), toggle_window: accel };
      view = await store.setShortcuts(next);
      store.toast("Atajo guardado");
      return null;
    }
    if (r.id === "lock") {
      const res = await store.setLockShortcut(codesOf(accel));
      return res.ok ? null : res.reason;
    }
    const scene = store.desk.scenes.find((s) => `scene:${s.id}` === r.id);
    if (!scene?.raw) return "Esa escena ya no existe";
    const res = await store.saveScene({ ...scene.raw, shortcut: codesOf(accel) }, false);
    return res.ok ? null : res.reason;
  }

  async function onkeydown(e: KeyboardEvent, r: AtajoRow) {
    if (recording !== r.id || busy) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape" && !e.ctrlKey && !e.altKey && !e.metaKey) {
      recording = null;
      return;
    }
    const c = r.id === "lock" ? readLockCombo(e) : readCombo(e);
    if (c.kind === "partial") return;
    if (c.kind === "invalid") {
      error = c.reason;
      return;
    }
    const dup = clashOf(rows, r.id, c.accel);
    if (dup) {
      error = `Ya lo usa «${dup.label}»`;
      return;
    }
    busy = true;
    try {
      error = await apply(r, c.accel);
      if (!error) recording = null;
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }
</script>

{#if view?.warning}<p class="warn">{view.warning}</p>{/if}
<p class="mut intro">Funcionan en cualquier máquina del puesto, aunque Tandem esté en segundo plano. Haz clic y pulsa la combinación nueva.</p>
{#each rows as r (r.id)}
  <div class="row">
    <span class="fill col">
      {r.label}
      {#if r.id === "toggle_window" && view?.failed.toggle_window}<span class="bad">El sistema no lo dejó registrar: {view.failed.toggle_window}. Funciona con la ventana al frente.</span>{/if}
    </span>
    <button
      type="button"
      class="combo"
      class:rec={recording === r.id}
      disabled={!r.editable}
      aria-label="Atajo de «{r.label}»: {recording === r.id ? 'pulsa la combinación nueva' : formatAccel(r.accel, mac) || 'sin atajo'}"
      aria-pressed={recording === r.id}
      onclick={() => record(r)}
      onkeydown={(e) => onkeydown(e, r)}>{recording === r.id ? "Pulsa…" : formatAccel(r.accel, mac) || "—"}</button
    >
  </div>
{/each}
{#if error}<p class="bad" role="alert">{error}</p>{/if}

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 14px 0;
    border-top: 1px solid #1f2126;
    font-size: 13.5px;
    flex-wrap: wrap;
  }
  .intro {
    margin: 0 0 8px;
    font-size: 12.5px;
  }
  .fill {
    flex: 1;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  /* El campo de la combinación (F-Ajustes). */
  .combo {
    font-family: "JetBrains Mono", monospace;
    font-size: 12.5px;
    padding: 6px 12px;
    border-radius: 8px;
    background: #111215;
    border: 1px solid #31343b;
    box-sizing: content-box;
    min-width: 110px;
    text-align: center;
    color: #e8e8eb;
    cursor: pointer;
    transition: border-color 0.2s;
  }
  .combo:disabled {
    cursor: default;
  }
  .combo:not(:disabled):hover {
    border-color: #4a4e58;
  }
  .combo.rec {
    border-style: dashed;
    border-color: #7aa7e6;
    animation: combo-bd 1.6s ease-in-out infinite;
  }
  .combo:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .bad {
    color: #f0b1aa;
    font-size: 12.5px;
    margin: 0;
  }
  .warn {
    color: #e6c48f;
    font-size: 12.5px;
    margin: 0 0 6px;
  }
  @keyframes combo-bd {
    50% {
      border-color: #4a4e58;
    }
  }
</style>
