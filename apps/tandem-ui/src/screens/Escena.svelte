<script lang="ts">
  // New scene y editar escena (`F-NewScene`): nombre, qué muestra cada monitor compatible,
  // atajo y vista previa. Ocupa toda la ventana, con su ruta de vuelta a Monitores.
  import { untrack } from "svelte";
  import { useStore } from "../lib/context";
  import { KEEP_WALL, previewWall } from "../lib/monitors";
  import { formatHash } from "../lib/routes";
  import { codesOfAccel, compatible, draftOf, newDraft, problemOf, sceneOf, shortcutText, type Draft, type Pick } from "../lib/scenes";
  import { readCombo } from "../lib/shortcuts";
  import Button from "../ui/Button.svelte";

  interface Props {
    /** `null`: escena nueva. */
    id: string | null;
  }
  let { id }: Props = $props();

  const store = useStore();
  const desk = $derived(store.desk);
  const monitors = $derived(compatible(desk.monitors));
  const mac = $derived(desk.machines[desk.here]?.os === "macos");
  const machines = $derived((["a", "b"] as const).flatMap((m) => (desk.machines[m] ? [{ id: m, name: desk.machines[m].name, color: desk.machines[m].color }] : [])));
  const existing = $derived(id === null ? null : (desk.scenes.find((s) => s.id === id) ?? null));
  const others = $derived(desk.scenes.filter((s) => s.id !== id));

  // El borrador se arma una vez, con lo que hay al abrir (o cuando llega la escena a editar).
  let draft = $state<Draft | null>(null);
  $effect(() => {
    if (draft !== null || !desk.ready) return;
    if (id === null) draft = untrack(() => newDraft(desk.monitors));
    else if (existing) draft = untrack(() => draftOf(existing, desk.monitors));
  });
  // Una escena que no existe (se borró en the other machine, o la dirección está mal).
  $effect(() => {
    if (id !== null && desk.ready && desk.scenes.length > 0 && !existing && draft === null) {
      store.toast("That scene no longer exists", "bad");
      store.go({ screen: "monitores" });
    }
  });

  const problem = $derived(draft ? problemOf(draft, others) : null);
  /** El motivo de un campo, si es de ese campo (el nombre vacío no se grita antes de escribir). */
  const why = (field: "name" | "shortcut"): string | null => (problem && problem.field === field && !(field === "name" && draft?.name === "") ? problem.text : null);
  let busy = $state(false);
  const home = formatHash({ screen: "monitores" });

  const optionsOf = (): { value: Pick; label: string; color: string }[] => [...machines.map((m) => ({ value: m.id as Pick, label: m.name, color: m.color })), { value: "keep", label: "Keep as is", color: "#6a6d76" }];
  const wallFor = (p: Pick) => (p === "keep" ? KEEP_WALL : previewWall(desk.machines[p]?.color ?? "#9a9da6"));

  function choose(monitor: string, p: Pick) {
    if (draft) draft = { ...draft, picks: { ...draft.picks, [monitor]: p } };
  }

  // Grupo de radio: las flechas mueven y eligen.
  function arrows(e: KeyboardEvent, monitor: string, now: Pick) {
    const opts = optionsOf();
    const i = opts.findIndex((o) => o.value === now);
    const to = { ArrowRight: i + 1, ArrowDown: i + 1, ArrowLeft: i - 1, ArrowUp: i - 1 }[e.key];
    if (to === undefined) return;
    e.preventDefault();
    const next = opts[(to + opts.length) % opts.length];
    if (next) {
      choose(monitor, next.value);
      (e.currentTarget as HTMLElement).parentElement?.querySelectorAll<HTMLElement>("[role=radio]")[(to + opts.length) % opts.length]?.focus();
    }
  }

  let shortcutNote = $state("");
  /** El campo del atajo escucha la combinación mientras tiene el foco: Retroceso lo quita. */
  function capture(e: KeyboardEvent) {
    if (!draft || e.key === "Tab") return;
    e.preventDefault();
    if (e.key === "Backspace" || e.key === "Delete") {
      draft = { ...draft, shortcut: [] };
      shortcutNote = "";
      return;
    }
    const c = readCombo(e);
    if (c.kind === "ok") {
      draft = { ...draft, shortcut: codesOfAccel(c.accel) };
      shortcutNote = "";
    } else if (c.kind === "invalid") shortcutNote = c.reason;
  }

  async function save(apply: boolean) {
    if (!draft || problem) return;
    busy = true;
    const scene = sceneOf(draft);
    const r = await store.saveScene(scene, draft.id === null);
    if (r.ok && apply) {
      const target = scene.id || (await store.waitForScene(scene.name));
      if (target && (await store.applyScene(target)).ok) store.showProgress();
      else store.toast(`“${scene.name}” was not found to apply it`, "bad");
    }
    busy = false;
    if (r.ok && !apply) store.go({ screen: "monitores" });
  }

  const shortcut = $derived(draft ? shortcutText(draft.shortcut, mac) : "");
</script>

<div class="wrap">
  <nav aria-label="Path" class="mut crumbs"><a href={home}>Monitors</a> › {id === null ? "New scene" : "Edit scene"}</nav>
  {#if draft}
    {@const d = draft}
    <div class="cols">
      <section class="form">
        <label class="f"
          ><span class="lbl">Name</span><input
            value={d.name}
            maxlength="60"
            aria-invalid={why("name") ? "true" : undefined}
            oninput={(e) => (draft = { ...d, name: e.currentTarget.value })}
          />{#if why("name")}<span class="err" role="alert">{why("name")}</span>{/if}</label
        >
        {#each monitors as m (m.id)}
          {@const now = d.picks[m.id] ?? "keep"}
          <div class="f">
            <span class="lbl">{m.name} shows</span>
            <div class="seg" role="radiogroup" aria-label="What the {m.name.match(/\d+/)?.[0] ?? m.name} shows">
              {#each optionsOf() as o (o.value)}
                <button type="button" role="radio" class="sb" class:on={o.value === now} aria-checked={o.value === now} tabindex={o.value === now ? 0 : -1} onclick={() => choose(m.id, o.value)} onkeydown={(e) => arrows(e, m.id, now)}
                  ><span class="dot" style="background:{o.color}"></span>{o.label}</button
                >
              {/each}
            </div>
          </div>
        {/each}
        <div class="f">
          <span class="lbl" id="atajo">Shortcut</span>
          <div class="krow">
            <button type="button" class="kbd" aria-labelledby="atajo" onkeydown={capture}>{shortcut || "No shortcut"}</button>
            <span class="mut note">{shortcutNote || "Press the new combination. It works on any machine of the desk."}</span>
          </div>
          {#if why("shortcut")}<span class="err" role="alert">{why("shortcut")}</span>{/if}
        </div>
        <div class="acts">
          <Button icon="x" variant="ghost" size="lg" href={home} style="--btn-fs:13.5px;--btn-px:16px">Cancel</Button>
          <span class="fill"></span>
          <Button icon="play" size="lg" disabled={busy || problem !== null || !store.can("scenes")} onclick={() => save(true)} style="--btn-fs:13.5px;--btn-px:16px">Save and apply</Button>
          <Button icon="check" variant="pri" size="lg" disabled={busy || problem !== null} onclick={() => save(false)} style="--btn-fs:13.5px;--btn-px:16px">Save</Button>
        </div>
      </section>

      <section class="prev" aria-label="Preview">
        <span class="lbl self">Preview</span>
        <div class="stages">
          {#each monitors as m (m.id)}
            {@const now = d.picks[m.id] ?? "keep"}
            <div class="stage">
              <div class="scr" style="width:300px;height:126px"><div class="wall" style="background:{wallFor(now)}"></div></div>
              <div class="neck"></div>
              <div class="base"></div>
              <span class="cap">{m.name.match(/\d+"/)?.[0] ?? m.name} · <b>{now === "keep" ? "Keep as is" : (desk.machines[now]?.name ?? "")}</b></span>
            </div>
          {/each}
        </div>
        <p class="mut hint">When applied, each monitor confirms its change. If one fails, the whole scene is rolled back.</p>
      </section>
    </div>
  {/if}
</div>

<style>
  .wrap {
    width: 100%;
    min-height: 860px;
    box-sizing: border-box;
    padding: 24px 32px;
    display: flex;
    flex-direction: column;
    gap: 18px;
    font-family: "Inter", system-ui, sans-serif;
    color: #e8e8eb;
    font-size: 14px;
  }
  .crumbs {
    font-size: 12.5px;
  }
  .crumbs a {
    color: #9cc0f0;
  }
  .cols {
    display: flex;
    gap: 22px;
    flex-wrap: wrap;
    align-items: flex-start;
  }
  .form {
    flex: 1 1 460px;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  .f {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .f:has(.seg),
  .f:has(.krow) {
    gap: 8px;
  }
  .lbl {
    font-size: 12px;
    color: #9a9da6;
  }
  input {
    font: inherit;
    font-size: 16px;
    color: #e8e8eb;
    background: #111215;
    border: 1px solid #31343b;
    border-radius: 10px;
    padding: 12px 14px;
    transition:
      border-color 0.2s,
      box-shadow 0.2s;
  }
  input:focus {
    outline: none;
    border-color: #5b8fd4;
    box-shadow: 0 0 0 4px rgba(122, 167, 230, 0.15);
  }
  input[aria-invalid="true"] {
    border-color: #5a3532;
  }
  .err {
    font-size: 12.5px;
    color: #f0b1aa;
  }
  .seg {
    position: relative;
    display: flex;
    padding: 3px;
    border-radius: 10px;
    background: #111215;
    border: 1px solid #25272d;
    gap: 3px;
    flex-wrap: wrap;
  }
  .sb {
    flex: 1;
    font: inherit;
    font-size: 13px;
    padding: 8px 12px;
    border-radius: 8px;
    border: 0;
    background: transparent;
    color: #9a9da6;
    cursor: pointer;
    min-height: 38px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    transition:
      background 0.25s,
      color 0.25s;
  }
  .sb.on {
    background: #1c1d22;
    color: #e8e8eb;
    box-shadow: inset 0 0 0 1px #4a4e58;
  }
  .sb:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }
  .krow {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .kbd {
    font-family: "JetBrains Mono", monospace;
    font-size: 13px;
    padding: 8px 14px;
    border-radius: 9px;
    border: 1px dashed #4a4e58;
    background: #111215;
    color: #e8e8eb;
    min-width: 120px;
    /* En el mockup es un <span>: el ancho mínimo no incluye el relleno. */
    box-sizing: content-box;
    text-align: center;
    cursor: text;
    animation: es-bd 1.6s ease-in-out infinite;
  }
  .kbd:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .note {
    font-size: 12.5px;
  }
  .acts {
    display: flex;
    gap: 10px;
  }
  .fill {
    flex: 1;
  }
  .prev {
    flex: 1 1 380px;
    min-width: 0;
    border-radius: 14px;
    background: #15161a;
    border: 1px solid #25272d;
    padding: 22px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    align-items: center;
  }
  .self {
    align-self: flex-start;
  }
  .stages {
    display: flex;
    align-items: flex-end;
    gap: 22px;
    padding: 16px 0;
  }
  .stage {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
  }
  .scr {
    border-radius: 9px;
    padding: 6px;
    background: #0b0c0e;
    border: 1px solid #31343b;
    box-sizing: border-box;
    transition: transform 0.5s cubic-bezier(0.3, 1.4, 0.5, 1);
  }
  .wall {
    width: 100%;
    height: 100%;
    border-radius: 4px;
    transition: background 0.6s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .neck {
    width: 8px;
    height: 16px;
    background: #22242a;
  }
  .base {
    width: 80px;
    height: 5px;
    border-radius: 4px;
    background: #22242a;
  }
  .cap {
    font-size: 12.5px;
  }
  .cap b {
    font-weight: 600;
  }
  .hint {
    margin: 0;
    font-size: 12.5px;
    line-height: 1.5;
    text-align: center;
    max-width: 340px;
  }
  @keyframes es-bd {
    50% {
      border-color: #7aa7e6;
    }
  }
</style>
