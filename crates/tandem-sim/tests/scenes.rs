//! Escenarios de las escenas (T-100, D6): aplicar una de punta a punta con dos agentes, la
//! vuelta atrás completa cuando falla un paso y encender primero la máquina que necesita.
//!
//! Los monitores de escena son el doble del paso abstracto ([`tandem_sim::SceneScreens`]);
//! T-111 los conecta a los monitores reales.

use std::time::Duration;

use tandem_core::{Align, Arrangement, DeskPatch, Machine, Scene, Side, Step};
use tandem_proto::{Reply, TxnEvent, UiCommand};
use tandem_sim::{Desk, FaultAction, FaultPlan, Outcome, Power, TxnRun};

const LIMIT: Duration = Duration::from_secs(600);

/// Las dos prendidas, emparejadas, con las escenas por omisión en las dos.
async fn paired(d: &Desk) {
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    for m in Machine::ALL {
        has_defaults(d, m).await;
    }
    d.pc_agrees().await.unwrap();
}

async fn has_defaults(d: &Desk, m: Machine) {
    let mut desk = d.agent(m).subscribe_desk();
    tokio::time::timeout(
        LIMIT,
        desk.wait_for(|d| {
            let ids: Vec<&str> = d.scenes.iter().map(|s| s.id.as_str()).collect();
            ids == ["trabajo", "solo-a", "solo-b"]
        }),
    )
    .await
    .expect("las escenas por omisión no llegaron")
    .unwrap();
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

/// Una escena nueva desde la UI de `on`.
fn add(d: &Desk, on: Machine, scene: Scene) {
    let reply = d.agent(on).handle(UiCommand::EditDesk {
        patch: DeskPatch::AddScene(scene),
    });
    assert!(matches!(reply, Reply::Desk(_)), "{reply:?}");
}

/// Al emparejar aparecen «Trabajo», «Solo A» y «Solo B». Con la otra apagada, aplicar
/// «Trabajo» (las dos, con acomodo) la enciende primero y después acomoda: queda como la
/// escena activa en las dos.
#[tokio::test(start_paused = true)]
async fn trabajo_con_la_otra_apagada_la_enciende_primero() {
    let d = Desk::new().await.unwrap();
    paired(&d).await;
    let names: Vec<String> = d.mac.desk().scenes.iter().map(|s| s.name.clone()).collect();
    assert_eq!(names[0], "Trabajo");
    assert_eq!(
        names[1],
        format!("Solo {}", d.mac.desk().machines.name(Machine::A))
    );
    assert_eq!(
        names[2],
        format!("Solo {}", d.mac.desk().machines.name(Machine::B))
    );

    let off = d.shutdown_pc().await.unwrap();
    assert_eq!(off.outcome(), Some(Outcome::Committed));
    d.settle().await;
    assert_eq!(d.world.power_of(Machine::B), Power::Off);

    let run = d.apply_scene(Machine::A, "trabajo").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert_eq!(
        steps(&run),
        [
            Step::PowerOn {
                machine: Machine::B
            },
            Step::Arrange {
                layout: Some(Arrangement::default())
            },
            Step::Geometry,
        ]
    );
    assert_eq!(d.world.power_of(Machine::B), Power::On);
    assert_eq!(d.mac.desk().arrangement, Some(Arrangement::default()));
    assert_eq!(d.scene_screens().geometry_of(Machine::A), 1);
    assert_eq!(d.mac.state().scene.as_deref(), Some("trabajo"));
    let pc = d.pc_agrees().await.unwrap();
    assert_eq!(pc.scene.as_deref(), Some("trabajo"));
    d.check().await.unwrap();

    // Aplicarla otra vez no tiene nada que hacer, y sigue activa.
    let again = d.apply_scene(Machine::A, "trabajo").await.unwrap();
    assert_eq!(again.outcome(), Some(Outcome::Committed));
    assert!(steps(&again).is_empty(), "{:?}", steps(&again));

    // Se apaga la que «Trabajo» necesita: deja de ser la activa.
    d.shutdown_pc().await.unwrap();
    d.settle().await;
    assert_eq!(d.mac.state().scene, None);
}

/// «Solo B» pedido desde B con A apagada: también se enciende primero (el espejo).
#[tokio::test(start_paused = true)]
async fn solo_la_otra_desde_la_pc() {
    let d = Desk::builder().principal(Machine::B).build().await.unwrap();
    add(
        &d,
        Machine::B,
        Scene {
            require_on: Some(Machine::A),
            ..Scene::new("solo-mac", "Solo Mac")
        },
    );
    let run = d.apply_scene(Machine::B, "solo-mac").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert_eq!(
        steps(&run),
        [Step::PowerOn {
            machine: Machine::A
        }]
    );
    assert_eq!(d.world.power_of(Machine::A), Power::On);
    assert_eq!(d.pc.state().scene.as_deref(), Some("solo-mac"));
}

/// Una escena con dos monitores y acomodo: falla el segundo monitor → el primero vuelve a
/// lo que mostraba, el acomodo y la geometría no se tocan y la escena activa sigue siendo la
/// de antes. Después, con el monitor sano, se aplica entera.
#[tokio::test(start_paused = true)]
async fn falla_un_monitor_y_la_escena_se_deshace_entera() {
    let d = Desk::builder()
        .scene_monitors(&[("34", Machine::A), ("27", Machine::A)])
        .build()
        .await
        .unwrap();
    paired(&d).await;
    let up = Arrangement {
        a_side: Side::Up,
        align: Align::End,
    };
    add(
        &d,
        Machine::A,
        Scene {
            icon: "play".into(),
            monitors: vec![("34".into(), Machine::B), ("27".into(), Machine::B)],
            layout: Some(up),
            ..Scene::new("", "Juego")
        },
    );
    let solo = d.apply_scene(Machine::A, "solo-a").await.unwrap();
    assert_eq!(solo.outcome(), Some(Outcome::Committed));
    let before = d.scene_screens().shows();
    let desk_before = d.mac.desk().arrangement;

    d.arm(FaultPlan::new().on(
        Machine::A,
        Step::ShowMachine {
            monitor: "27".into(),
            machine: Machine::B,
        },
        FaultAction::Fail("sin veredicto".into()),
    ));
    let run = d.apply_scene(Machine::A, "juego").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    assert_eq!(run.failure().map(|f| f.0), Some(1));
    assert!(run.failure().unwrap().1.contains("sin veredicto"));
    assert_eq!(undone(&run), [0]);
    assert_eq!(d.scene_screens().shows(), before, "el 34 volvió a la Mac");
    assert_eq!(
        d.scene_screens().log(),
        [
            (Machine::A, "34".to_owned(), Machine::B),
            (Machine::A, "34".to_owned(), Machine::A),
        ]
    );
    assert_eq!(d.mac.desk().arrangement, desk_before);
    assert_eq!(d.scene_screens().geometry_of(Machine::A), 0);
    assert_eq!(d.mac.state().scene.as_deref(), Some("solo-a"));
    d.check().await.unwrap();

    // Sin el fallo, la misma escena se aplica entera y llega a la otra.
    let run = d.apply_scene(Machine::A, "juego").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert!(d.scene_screens().shows().values().all(|&m| m == Machine::B));
    assert_eq!(d.mac.desk().arrangement, Some(up));
    assert_eq!(d.pc_agrees().await.unwrap().scene.as_deref(), Some("juego"));
}

/// Si falla la geometría (el último paso), también vuelven los monitores y el acomodo.
#[tokio::test(start_paused = true)]
async fn falla_la_geometria_y_vuelven_monitores_y_acomodo() {
    let d = Desk::builder()
        .scene_monitors(&[("34", Machine::A)])
        .build()
        .await
        .unwrap();
    paired(&d).await;
    let left_start = Arrangement {
        a_side: Side::Left,
        align: Align::Start,
    };
    add(
        &d,
        Machine::A,
        Scene {
            monitors: vec![("34".into(), Machine::B)],
            layout: Some(left_start),
            ..Scene::new("pc", "PC grande")
        },
    );
    let arrangement = d.mac.desk().arrangement;
    d.arm(FaultPlan::new().on(
        Machine::A,
        Step::Geometry,
        FaultAction::Fail("el motor no respondió".into()),
    ));
    let run = d.apply_scene(Machine::A, "pc").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    assert_eq!(undone(&run), [1, 0]);
    assert_eq!(d.scene_screens().shows()["34"], Machine::A);
    assert_eq!(d.mac.desk().arrangement, arrangement);
    // El acomodo también volvió en la otra máquina.
    let mut pc = d.pc.subscribe_desk();
    tokio::time::timeout(LIMIT, pc.wait_for(|p| p.arrangement == arrangement))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.mac.state().scene, None);
}

/// Una escena que no existe, o con un monitor que no se sabe controlar, no cambia nada.
#[tokio::test(start_paused = true)]
async fn una_escena_imposible_no_toca_nada() {
    let d = Desk::new().await.unwrap();
    paired(&d).await;
    let Reply::Rejected(why) = d.mac.handle(UiCommand::ApplyScene { id: "nada".into() }) else {
        panic!("se aceptó una escena que no existe");
    };
    assert!(why.contains("no existe"), "{why}");
    // Un monitor que no se ve no entra en una escena (T-111: el editor lo rechaza).
    let reply = d.mac.handle(UiCommand::EditDesk {
        patch: DeskPatch::AddScene(Scene {
            monitors: vec![("fantasma".into(), Machine::B)],
            ..Scene::new("f", "Fantasma")
        }),
    });
    let Reply::Rejected(why) = reply else {
        panic!("se aceptó un monitor que no existe: {reply:?}");
    };
    assert!(why.contains("fantasma"), "{why}");
    assert!(d.mac.desk().scenes.iter().all(|s| s.id != "f"));
    assert!(d.scene_screens().log().is_empty());
}

/// Espera a que el puesto de `m` cumpla `pred`.
async fn desk_until(d: &Desk, m: Machine, what: &str, pred: impl Fn(&tandem_core::Desk) -> bool) {
    let mut desk = d.agent(m).subscribe_desk();
    tokio::time::timeout(LIMIT, desk.wait_for(|d| pred(d)))
        .await
        .unwrap_or_else(|_| panic!("{m}: {what}"))
        .unwrap();
}

/// Revisión de #82 (D4/D5): una escena creada en A llega a B; editada en B llega a A; B borra
/// la escena activa y las dos se quedan sin ella, también como escena activa.
#[tokio::test(start_paused = true)]
async fn las_escenas_se_crean_editan_y_borran_en_las_dos() {
    let d = Desk::new().await.unwrap();
    paired(&d).await;
    add(
        &d,
        Machine::A,
        Scene {
            require_on: Some(Machine::B),
            shortcut: vec!["ControlLeft".into(), "AltLeft".into(), "Digit4".into()],
            ..Scene::new("", "Juego")
        },
    );
    let juego = d.mac.desk().scenes[3].clone();
    assert_eq!(juego.id, "juego");
    desk_until(&d, Machine::B, "la escena nueva", |p| {
        p.scenes.contains(&juego)
    })
    .await;

    let edited = Scene {
        icon: "play".into(),
        name: "Juego en la PC".into(),
        ..juego.clone()
    };
    let reply = d.pc.handle(UiCommand::EditDesk {
        patch: DeskPatch::EditScene(edited.clone()),
    });
    assert!(matches!(reply, Reply::Desk(_)), "{reply:?}");
    desk_until(&d, Machine::A, "la escena editada", |p| {
        p.scenes.contains(&edited)
    })
    .await;

    // Activa en las dos.
    let run = d.apply_scene(Machine::A, "juego").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert_eq!(d.pc_agrees().await.unwrap().scene.as_deref(), Some("juego"));

    // B la borra: desaparece de las dos y ninguna la tiene como activa.
    let reply = d.pc.handle(UiCommand::EditDesk {
        patch: DeskPatch::DeleteScene { id: "juego".into() },
    });
    assert!(matches!(reply, Reply::Desk(_)), "{reply:?}");
    desk_until(&d, Machine::A, "la escena borrada", |p| {
        p.scenes.iter().all(|s| s.id != "juego")
    })
    .await;
    assert_eq!(d.pc.state().scene, None);
    let mut mac = d.mac.subscribe();
    tokio::time::timeout(LIMIT, mac.wait_for(|s| s.scene.is_none()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.mac.desk(), d.pc.desk());
}
