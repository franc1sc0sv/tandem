<script lang="ts">
  // Monitors and scenes (`V3-Monitors`): each compatible monitor with its real input and who
  // controls it, the incompatible ones (D11) with their reason, and the scenes as cards.
  import { useStore } from "../lib/context";
  import { wallOf } from "../lib/arrangement";
  import { DIM_WALL } from "../lib/desk";
  import { INPUT_LABEL, KEEP_WALL, accentOf, formatClock, isDim, linkText, shortMonitor, whyNotSwitch } from "../lib/monitors";
  import { formatHash } from "../lib/routes";
  import type { Machine } from "../lib/model";
  import type { MonitorEventView, MonitorUi } from "../lib/store/types";
  import PageHeader from "../shell/PageHeader.svelte";
  import Button from "../ui/Button.svelte";
  import Card from "../ui/Card.svelte";
  import Chip from "../ui/Chip.svelte";
  import Icon from "../ui/Icon.svelte";
  import type { IconName } from "../ui/icons";
  import { machineIcon } from "../ui/icons";
  import Kbd from "../ui/Kbd.svelte";
  import Segmented from "../ui/Segmented.svelte";
  import type { SegOption } from "../ui/Segmented.svelte";
  import Thumb from "../ui/Thumb.svelte";

  const store = useStore();
  const desk = $derived(store.desk);
  const machines = $derived((["a", "b"] as Machine[]).flatMap((id) => (desk.machines[id] ? [desk.machines[id]] : [])));
  const compatible = $derived(desk.monitors.filter((m) => m.compat.ok));
  const others = $derived(desk.monitors.filter((m) => !m.compat.ok));
  const nameOf = (m: Machine | null) => (m ? (desk.machines[m]?.name ?? "") : "");
  const colorOf = (m: Machine | null) => (m ? (desk.machines[m]?.color ?? "#9a9da6") : "#9a9da6");
  const wall = (m: Machine | null) => (m ? wallOf(colorOf(m)) : KEEP_WALL);

  /** What was requested and the monitor has not confirmed yet: the indicator already goes, but the truth is the monitor's. */
  let pending: Record<string, { machine: Machine; since: number }> = $state({});

  function options(m: MonitorUi): SegOption[] {
    return machines.map((x) => ({
      value: x.id,
      label: x.name,
      icon: machineIcon(x.os),
      iconColor: x.color,
      note: m.ports[x.id] ? INPUT_LABEL[m.ports[x.id] as keyof typeof INPUT_LABEL] : undefined,
      accent: accentOf(x.color),
    }));
  }

  /** "PC-Desktop · DP": who controls it and through which input it arrives. */
  const ownerText = (m: MonitorUi): string => {
    const port = m.owner ? m.ports[m.owner] : undefined;
    return `${nameOf(m.owner)}${port ? ` · ${INPUT_LABEL[port]}` : ""}`;
  };

  /** Which machine the selector marks: what was requested while the monitor decides, otherwise what it shows. */
  const selected = (m: MonitorUi): string => pending[m.id]?.machine ?? m.shows ?? "";

  async function pick(m: MonitorUi, who: string) {
    const target = who as Machine;
    if (target === m.shows && !pending[m.id]) return;
    const input = m.ports[target];
    const why = whyNotSwitch(m, target, nameOf(target));
    if (!input || why) {
      store.toast(why ?? `It is not known which input ${nameOf(target)} uses`, "bad");
      return;
    }
    pending[m.id] = { machine: target, since: Date.now() };
    const r = await store.switchMonitor(m.id, input);
    if (!r.ok) delete pending[m.id];
  }

  // The monitor confirmed (or the change ended and was undone): the selector goes back to showing the truth.
  $effect(() => {
    const run = store.run;
    for (const m of desk.monitors) {
      const p = pending[m.id];
      if (!p) continue;
      if (m.shows === p.machine || (run?.endedAt != null && run.startedAt >= p.since - 1000)) delete pending[m.id];
    }
  });

  const EV_ICON: Record<MonitorEventView["kind"], { icon: IconName; color: string; stroke: number }> = {
    switch: { icon: "arrow-right", color: "#7aa7e6", stroke: 2.2 },
    button: { icon: "hand", color: "#d9a75a", stroke: 2 },
    signal: { icon: "pulse", color: "#9a9da6", stroke: 2 },
    failed: { icon: "x", color: "#e07a6f", stroke: 2 },
    boot: { icon: "power", color: "#9a9da6", stroke: 2 },
    power: { icon: "power", color: "#9a9da6", stroke: 2 },
    other: { icon: "info", color: "#9a9da6", stroke: 2 },
  };

  /** "34 inches" in the accessible label, like the mockup. */
  const spoken = (name: string) => shortMonitor(name).replace('"', " inches");

  /** "Apply" leads to the step-by-step progress (F-Transactions). */
  async function apply(id: string) {
    const r = await store.applyScene(id);
    if (r.ok) store.showProgress();
  }

  let explained: Record<string, boolean> = $state({});
  const editHref = (id: string) => formatHash({ screen: "scene", id });
  const newHref = formatHash({ screen: "scene", id: null });
</script>

<div class="page">
  <PageHeader title="Monitors" search={false} />

  {#if desk.monitors.length > 0}
    <div class="grid">
      {#each compatible as m, i (m.id)}
        {@const state = linkText(m.link, nameOf)}
        <Card tag="article" delay={i * 0.08} class="mon" style="--card-pad:18px" label={m.name}>
          <div class="head">
            <Thumb width={132} height={58} wall={isDim(m.link) ? DIM_WALL : wall(m.shows)} dim={isDim(m.link)} flashKey={m.shows ?? ""} label={state ?? undefined} />
            <div class="who">
              <b>{m.name}</b>
              <span class="chips">
                <Chip dot={false} style="padding:4px 10px 4px 8px"><Icon name="shield" size={13} stroke={2} color="#6cc59a" />Tandem standard</Chip>
                {#if m.owner}
                  <Chip dot={false} style="padding:4px 10px 4px 8px"
                    ><Icon name="plug" size={13} stroke={2} color={colorOf(m.owner)} />{ownerText(m)}</Chip
                  >
                {/if}
                {#if !m.connected}
                  <Chip dot={false} class="mut" style="padding:4px 10px 4px 8px"><Icon name="unplugged" size={13} stroke={2} />Disconnected</Chip>
                {/if}
                {#if isDim(m.link) && state}
                  <Chip dot={false} class="link-state" style="padding:4px 10px 4px 8px"
                    ><Icon name={m.link?.kind === "unplugged" ? "unplugged" : "power"} size={13} stroke={2} color={m.link?.kind === "unplugged" ? colorOf(m.link.machine) : "#d9a75a"} />{state}</Chip
                  >
                {/if}
              </span>
            </div>
          </div>
          <Segmented size="lg" label="Which machine shows the {spoken(m.name)}" options={options(m)} value={selected(m)} onchange={(v) => pick(m, v)} />
          {#if m.owner && m.link?.kind !== "off" && m.link?.kind !== "unplugged"}
            {@const asleep = m.link?.kind === "standby"}
            <div class="power">
              <Button icon="power" size="sm" iconSize={15} variant={asleep ? undefined : "ghost"} onclick={() => store.monitorPower(m.id, asleep)}
                >{asleep ? "Wake" : "Standby"}</Button
              >
            </div>
          {/if}
          {#if m.events.length > 0}
            <div class="events">
              {#each m.events as e, k (`${e.at}-${e.text}-${k}`)}
                {@const ic = EV_ICON[e.kind]}
                <div class="ev" style="animation-delay:{0.1 + k * 0.08}s">
                  <span class="evi"><Icon name={ic.icon} size={12} stroke={ic.stroke} color={ic.color} /></span><span class="mono mut">{formatClock(e.at)}</span><span>{e.text}</span>
                </div>
              {/each}
            </div>
          {/if}
        </Card>
      {/each}

      {#each others as m, i (m.id)}
        <Card tag="article" delay={0.08 + i * 0.08} class="mon off" style="--card-pad:18px" label="{m.name}, unsupported">
          <div class="head">
            <Thumb width={110} height={58} wall="repeating-linear-gradient(45deg,#17181c 0 8px,#121316 8px 16px)" dim />
            <div class="who grow">
              <b>{m.name}</b>
              <span class="chips">
                <Chip dot={false} tone="warn" style="padding:4px 10px 4px 8px"><Icon name="warn" size={13} stroke={2} />Unsupported</Chip>
                {#if !m.connected}<Chip dot={false} class="mut" style="padding:4px 10px 4px 8px"><Icon name="unplugged" size={13} stroke={2} />Disconnected</Chip>{/if}
              </span>
            </div>
            <!-- Without an agent command to forget a monitor, the action is not offered (DESIGN §3.8). -->
            {#if store.can("forget")}
              <Button icon="trash" variant="ghost" size="sm" iconSize={16} iconStroke={1.8} iconOnly label="Forget this monitor" disabled={m.connected} onclick={() => store.forgetMonitor(m.id)} />
            {/if}
          </div>
          <p class="why">{m.compat.ok ? "" : m.compat.reason}</p>
          <Button icon="info" size="sm" iconSize={16} class="explain" pressed={explained[m.id] ?? false} onclick={() => (explained[m.id] = !explained[m.id])}>What the standard is</Button>
          {#if explained[m.id]}
            <p class="standard">Tandem controls a monitor only if it speaks the custom firmware protocol (the 34"’s). The others are shown untouched until they get their patch.</p>
          {/if}
        </Card>
      {/each}
    </div>
  {/if}

  <Card tag="section" delay={0.12} label="Scenes" class="scenes" style="--card-pad:18px">
    <div class="sh">
      <h2 class="h3">Scenes</h2>
      <span class="fill"></span>
      <Button icon="plus" variant="pri" size="sm" iconSize={16} iconStroke={2.2} iconLinecap="round" href={newHref} style="--btn-px:11px">New scene</Button>
    </div>
    <div class="sgrid">
      {#each desk.scenes as s, i (s.id)}
        {@const on = desk.activeScene === s.id}
        <article class="sc" class:act={on} style="animation-delay:{0.15 + i * 0.06}s" aria-label="Scene {s.name}">
          <div class="st">
            <Thumb width={72} height={36} pad={3} wall={wall(s.shows)} />
            <div class="sn"><b>{s.name}</b><span class="mut">{s.desc}</span></div>
          </div>
          <div class="sb">
            {#if s.key}<Kbd raised>{s.key}</Kbd>{/if}
            <span class="fill"></span>
            {#if on}
              <span class="badge"><span class="d"></span>Active</span>
            {:else}
              <Button icon="pencil" variant="ghost" size="sm" iconSize={16} iconStroke={1.8} iconOnly label="Edit scene" href={editHref(s.id)} />
              <Button icon="play" size="sm" iconSize={16} disabled={!store.can("scenes")} onclick={() => apply(s.id)}>Apply</Button>
            {/if}
          </div>
        </article>
      {/each}
    </div>
  </Card>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 16px;
    flex: 1;
    --btn-lh: normal;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(400px, 1fr));
    gap: 14px;
  }
  .grid :global(.card.mon) {
    display: flex;
    flex-direction: column;
    gap: 16px;
    overflow: visible;
  }
  .grid :global(.card.mon.off) {
    gap: 14px;
    border-style: dashed;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .who {
    display: flex;
    flex-direction: column;
    gap: 7px;
    min-width: 0;
  }
  .who.grow {
    flex: 1;
  }
  .who b {
    font-weight: 600;
    font-size: 15px;
  }
  .chips {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .power {
    display: flex;
    justify-content: flex-end;
    margin-top: 10px;
  }
  .events {
    display: flex;
    flex-direction: column;
    border-top: 1px solid #1f2126;
    padding-top: 10px;
  }
  .ev {
    display: grid;
    grid-template-columns: 22px 48px minmax(0, 1fr);
    gap: 8px;
    align-items: center;
    font-size: 12.5px;
    padding: 6px 0;
    animation: mon-up 0.4s ease-out both;
  }
  .evi {
    width: 22px;
    height: 22px;
    border-radius: 7px;
    display: grid;
    place-items: center;
    background: #1b1c21;
    border: 1px solid #2a2c33;
  }
  .why {
    margin: 0;
    font-size: 13px;
    line-height: 1.55;
    color: #c4c6cc;
  }
  .standard {
    margin: 0;
    font-size: 12.5px;
    line-height: 1.55;
    color: #9a9da6;
    animation: mon-in 0.45s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .grid :global(.explain) {
    align-self: flex-start;
  }
  .page :global(.card.scenes) {
    overflow: visible;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .sh {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .fill {
    flex: 1;
  }
  .sgrid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 12px;
  }
  .sc {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 14px;
    border-radius: 14px;
    background: #111215;
    border: 1px solid #25272d;
    transition:
      border-color 0.25s,
      transform 0.25s,
      box-shadow 0.25s;
    animation: mon-up 0.45s cubic-bezier(0.2, 0.8, 0.2, 1) both;
  }
  .sc:hover {
    transform: translateY(-3px);
    border-color: #3a3d45;
    box-shadow: 0 12px 26px rgba(0, 0, 0, 0.35);
  }
  .sc.act {
    border-color: #4f7cbc;
    background: linear-gradient(180deg, #141c28, #111215);
    box-shadow:
      0 0 0 1px rgba(122, 167, 230, 0.2),
      0 0 26px rgba(122, 167, 230, 0.12);
  }
  .st,
  .sb {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .sb {
    gap: 8px;
  }
  .sn {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
    flex: 1;
  }
  .sn b {
    font-weight: 600;
    font-size: 14px;
  }
  .sn span {
    font-size: 12px;
  }
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 10px;
    border-radius: 999px;
    font-size: 12px;
    color: #c3d8f6;
    background: rgba(122, 167, 230, 0.1);
    border: 1px solid rgba(122, 167, 230, 0.35);
  }
  .badge .d {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #7aa7e6;
    animation: mon-pl 1.8s ease-in-out infinite;
  }
  @keyframes mon-up {
    from {
      opacity: 0;
      transform: translateY(10px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  @keyframes mon-in {
    from {
      opacity: 0;
      transform: translateY(6px) scale(0.98);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  @keyframes mon-pl {
    0%,
    100% {
      box-shadow: 0 0 0 0 rgba(122, 167, 230, 0.55);
    }
    50% {
      box-shadow: 0 0 0 5px rgba(122, 167, 230, 0);
    }
  }
</style>
