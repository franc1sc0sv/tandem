//! T-346 (DESIGN §10): the machine with a DDC channel that is not the owner, with two real
//! agents on one shared monitor bus.
//!
//! The 34"'s DP and HDMI share one DDC/CI request buffer (monitor-firmware,
//! `inactive-hdmi-ddc.md`): two hosts talking at once can get each other's replies. So:
//!
//! - While the owner is reachable, the other machine sends nothing to the monitor beyond the
//!   probe it always sent (it shows the owner's report).
//! - While the owner is unreachable (here the PC, which loses power) but not yet given up for
//!   lost, the Mac reads the live state itself (`0xEE`, `0x60`, never an event register) and can
//!   switch the input with the firmware's verdict.
//! - When the PC is given up for lost, the monitor is the Mac's within one poll and its reading
//!   carries on from the PC's last reported cursor: nothing that happened meanwhile is lost.
//! - A reply meant for the other host is dropped, never shown.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use monitor_ddc::fake::{BusOp, FakeMonitorConfig, Personality};
use monitor_ddc::{Input, VCP_INPUT, VCP_LIVE};
use tandem_core::{Machine, MonitorInput};
use tandem_link::manager::LinkKind;
use tandem_proto::{MonitorEventKind, MonitorView};
use tandem_sim::desk::SIM_MONITOR_TIMES;
use tandem_sim::{Desk, MonitorSpec, Outcome, SimMonitors};

const STD: usize = 0;

/// The probe and the live state: everything else is the owner's (events and their cursor).
const EVENT_REGISTERS: [u8; 4] = [0xEB, 0xEC, 0xED, 0xEF];

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// The 34" seen by both, the PC faster (the owner) and the Mac also with a channel.
async fn desk() -> (Desk, SimMonitors, String) {
    let d = Desk::builder()
        .monitors(vec![MonitorSpec {
            config: FakeMonitorConfig::new(Personality::Standard),
            cables: vec![(Machine::A, ms(30)), (Machine::B, ms(5))],
            via: vec![],
        }])
        .build()
        .await
        .unwrap();
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    d.pc_agrees().await.unwrap();
    let sim = d.monitors().unwrap().clone();
    let id = sim.id(STD).to_owned();
    for m in Machine::ALL {
        d.wait_monitors(m, "the PC as owner", owner_is(&id, Machine::B))
            .await
            .unwrap();
    }
    d.wait_monitors(
        Machine::A,
        "the PC's report",
        input_is(&id, MonitorInput::DisplayPort),
    )
    .await
    .unwrap();
    (d, sim, id)
}

fn view<'a>(views: &'a [MonitorView], id: &str) -> Option<&'a MonitorView> {
    views.iter().find(|v| v.id == id)
}

fn owner_is(id: &str, owner: Machine) -> impl Fn(&[MonitorView]) -> bool + '_ {
    move |v| view(v, id).is_some_and(|m| m.owner == Some(owner))
}

fn input_is(id: &str, input: MonitorInput) -> impl Fn(&[MonitorView]) -> bool + '_ {
    move |v| view(v, id).is_some_and(|m| m.report.as_ref().and_then(|r| r.input) == Some(input))
}

/// What `machine` sent to the 34" since the last `clear_ops`.
fn ops_of(sim: &SimMonitors, machine: Machine) -> Vec<BusOp> {
    sim.ops_for(sim.handle(STD))
        .into_iter()
        .filter(|(m, _)| *m == machine)
        .map(|(_, op)| op)
        .collect()
}

fn gets(ops: &[BusOp]) -> Vec<u8> {
    ops.iter()
        .filter_map(|op| match op {
            BusOp::Get { code, .. } => Some(*code),
            _ => None,
        })
        .collect()
}

fn no_events_nor_sets(ops: &[BusOp]) -> bool {
    ops.iter().all(|op| match op {
        BusOp::Get { code, .. } => !EVENT_REGISTERS.contains(code),
        _ => false,
    })
}

#[tokio::test(start_paused = true)]
async fn while_the_owner_is_reachable_the_other_only_probes() {
    let (d, sim, _) = desk().await;
    sim.clear_ops();
    sim.fake().osd_switch(sim.handle(STD), Input::Hdmi);
    tokio::time::sleep(Duration::from_secs(6)).await;
    let mac = ops_of(&sim, Machine::A);
    assert!(
        mac.iter()
            .all(|op| matches!(op, BusOp::Get { code: 0xEA, .. })),
        "the Mac sends nothing but its probe: {mac:?}"
    );
    assert!(gets(&ops_of(&sim, Machine::B)).contains(&VCP_LIVE));
    // It learns the new input from the PC's report.
    d.wait_monitors(
        Machine::A,
        "the PC's new report",
        input_is(sim.id(STD), MonitorInput::Hdmi),
    )
    .await
    .unwrap();
}

#[tokio::test(start_paused = true)]
async fn with_the_pc_off_the_mac_reads_and_switches_and_then_carries_on_its_cursor() {
    let (d, sim, id) = desk().await;
    let lost_after = d.mac.config().power.peer_lost_after();
    let started = tokio::time::Instant::now();
    sim.clear_ops();
    d.pull_plug(Machine::B);

    // Within the threshold the PC is still the owner, but the Mac reads the real input itself.
    sim.fake().osd_switch(sim.handle(STD), Input::Hdmi);
    let views = d
        .wait_monitors(
            Machine::A,
            "the Mac's own reading",
            input_is(&id, MonitorInput::Hdmi),
        )
        .await
        .unwrap();
    assert_eq!(view(&views, &id).unwrap().owner, Some(Machine::B));
    let mac = ops_of(&sim, Machine::A);
    assert!(no_events_nor_sets(&mac), "live state only: {mac:?}");
    assert!(gets(&mac).contains(&VCP_LIVE) && gets(&mac).contains(&VCP_INPUT));

    // And it switches the 34" itself, with the firmware's verdict.
    let run = d
        .switch_monitor(Machine::A, &id, MonitorInput::DisplayPort)
        .await
        .unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert_eq!(
        sim.fake().screen_input(sim.handle(STD)),
        Some(Input::DisplayPort)
    );
    assert!(
        sim.sets_to(sim.handle(STD))
            .contains(&(Machine::A, VCP_INPUT, Input::DisplayPort.vcp())),
        "{:?}",
        sim.sets_to(sim.handle(STD))
    );
    let views = d.agent(Machine::A).monitors();
    assert!(
        input_is(&id, MonitorInput::DisplayPort)(&views),
        "{views:?}"
    );
    assert!(
        d.mac.state().is_on(Machine::B),
        "all of this before the PC is given up for lost ({:?} of {lost_after:?})",
        started.elapsed()
    );

    // The PC is given up for lost: within one poll the monitor is the Mac's, and it reads what
    // happened since the PC's last report (the menu switch and its own switch), not afresh.
    let mut states = d.mac.subscribe();
    states
        .wait_for(|s| !s.is_on(Machine::B))
        .await
        .expect("the agent closed");
    let lost = tokio::time::Instant::now();
    let views = d
        .wait_monitors(Machine::A, "the Mac as owner with the events", |v| {
            owner_is(&id, Machine::A)(v)
                && view(v, &id)
                    .and_then(|m| m.report.as_ref())
                    .is_some_and(|r| !r.events.is_empty())
        })
        .await
        .unwrap();
    assert!(
        lost.elapsed() <= SIM_MONITOR_TIMES.poll_every,
        "{:?}",
        lost.elapsed()
    );
    let events = &view(&views, &id).unwrap().report.as_ref().unwrap().events;
    let seen = |kind, input| events.iter().any(|e| e.kind == kind && e.input == input);
    assert!(
        seen(MonitorEventKind::Input, Some(MonitorInput::Hdmi)),
        "the menu switch while nobody owned it: {events:?}"
    );
    assert!(
        seen(
            MonitorEventKind::SwitchDone,
            Some(MonitorInput::DisplayPort)
        ),
        "the Mac's own switch: {events:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_reply_meant_for_the_other_host_is_never_shown() {
    let (d, sim, id) = desk().await;
    // The links go down but the PC stays on: it keeps reading the 34" as its owner, so the two
    // hosts' requests can cross in the monitor's shared buffer.
    for kind in LinkKind::ALL {
        d.unplug_cable(kind);
    }
    let mut connected = d.mac.orchestrator().peer_connected();
    connected.wait_for(|c| !*c).await.expect("the agent closed");
    sim.fake().osd_switch(sim.handle(STD), Input::Hdmi);
    d.wait_monitors(
        Machine::A,
        "the Mac's own reading",
        input_is(&id, MonitorInput::Hdmi),
    )
    .await
    .unwrap();

    // From now on every reply the Mac gets is the one to the PC's ring request (`0xEB`, which the
    // PC reads on every poll): as `0xEE` it would say "DP", with the power off.
    sim.clear_ops();
    sim.cross_replies(Machine::A, u32::MAX, Some(0xEB));
    let mut views = d.agent(Machine::A).subscribe_monitors();
    let shown = Arc::new(Mutex::new(Vec::new()));
    let watch = tokio::spawn({
        let (shown, id) = (shown.clone(), id.clone());
        async move {
            while views.changed().await.is_ok() {
                let r = view(&views.borrow_and_update(), &id).and_then(|m| m.report.clone());
                if let Some(r) = r {
                    shown.lock().unwrap().push((r.input, r.power_on, r.signal));
                }
            }
        }
    });
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(sim.crossed_left(Machine::A) < u32::MAX, "replies did cross");
    assert!(gets(&ops_of(&sim, Machine::A)).contains(&VCP_INPUT));
    sim.cross_replies(Machine::A, 0, None);
    let views = d.agent(Machine::A).monitors();
    let r = view(&views, &id).unwrap().report.clone().unwrap();
    assert_eq!(
        (r.input, r.power_on, r.signal),
        (Some(MonitorInput::Hdmi), Some(true), Some(true)),
        "nothing crossed got through"
    );
    watch.abort();
    let shown = shown.lock().unwrap().clone();
    assert!(
        shown
            .iter()
            .all(|s| *s == (Some(MonitorInput::Hdmi), Some(true), Some(true))),
        "{shown:?}"
    );

    // The links come back: the Mac stops reading and shows the PC's report again.
    for kind in LinkKind::ALL {
        d.plug_cable(kind);
    }
    connected.wait_for(|c| *c).await.expect("the agent closed");
    d.settle().await;
    sim.clear_ops();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let mac = ops_of(&sim, Machine::A);
    assert!(
        mac.iter()
            .all(|op| matches!(op, BusOp::Get { code: 0xEA, .. })),
        "{mac:?}"
    );
    let views = d.agent(Machine::A).monitors();
    assert_eq!(view(&views, &id).unwrap().owner, Some(Machine::B));
    assert!(input_is(&id, MonitorInput::Hdmi)(&views), "{views:?}");
}
