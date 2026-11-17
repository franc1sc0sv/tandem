//! Escenas con monitores de verdad (T-111, D6, D13): dos agentes reales con los dobles de
//! `monitor-ddc`. El paso de escena «que el 34" muestre B» es un cambio de entrada que hace el
//! dueño (la PC, más rápida), con veredicto del firmware; la geometría saca la pantalla de la
//! máquina que deja de verse; un veredicto fallido vuelve atrás los monitores ya cambiados; la
//! escena activa sale de la entrada real; el editor solo acepta monitores compatibles.

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

/// El 34" y un 27" con el firmware propio (la PC por DP, más rápida; la Mac por HDMI, sin
/// tipo de interfaz en su EDID) y uno de fábrica que solo ve la PC. Del 27" el EDID de la PC
/// dice la entrada solo si `known27`.
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
        .unwrap_or_else(|_| panic!("{m}: la escena activa no llegó a {scene:?}"))
        .unwrap();
}

/// Las dos prendidas y emparejadas; la PC (dueña) presentó los monitores compatibles cuyas
/// entradas se saben, y las dos reconocen «Trabajo» por la entrada real (todo en DP, la PC).
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
        desk_until(&d, m, "los monitores presentados", |p| {
            p.cable(&m34).is_some() && p.cable(&m27).is_some() == known27 && p.scenes.len() == 3
        })
        .await;
    }
    let desk = d.mac.desk();
    let cable = desk.cable(&m34).unwrap();
    assert_eq!(
        (cable.a.input, cable.b.input),
        (Some(MonitorInput::Hdmi), Some(MonitorInput::DisplayPort)),
        "la Mac va a la otra entrada"
    );
    assert!(desk.cable(&id(&d, FAB)).is_none(), "el de fábrica no entra");
    let n = if known27 { 2 } else { 1 };
    let scene = |sid: &str| desk.scenes.iter().find(|s| s.id == sid).unwrap().clone();
    assert_eq!(scene("solo-a").monitors.len(), n);
    assert!(
        scene("solo-a")
            .monitors
            .iter()
            .all(|(_, m)| *m == Machine::A)
    );
    assert!(
        scene("solo-b")
            .monitors
            .iter()
            .all(|(_, m)| *m == Machine::B)
    );
    assert_eq!(scene("trabajo").monitors[0], (m34.clone(), Machine::B));
    for m in Machine::ALL {
        scene_until(&d, m, Some("trabajo")).await;
    }
    d
}

/// Pantallas declaradas: la Mac con la suya y el 34"; la PC solo con el 34". El cableado dice
/// qué pantalla de cada una es el 34".
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

/// Las pantallas de A y de B en la geometría de `on`, con lo que muestra cada monitor según
/// lo que leyó (lo mismo que calcula su motor de entrada).
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
    // Cambia con el puesto, el estado y los monitores: se mira cada tanto (reloj pausado).
    tokio::time::timeout(LIMIT, async {
        while geometry(d, on) != want {
            tokio::time::sleep(ms(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "{on}: geometría {:?}, se esperaba {want:?}",
            geometry(d, on)
        )
    });
}

fn steps(run: &TxnRun) -> Vec<Step> {
    match run.events.first() {
        Some(TxnEvent::Started { steps, .. }) => steps.clone(),
        other => panic!("sin inicio: {other:?}"),
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

/// Los `Set 0x60` al monitor `i`: quién y a qué entrada.
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

/// Aplicar «Solo A» y después «Solo B» desde la Mac: cada monitor lo cambia la PC (su dueña)
/// con el veredicto del firmware, y la geometría de las dos máquinas saca el 34" de la que
/// deja de verse en él.
#[tokio::test(start_paused = true)]
async fn solo_b_cambia_los_monitores_y_la_geometria() {
    let d = paired(SwitchBehavior::Done, true).await;
    let (m34, m27) = (id(&d, M34), id(&d, M27));
    declare_screens(&d);
    for m in Machine::ALL {
        geometry_until(&d, m, &["mac"], &["pc-34"]).await;
    }
    d.monitors().unwrap().clear_ops();

    let run = d.apply_scene(Machine::A, "solo-a").await.unwrap();
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
    scene_until(&d, Machine::B, Some("solo-a")).await;

    let run = d.apply_scene(Machine::A, "solo-b").await.unwrap();
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
        "solo la dueña escribe"
    );
    assert_eq!(
        geometry(&d, Machine::A),
        (vec!["mac".into()], vec!["pc-34".into()])
    );
    geometry_until(&d, Machine::B, &["mac"], &["pc-34"]).await;
    assert_eq!(d.mac.state().scene.as_deref(), Some("solo-b"));
    scene_until(&d, Machine::B, Some("solo-b")).await;
    d.check().await.unwrap();
}

/// El 27" da veredicto fallido: el 34", que ya había cambiado, vuelve a la PC; la escena
/// activa sigue siendo la de antes y la geometría no se movió.
#[tokio::test(start_paused = true)]
async fn falla_un_veredicto_y_vuelven_los_monitores_ya_cambiados() {
    let d = paired(SwitchBehavior::Fail, true).await;
    declare_screens(&d);
    d.monitors().unwrap().clear_ops();

    let run = d.apply_scene(Machine::A, "solo-a").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    assert_eq!(run.failure().map(|f| f.0), Some(1), "{:#?}", run.events);
    assert_eq!(undone(&run), [0]);
    assert_eq!(
        on_screen(&d, M34),
        Some(Input::DisplayPort),
        "volvió a la PC"
    );
    assert_eq!(on_screen(&d, M27), Some(Input::DisplayPort), "nunca cambió");
    assert_eq!(
        input_sets(&d, M34),
        [
            (Machine::B, Input::Hdmi.vcp()),
            (Machine::B, Input::DisplayPort.vcp())
        ]
    );
    assert_eq!(d.mac.state().scene.as_deref(), Some("trabajo"));
    for m in Machine::ALL {
        geometry_until(&d, m, &["mac"], &["pc-34"]).await;
        scene_until(&d, m, Some("trabajo")).await;
    }
}

/// El 27" cambia pero nunca da veredicto: pudo haber cambiado, así que vuelve él también, y
/// después el 34".
#[tokio::test(start_paused = true)]
async fn sin_veredicto_vuelve_tambien_el_monitor_que_fallo() {
    let d = paired(SwitchBehavior::Silent, true).await;
    let run = d.apply_scene(Machine::A, "solo-a").await.unwrap();
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
    assert_eq!(d.mac.state().scene.as_deref(), Some("trabajo"));
}

/// La escena activa sale de la entrada real: alguien cambia las entradas desde el menú del
/// monitor y las dos máquinas reconocen «Solo A»; con una sola cambiada no hay ninguna.
#[tokio::test(start_paused = true)]
async fn la_escena_activa_sale_de_la_entrada_real() {
    let d = paired(SwitchBehavior::Done, true).await;
    let sim = d.monitors().unwrap().clone();
    sim.fake().osd_switch(sim.handle(M34), Input::Hdmi);
    sim.fake().osd_switch(sim.handle(M27), Input::Hdmi);
    for m in Machine::ALL {
        scene_until(&d, m, Some("solo-a")).await;
    }
    sim.fake().osd_switch(sim.handle(M34), Input::DisplayPort);
    for m in Machine::ALL {
        scene_until(&d, m, None).await;
    }
}

/// El editor solo deja entrar monitores compatibles cuya entrada se sabe: el de fábrica y uno
/// que no se ve, no; el 27" sin entrada conocida, hasta que se declara su cable.
#[tokio::test(start_paused = true)]
async fn el_editor_solo_acepta_monitores_compatibles() {
    let d = paired(SwitchBehavior::Done, false).await;
    let (m34, m27, fab) = (id(&d, M34), id(&d, M27), id(&d, FAB));
    let add = |monitor: &str| {
        d.mac.handle(UiCommand::EditDesk {
            patch: DeskPatch::AddScene(Scene {
                monitors: vec![(monitor.to_owned(), Machine::B)],
                ..Scene::new("", format!("Con {monitor}"))
            }),
        })
    };
    for (monitor, why) in [
        (fab.as_str(), "no es compatible"),
        ("fantasma", "no se ve"),
        (m27.as_str(), "no se sabe a qué entrada"),
    ] {
        match add(monitor) {
            Reply::Rejected(e) => assert!(e.contains(why), "{monitor}: {e}"),
            other => panic!("{monitor}: se aceptó: {other:?}"),
        }
    }
    assert!(matches!(add(&m34), Reply::Desk(_)));
    // Con su cable declarado (la PC por DP), el 27" ya entra.
    let mut cable = tandem_core::MonitorCable::new(m27.clone());
    cable.b.input = Some(MonitorInput::DisplayPort);
    d.mac.edit_desk(DeskPatch::Cable(cable)).unwrap();
    assert!(matches!(add(&m27), Reply::Desk(_)));
    // Una escena que ya lo tenía se puede renombrar aunque el monitor deje de verse.
    let mut scene = d.mac.desk().scenes[3].clone();
    sim_unplug(&d, M34);
    d.wait_monitors(Machine::A, "el 34\" desconectado", |v| {
        v.iter().all(|m| m.id != m34)
    })
    .await
    .unwrap();
    scene.name = "Renombrada".into();
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
