//! Scenes with real monitors (T-111, D6, D13): two real agents with the `monitor-ddc` doubles.
//! The scene step "the 34" shows B" is an input switch done by the
//! owner (the PC, faster), with the firmware's verdict; the geometry takes the screen off the
//! machine that stops being seen; a failed verdict rolls back the monitors already switched; the
//! active scene comes from the real input; the editor only accepts compatible monitors.

use std::time::Duration;

use monitor_ddc::fake::{FakeMonitorConfig, Personality, SwitchBehavior, test_edid};
use monitor_ddc::{Input, VCP_INPUT};
use tandem_core::{DeskPatch, Machine, MonitorInput, Scale, Scene, ScreenSpec, Step};
use tandem_proto::{Reply, TxnEvent, UiCommand};
use tandem_sim::{Desk, MonitorSpec, Outcome, TxnRun};
use tandemd::input::screens;

const LIMIT: Duration = Duration::from_secs(600);
const M34: usize = 0;
const M27: usize = 1;
const FAB: usize = 2;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// The 34" and a 27" with the own firmware (the PC over DP, faster; the Mac over HDMI, without an
/// interface type in its EDID) and a factory one that only the PC sees. For the 27" the PC's EDID
/// says the input only if `known27`.
fn specs(switch27: SwitchBehavior, known27: bool) -> Vec<MonitorSpec> {
    let both = vec![(Machine::A, ms(30)), (Machine::B, ms(5))];
    let mut m27 = FakeMonitorConfig::new(Personality::Standard);
    m27.edid = test_edid("SAM", 0x7154, "Odyssey G5", "FAKE0000002");
    m27.switch = switch27;
    let mut fab = FakeMonitorConfig::new(Personality::Factory);
    fab.edid = test_edid("DEL", 0x4321, "U2720Q", "FAKE0000003");
    vec![
        MonitorSpec {
            config: FakeMonitorConfig::new(Personality::Standard),
            cables: both.clone(),
            via: vec![(Machine::B, MonitorInput::DisplayPort)],
        },
        MonitorSpec {
            config: m27,
            cables: both,
            via: if known27 {
                vec![(Machine::B, MonitorInput::DisplayPort)]
            } else {
                vec![]
            },
        },
        MonitorSpec {
            config: fab,
            cables: vec![(Machine::B, ms(5))],
            via: vec![],
        },
    ]
}

fn id(d: &Desk, i: usize) -> String {
    d.monitors().unwrap().id(i).to_owned()
}

async fn desk_until(d: &Desk, m: Machine, what: &str, pred: impl Fn(&tandem_core::Desk) -> bool) {
    let mut desk = d.agent(m).subscribe_desk();
    tokio::time::timeout(LIMIT, desk.wait_for(|d| pred(d)))
        .await
        .unwrap_or_else(|_| panic!("{m}: {what}"))
        .unwrap();
}

async fn scene_until(d: &Desk, m: Machine, scene: Option<&str>) {
    let mut state = d.agent(m).subscribe();
    tokio::time::timeout(LIMIT, state.wait_for(|s| s.scene.as_deref() == scene))
        .await
        .unwrap_or_else(|_| panic!("{m}: the active scene did not reach {scene:?}"))
        .unwrap();
}

/// Both on and paired; the PC (owner) presented the compatible monitors whose
/// inputs are known, and both recognize "Work" by the real input (all on DP, the PC).
async fn paired(switch27: SwitchBehavior, known27: bool) -> Desk {
    let d = Desk::builder()
        .monitors(specs(switch27, known27))
        .build()
        .await
        .unwrap();
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    let (m34, m27) = (id(&d, M34), id(&d, M27));
    for m in Machine::ALL {
        desk_until(&d, m, "the monitors presented", |p| {
            p.cable(&m34).is_some() && p.cable(&m27).is_some() == known27 && p.scenes.len() == 3
        })
        .await;
    }
    let desk = d.mac.desk();
    let cable = desk.cable(&m34).unwrap();
    assert_eq!(
        (cable.a.input, cable.b.input),
        (Some(MonitorInput::Hdmi), Some(MonitorInput::DisplayPort)),
        "the Mac goes to the other input"
    );
    assert!(
        desk.cable(&id(&d, FAB)).is_none(),
        "the factory one does not come in"
    );
    let n = if known27 { 2 } else { 1 };
    let scene = |sid: &str| desk.scenes.iter().find(|s| s.id == sid).unwrap().clone();
    assert_eq!(scene("only-a").monitors.len(), n);
    assert!(
        scene("only-a")
            .monitors
            .iter()
            .all(|(_, m)| *m == Machine::A)
    );
    assert!(
        scene("only-b")
            .monitors
            .iter()
            .all(|(_, m)| *m == Machine::B)
    );
    assert_eq!(scene("work").monitors[0], (m34.clone(), Machine::B));
    for m in Machine::ALL {
        scene_until(&d, m, Some("work")).await;
    }
    d
}

/// Declared screens: the Mac with its own and the 34"; the PC only with the 34". The wiring says
/// which screen of each one is the 34".
fn declare_screens(d: &Desk) {
    let screen = |id: &str, x: i32, width: u32, height: u32| ScreenSpec {
        machine: Machine::A,
        id: id.into(),
        x,
        y: 0,
        width,
        height,
        scale: Scale::default(),
    };
    for (machine, screens) in [
        (
            Machine::A,
            vec![
                screen("mac", 0, 1512, 982),
                screen("mac-34", 1512, 3440, 1440),
            ],
        ),
        (Machine::B, vec![screen("pc-34", 0, 3440, 1440)]),
    ] {
        d.mac
            .edit_desk(DeskPatch::Screens { machine, screens })
            .unwrap();
    }
    let mut cable = d.mac.desk().cable(&id(d, M34)).unwrap().clone();
    cable.a.screen = Some("mac-34".into());
    cable.b.screen = Some("pc-34".into());
    d.mac.edit_desk(DeskPatch::Cable(cable)).unwrap();
}

/// A's and B's screens in `on`'s geometry, with what each monitor shows according to
/// what it read (the same as its input engine computes).
fn geometry(d: &Desk, on: Machine) -> (Vec<String>, Vec<String>) {
    let agent = d.agent(on);
    let desk = agent.desk();
    let shows = tandemd::monitors::shows(&agent.monitors(), &desk);
    let arr = screens::arrangement(&desk, &agent.state(), on, &[], &[], &shows);
    (
        arr.a.into_iter().map(|s| s.id.0).collect(),
        arr.b.into_iter().map(|s| s.id.0).collect(),
    )
}

async fn geometry_until(d: &Desk, on: Machine, a: &[&str], b: &[&str]) {
    let want = (
        a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
        b.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
    );
    // It changes with the desk, the state and the monitors: it is looked at every so often (paused clock).
    tokio::time::timeout(LIMIT, async {
        while geometry(d, on) != want {
            tokio::time::sleep(ms(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "{on}: geometry {:?}, {want:?} was expected",
            geometry(d, on)
        )
    });
}

fn steps(run: &TxnRun) -> Vec<Step> {
    match run.events.first() {
        Some(TxnEvent::Started { steps, .. }) => steps.clone(),
        other => panic!("no start: {other:?}"),
    }
}

fn undone(run: &TxnRun) -> Vec<u32> {
    run.events
        .iter()
        .filter_map(|e| match e {
            TxnEvent::StepUndone { index, .. } => Some(*index),
            _ => None,
        })
        .collect()
}

fn show(monitor: &str, machine: Machine) -> Step {
    Step::ShowMachine {
        monitor: monitor.to_owned(),
        machine,
    }
}

/// The `Set 0x60`s to monitor `i`: who and to which input.
fn input_sets(d: &Desk, i: usize) -> Vec<(Machine, u16)> {
    let sim = d.monitors().unwrap();
    sim.sets_to(sim.handle(i))
        .into_iter()
        .filter(|(_, code, _)| *code == VCP_INPUT)
        .map(|(m, _, v)| (m, v))
        .collect()
}

fn on_screen(d: &Desk, i: usize) -> Option<Input> {
    let sim = d.monitors().unwrap();
    sim.fake().screen_input(sim.handle(i))
}

/// Apply "Only A" and then "Only B" from the Mac: each monitor is switched by the PC (its owner)
/// with the firmware's verdict, and both machines' geometry takes the 34" off the one
/// that stops being seen on it.
#[tokio::test(start_paused = true)]
async fn only_b_switches_the_monitors_and_the_geometry() {
    let d = paired(SwitchBehavior::Done, true).await;
    let (m34, m27) = (id(&d, M34), id(&d, M27));
    declare_screens(&d);
    for m in Machine::ALL {
        geometry_until(&d, m, &["mac"], &["pc-34"]).await;
    }
    d.monitors().unwrap().clear_ops();

    let run = d.apply_scene(Machine::A, "only-a").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert_eq!(
        steps(&run),
        [
            show(&m34, Machine::A),
            show(&m27, Machine::A),
            Step::Geometry
        ]
    );
    assert_eq!(on_screen(&d, M34), Some(Input::Hdmi));
    assert_eq!(on_screen(&d, M27), Some(Input::Hdmi));
    assert_eq!(input_sets(&d, M34), [(Machine::B, Input::Hdmi.vcp())]);
    assert_eq!(
        geometry(&d, Machine::A),
        (vec!["mac".into(), "mac-34".into()], vec![])
    );
    geometry_until(&d, Machine::B, &["mac", "mac-34"], &[]).await;
    scene_until(&d, Machine::B, Some("only-a")).await;

    let run = d.apply_scene(Machine::A, "only-b").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert_eq!(
        steps(&run),
        [
            show(&m34, Machine::B),
            show(&m27, Machine::B),
            Step::Geometry
        ]
    );
    assert_eq!(on_screen(&d, M34), Some(Input::DisplayPort));
    assert_eq!(
        input_sets(&d, M34),
        [
            (Machine::B, Input::Hdmi.vcp()),
            (Machine::B, Input::DisplayPort.vcp())
        ],
        "only the owner writes"
    );
    assert_eq!(
        geometry(&d, Machine::A),
        (vec!["mac".into()], vec!["pc-34".into()])
    );
    geometry_until(&d, Machine::B, &["mac"], &["pc-34"]).await;
    assert_eq!(d.mac.state().scene.as_deref(), Some("only-b"));
    scene_until(&d, Machine::B, Some("only-b")).await;
    d.check().await.unwrap();
}

/// The 27" gives a failed verdict: the 34", which had already switched, goes back to the PC; the
/// active scene stays the previous one and the geometry did not move.
#[tokio::test(start_paused = true)]
async fn a_verdict_fails_and_the_already_switched_monitors_roll_back() {
    let d = paired(SwitchBehavior::Fail, true).await;
    declare_screens(&d);
    d.monitors().unwrap().clear_ops();

    let run = d.apply_scene(Machine::A, "only-a").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    assert_eq!(run.failure().map(|f| f.0), Some(1), "{:#?}", run.events);
    assert_eq!(undone(&run), [0]);
    assert_eq!(
        on_screen(&d, M34),
        Some(Input::DisplayPort),
        "went back to the PC"
    );
    assert_eq!(
        on_screen(&d, M27),
        Some(Input::DisplayPort),
        "never switched"
    );
    assert_eq!(
        input_sets(&d, M34),
        [
            (Machine::B, Input::Hdmi.vcp()),
            (Machine::B, Input::DisplayPort.vcp())
        ]
    );
    assert_eq!(d.mac.state().scene.as_deref(), Some("work"));
    for m in Machine::ALL {
        geometry_until(&d, m, &["mac"], &["pc-34"]).await;
        scene_until(&d, m, Some("work")).await;
    }
}

/// The 27" switches but never gives a verdict: it might have switched, so it goes back too, and
/// then the 34".
#[tokio::test(start_paused = true)]
async fn without_a_verdict_the_monitor_that_failed_also_rolls_back() {
    let d = paired(SwitchBehavior::Silent, true).await;
    let run = d.apply_scene(Machine::A, "only-a").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    let (index, error) = run.failure().unwrap();
    assert_eq!(index, 1);
    assert!(error.contains(tandem_monitor::NO_VERDICT), "{error}");
    assert_eq!(undone(&run), [0]);
    assert_eq!(on_screen(&d, M34), Some(Input::DisplayPort));
    assert_eq!(on_screen(&d, M27), Some(Input::DisplayPort));
    assert_eq!(
        input_sets(&d, M27),
        [
            (Machine::B, Input::Hdmi.vcp()),
            (Machine::B, Input::DisplayPort.vcp())
        ]
    );
    assert_eq!(d.mac.state().scene.as_deref(), Some("work"));
}

/// The active scene comes from the real input: someone switches the inputs from the monitor's
/// menu and both machines recognize "Only A"; with only one switched there is none.
#[tokio::test(start_paused = true)]
async fn the_active_scene_comes_from_the_real_input() {
    let d = paired(SwitchBehavior::Done, true).await;
    let sim = d.monitors().unwrap().clone();
    sim.fake().osd_switch(sim.handle(M34), Input::Hdmi);
    sim.fake().osd_switch(sim.handle(M27), Input::Hdmi);
    for m in Machine::ALL {
        scene_until(&d, m, Some("only-a")).await;
    }
    sim.fake().osd_switch(sim.handle(M34), Input::DisplayPort);
    for m in Machine::ALL {
        scene_until(&d, m, None).await;
    }
}

/// The editor only lets in compatible monitors whose input is known: the factory one and one
/// that is not seen, no; the 27" without a known input, until its cable is declared.
#[tokio::test(start_paused = true)]
async fn the_editor_only_accepts_compatible_monitors() {
    let d = paired(SwitchBehavior::Done, false).await;
    let (m34, m27, fab) = (id(&d, M34), id(&d, M27), id(&d, FAB));
    let add = |monitor: &str| {
        d.mac.handle(UiCommand::EditDesk {
            patch: DeskPatch::AddScene(Scene {
                monitors: vec![(monitor.to_owned(), Machine::B)],
                ..Scene::new("", format!("With {monitor}"))
            }),
        })
    };
    for (monitor, why) in [
        (fab.as_str(), "is not compatible"),
        ("ghost", "is not seen"),
        (m27.as_str(), "it is not known which input"),
    ] {
        match add(monitor) {
            Reply::Rejected(e) => assert!(e.contains(why), "{monitor}: {e}"),
            other => panic!("{monitor}: was accepted: {other:?}"),
        }
    }
    assert!(matches!(add(&m34), Reply::Desk(_)));
    // With its cable declared (the PC over DP), the 27" now comes in.
    let mut cable = tandem_core::MonitorCable::new(m27.clone());
    cable.b.input = Some(MonitorInput::DisplayPort);
    d.mac.edit_desk(DeskPatch::Cable(cable)).unwrap();
    assert!(matches!(add(&m27), Reply::Desk(_)));
    // A scene that already had it can be renamed even if the monitor stops being seen.
    let mut scene = d.mac.desk().scenes[3].clone();
    sim_unplug(&d, M34);
    d.wait_monitors(Machine::A, "the disconnected 34\"", |v| {
        v.iter().all(|m| m.id != m34)
    })
    .await
    .unwrap();
    scene.name = "Renamed".into();
    let reply = d.mac.handle(UiCommand::EditDesk {
        patch: DeskPatch::EditScene(scene),
    });
    assert!(matches!(reply, Reply::Desk(_)), "{reply:?}");
}

fn sim_unplug(d: &Desk, i: usize) {
    let sim = d.monitors().unwrap();
    for m in Machine::ALL {
        sim.set_cable(i, m, None);
    }
}

/// T-280: right after pairing, the non-owner (the Mac) read the 34" itself for a moment, while it
/// did not know the PC's channel yet and believed it owned it. That early reading must not hide
/// the owner's report: the owner has nothing new to send, and without its report every scene
/// applied from the Mac failed with "it is not known which machine is shown".
#[tokio::test(start_paused = true)]
async fn an_early_claim_of_the_non_owner_does_not_hide_the_owners_report() {
    let d = paired(SwitchBehavior::Done, true).await;
    let m34 = id(&d, M34);
    let has_owner_report = |v: &[tandem_proto::MonitorView]| {
        v.iter().any(|m| {
            m.id == m34
                && m.owner == Some(Machine::B)
                && m.report.as_ref().and_then(|r| r.input) == Some(MonitorInput::DisplayPort)
        })
    };
    d.wait_monitors(Machine::A, "the owner's report", has_owner_report)
        .await
        .unwrap();

    // The Mac's early claim, as its own monitor loop stores it: a reading tagged with itself.
    d.mac.monitor_hub().receive(
        Machine::A,
        vec![tandem_proto::MonitorReport {
            id: m34.clone(),
            input: Some(MonitorInput::Hdmi),
            power_on: Some(true),
            signal: Some(true),
            events: vec![],
        }],
    );
    // Several scans and polls go by; the owner reads nothing new.
    tokio::time::sleep(Duration::from_secs(30)).await;
    let views = d.mac.monitors();
    assert!(has_owner_report(&views), "{views:#?}");

    let run = d.apply_scene(Machine::A, "only-a").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert_eq!(on_screen(&d, M34), Some(Input::Hdmi));
}
