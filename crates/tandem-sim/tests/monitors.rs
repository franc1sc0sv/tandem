//! Monitor scenarios (T-110, DESIGN §10): two real agents, a standard monitor
//! (own firmware) and a factory one, with the `monitor-ddc` doubles.
//!
//! - Owner election: the same on both machines, the one with the lower latency.
//! - Input switch from the UI of the one that is not the owner: the owner does it, with a verdict.
//! - Re-election when the owner shuts down, with a new event cursor.
//! - Rollback if the switch is left without a verdict.
//! - The factory monitor never receives a Set.

use std::time::Duration;

use monitor_ddc::fake::{FakeMonitorConfig, Personality, SwitchBehavior, test_edid};
use monitor_ddc::{Input, VCP_INPUT};
use tandem_core::{Machine, MonitorInput};
use tandem_proto::{MonitorEventKind, MonitorView};
use tandem_sim::{Desk, MonitorSpec, Outcome, SimError};

const STD: usize = 0;
const FACTORY: usize = 1;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// The 34" with the own firmware (the PC faster over DP) and a factory one that only the PC sees.
fn monitors(standard: FakeMonitorConfig) -> Vec<MonitorSpec> {
    let mut factory = FakeMonitorConfig::new(Personality::Factory);
    factory.edid = test_edid("SAM", 0x7154, "Odyssey G5", "FAKE0000002");
    vec![
        MonitorSpec {
            config: standard,
            cables: vec![(Machine::A, ms(30)), (Machine::B, ms(5))],
            via: vec![],
        },
        MonitorSpec {
            config: factory,
            cables: vec![(Machine::B, ms(5))],
            via: vec![],
        },
    ]
}

async fn desk_with(standard: FakeMonitorConfig) -> Desk {
    let d = Desk::builder()
        .monitors(monitors(standard))
        .build()
        .await
        .unwrap();
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    d.pc_agrees().await.unwrap();
    // While the PC was off the monitor was the Mac's; with both, the PC's.
    let std = d.monitors().unwrap().id(STD).to_owned();
    for m in Machine::ALL {
        d.wait_monitors(m, "the elected owner", owner_is(&std, Machine::B))
            .await
            .unwrap();
    }
    d
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

/// The Sets to monitor `i` and who sent them.
fn sets(d: &Desk, i: usize) -> Vec<(Machine, u8, u16)> {
    let sim = d.monitors().unwrap();
    sim.sets_to(sim.handle(i))
}

#[tokio::test(start_paused = true)]
async fn the_owner_is_elected_the_same_on_both_and_switches_with_its_verdict() {
    let d = desk_with(FakeMonitorConfig::new(Personality::Standard)).await;
    let sim = d.monitors().unwrap().clone();
    let (std, fab) = (sim.id(STD).to_owned(), sim.id(FACTORY).to_owned());

    // Both reach the same owner: the PC (B), which answers in 5 ms against 30 ms.
    for m in Machine::ALL {
        let views = d
            .wait_monitors(m, "the elected owner", owner_is(&std, Machine::B))
            .await
            .unwrap();
        let s = view(&views, &std).unwrap();
        assert!(s.compat.is_compatible());
        assert_eq!(s.seen_by, [Machine::A, Machine::B]);
        let f = view(&views, &fab).unwrap();
        assert!(!f.compat.is_compatible(), "{f:?}");
        assert_eq!((f.owner, f.seen_by.as_slice()), (None, &[Machine::B][..]));
    }
    // The Mac sees what the PC reads: the real input.
    d.wait_monitors(
        Machine::A,
        "the input read by the PC",
        input_is(&std, MonitorInput::DisplayPort),
    )
    .await
    .unwrap();

    // From the Mac's UI: the PC does the switch, with the firmware's verdict.
    sim.clear_ops();
    let run = d
        .switch_monitor(Machine::A, &std, MonitorInput::Hdmi)
        .await
        .unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert_eq!(sim.fake().screen_input(sim.handle(STD)), Some(Input::Hdmi));
    let input_sets: Vec<_> = sets(&d, STD)
        .into_iter()
        .filter(|(_, code, _)| *code == VCP_INPUT)
        .collect();
    assert_eq!(input_sets, [(Machine::B, VCP_INPUT, Input::Hdmi.vcp())]);
    assert!(
        sets(&d, STD).iter().all(|(m, _, _)| *m == Machine::B),
        "only the owner writes: {:?}",
        sets(&d, STD)
    );

    // Both UIs see the new input and the verdict among the events.
    for m in Machine::ALL {
        let views = d
            .wait_monitors(m, "the new input", input_is(&std, MonitorInput::Hdmi))
            .await
            .unwrap();
        let events = &view(&views, &std).unwrap().report.as_ref().unwrap().events;
        assert!(
            events
                .iter()
                .any(|e| e.kind == MonitorEventKind::SwitchDone
                    && e.input == Some(MonitorInput::Hdmi)),
            "{events:?}"
        );
    }

    // The factory one is shown, but not controlled: the request is rejected without touching it.
    let e = d
        .switch_monitor(Machine::A, &fab, MonitorInput::Hdmi)
        .await
        .unwrap_err();
    assert!(
        matches!(&e, SimError::Rejected(why) if why.contains("no es compatible")),
        "{e}"
    );
    let e = d
        .switch_monitor(Machine::B, &fab, MonitorInput::Hdmi)
        .await
        .unwrap_err();
    assert!(matches!(e, SimError::Rejected(_)), "{e}");
    d.settle().await;
    assert!(sets(&d, FACTORY).is_empty(), "{:?}", sets(&d, FACTORY));
}

#[tokio::test(start_paused = true)]
async fn when_the_owner_powers_off_the_other_takes_over_with_a_fresh_cursor() {
    let d = desk_with(FakeMonitorConfig::new(Personality::Standard)).await;
    let sim = d.monitors().unwrap().clone();
    let std = sim.id(STD).to_owned();
    d.wait_monitors(Machine::A, "the PC as owner", owner_is(&std, Machine::B))
        .await
        .unwrap();

    // While the PC reads, the user switches twice with the monitor menu. The Mac was the
    // owner before the PC started: if on becoming one again it used that cursor, it would report
    // these two as well.
    sim.fake().osd_switch(sim.handle(STD), Input::Hdmi);
    sim.fake().osd_switch(sim.handle(STD), Input::DisplayPort);
    d.wait_monitors(Machine::A, "the switches read by the PC", |v| {
        view(v, &std)
            .and_then(|m| m.report.as_ref())
            .is_some_and(|r| {
                r.events
                    .iter()
                    .filter(|e| e.kind == MonitorEventKind::Input)
                    .count()
                    == 2
            })
    })
    .await
    .unwrap();

    // The PC loses power: the Mac considers it off and the monitor goes to it.
    d.pull_plug(Machine::B);
    let views = d
        .wait_monitors(Machine::A, "the Mac as owner", owner_is(&std, Machine::A))
        .await
        .unwrap();
    assert_eq!(view(&views, &std).unwrap().seen_by, [Machine::A]);

    // A new switch with the menu: the Mac, now the owner, reports that one and nothing from before.
    sim.fake().osd_switch(sim.handle(STD), Input::Hdmi);
    let views = d
        .wait_monitors(
            Machine::A,
            "the menu switch",
            input_is(&std, MonitorInput::Hdmi),
        )
        .await
        .unwrap();
    let report = view(&views, &std).unwrap().report.as_ref().unwrap();
    let inputs: Vec<_> = report
        .events
        .iter()
        .filter(|e| e.kind == MonitorEventKind::Input)
        .map(|e| e.input)
        .collect();
    assert_eq!(
        inputs,
        [Some(MonitorInput::Hdmi)],
        "new cursor: only what came after taking the monitor: {:?}",
        report.events
    );

    // And the switch from the Mac is done by the Mac.
    sim.clear_ops();
    let run = d
        .switch_monitor(Machine::A, &std, MonitorInput::DisplayPort)
        .await
        .unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert_eq!(
        sim.fake().screen_input(sim.handle(STD)),
        Some(Input::DisplayPort)
    );
    assert!(
        sets(&d, STD)
            .iter()
            .any(|s| *s == (Machine::A, VCP_INPUT, Input::DisplayPort.vcp())),
        "{:?}",
        sets(&d, STD)
    );
    assert!(sets(&d, FACTORY).is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_switch_without_verdict_goes_back() {
    let mut silent = FakeMonitorConfig::new(Personality::Standard);
    silent.switch = SwitchBehavior::Silent;
    let d = desk_with(silent).await;
    let sim = d.monitors().unwrap().clone();
    let std = sim.id(STD).to_owned();
    d.wait_monitors(
        Machine::A,
        "the input read",
        input_is(&std, MonitorInput::DisplayPort),
    )
    .await
    .unwrap();

    let run = d
        .switch_monitor(Machine::A, &std, MonitorInput::Hdmi)
        .await
        .unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    let (_, why) = run.failure().unwrap();
    assert!(why.contains(tandem_monitor::NO_VERDICT), "{why}");
    // Rollback: the owner put DP back.
    assert_eq!(
        sim.fake().screen_input(sim.handle(STD)),
        Some(Input::DisplayPort)
    );
    let input_sets: Vec<_> = sets(&d, STD)
        .into_iter()
        .filter(|(_, code, _)| *code == VCP_INPUT)
        .collect();
    assert_eq!(
        input_sets,
        [
            (Machine::B, VCP_INPUT, Input::Hdmi.vcp()),
            (Machine::B, VCP_INPUT, Input::DisplayPort.vcp()),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_failed_verdict_leaves_the_monitor_where_it_was() {
    let mut failing = FakeMonitorConfig::new(Personality::Standard);
    failing.switch = SwitchBehavior::Fail;
    let d = desk_with(failing).await;
    let sim = d.monitors().unwrap().clone();
    let std = sim.id(STD).to_owned();
    d.wait_monitors(
        Machine::B,
        "the input read",
        input_is(&std, MonitorInput::DisplayPort),
    )
    .await
    .unwrap();
    let run = d
        .switch_monitor(Machine::B, &std, MonitorInput::Hdmi)
        .await
        .unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed));
    assert_eq!(
        run.failure().map(|(_, w)| w.to_owned()),
        Some("the monitor showed no image on HDMI; it stays on DP".into())
    );
    // The firmware already stayed where it was: no need to return.
    assert_eq!(
        sets(&d, STD)
            .iter()
            .filter(|(_, code, _)| *code == VCP_INPUT)
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn without_monitors_the_module_says_why() {
    let d = Desk::new().await.unwrap();
    let caps = d.agent(Machine::A).capabilities().here;
    let m = caps
        .modules
        .iter()
        .find(|m| m.id == tandem_proto::ModuleId::Monitors)
        .unwrap();
    assert_eq!(
        m.state,
        tandem_proto::ModuleState::Unavailable {
            reason: tandemd::monitors::NO_DDC.into()
        }
    );
    assert!(d.agent(Machine::A).monitors().is_empty());
}
