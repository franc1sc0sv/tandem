//! Scene scenarios (T-100, D6): applying one end to end with two agents, the full
//! rollback when a step fails and powering on first the machine it needs.
//!
//! The scene monitors are the abstract step's double ([`tandem_sim::SceneScreens`]);
//! T-111 connects them to the real monitors.

use std::time::Duration;

use tandem_core::{Align, Arrangement, DeskPatch, Machine, Scene, Side, Step};
use tandem_proto::{Reply, TxnEvent, UiCommand};
use tandem_sim::{Desk, FaultAction, FaultPlan, Outcome, Power, TxnRun};

const LIMIT: Duration = Duration::from_secs(600);

/// Both on, paired, with the default scenes on both.
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
            ids == ["work", "only-a", "only-b"]
        }),
    )
    .await
    .expect("the default scenes did not arrive")
    .unwrap();
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

/// A new scene from the UI of `on`.
fn add(d: &Desk, on: Machine, scene: Scene) {
    let reply = d.agent(on).handle(UiCommand::EditDesk {
        patch: DeskPatch::AddScene(scene),
    });
    assert!(matches!(reply, Reply::Desk(_)), "{reply:?}");
}

/// On pairing, "Work", "Only A" and "Only B" appear. With the other off, applying
/// "Work" (both, with arrangement) powers it on first and then arranges: it stays as the
/// active scene on both.
#[tokio::test(start_paused = true)]
async fn work_with_the_other_off_powers_it_on_first() {
    let d = Desk::new().await.unwrap();
    paired(&d).await;
    let names: Vec<String> = d.mac.desk().scenes.iter().map(|s| s.name.clone()).collect();
    assert_eq!(names[0], "Work");
    assert_eq!(
        names[1],
        format!("Only {}", d.mac.desk().machines.name(Machine::A))
    );
    assert_eq!(
        names[2],
        format!("Only {}", d.mac.desk().machines.name(Machine::B))
    );

    let off = d.shutdown_pc().await.unwrap();
    assert_eq!(off.outcome(), Some(Outcome::Committed));
    d.settle().await;
    assert_eq!(d.world.power_of(Machine::B), Power::Off);

    let run = d.apply_scene(Machine::A, "work").await.unwrap();
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
    assert_eq!(d.mac.state().scene.as_deref(), Some("work"));
    let pc = d.pc_agrees().await.unwrap();
    assert_eq!(pc.scene.as_deref(), Some("work"));
    d.check().await.unwrap();

    // Applying it again has nothing to do, and it stays active.
    let again = d.apply_scene(Machine::A, "work").await.unwrap();
    assert_eq!(again.outcome(), Some(Outcome::Committed));
    assert!(steps(&again).is_empty(), "{:?}", steps(&again));

    // The one "Work" needs is powered off: it stops being the active one.
    d.shutdown_pc().await.unwrap();
    d.settle().await;
    assert_eq!(d.mac.state().scene, None);
}

/// "Only B" requested from B with A off: it is also powered on first (the mirror).
#[tokio::test(start_paused = true)]
async fn only_the_other_from_the_pc() {
    let d = Desk::builder().primary(Machine::B).build().await.unwrap();
    add(
        &d,
        Machine::B,
        Scene {
            require_on: Some(Machine::A),
            ..Scene::new("only-mac", "Only Mac")
        },
    );
    let run = d.apply_scene(Machine::B, "only-mac").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert_eq!(
        steps(&run),
        [Step::PowerOn {
            machine: Machine::A
        }]
    );
    assert_eq!(d.world.power_of(Machine::A), Power::On);
    assert_eq!(d.pc.state().scene.as_deref(), Some("only-mac"));
}

/// A scene with two monitors and arrangement: the second monitor fails → the first goes back to
/// what it showed, the arrangement and the geometry are not touched and the active scene stays
/// the previous one. Then, with the healthy monitor, it is applied whole.
#[tokio::test(start_paused = true)]
async fn a_monitor_fails_and_the_whole_scene_rolls_back() {
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
            ..Scene::new("", "Game")
        },
    );
    let only_a = d.apply_scene(Machine::A, "only-a").await.unwrap();
    assert_eq!(only_a.outcome(), Some(Outcome::Committed));
    let before = d.scene_screens().shows();
    let desk_before = d.mac.desk().arrangement;

    d.arm(FaultPlan::new().on(
        Machine::A,
        Step::ShowMachine {
            monitor: "27".into(),
            machine: Machine::B,
        },
        FaultAction::Fail("no verdict".into()),
    ));
    let run = d.apply_scene(Machine::A, "game").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    assert_eq!(run.failure().map(|f| f.0), Some(1));
    assert!(run.failure().unwrap().1.contains("no verdict"));
    assert_eq!(undone(&run), [0]);
    assert_eq!(
        d.scene_screens().shows(),
        before,
        "the 34 went back to the Mac"
    );
    assert_eq!(
        d.scene_screens().log(),
        [
            (Machine::A, "34".to_owned(), Machine::B),
            (Machine::A, "34".to_owned(), Machine::A),
        ]
    );
    assert_eq!(d.mac.desk().arrangement, desk_before);
    assert_eq!(d.scene_screens().geometry_of(Machine::A), 0);
    assert_eq!(d.mac.state().scene.as_deref(), Some("only-a"));
    d.check().await.unwrap();

    // Without the fault, the same scene is applied whole and it reaches the other side.
    let run = d.apply_scene(Machine::A, "game").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert!(d.scene_screens().shows().values().all(|&m| m == Machine::B));
    assert_eq!(d.mac.desk().arrangement, Some(up));
    assert_eq!(d.pc_agrees().await.unwrap().scene.as_deref(), Some("game"));
}

/// If the geometry fails (the last step), the arrangement and monitors also go back.
#[tokio::test(start_paused = true)]
async fn geometry_fails_and_monitors_and_arrangement_roll_back() {
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
            ..Scene::new("pc", "Big PC")
        },
    );
    let arrangement = d.mac.desk().arrangement;
    d.arm(FaultPlan::new().on(
        Machine::A,
        Step::Geometry,
        FaultAction::Fail("the engine did not respond".into()),
    ));
    let run = d.apply_scene(Machine::A, "pc").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    assert_eq!(undone(&run), [1, 0]);
    assert_eq!(d.scene_screens().shows()["34"], Machine::A);
    assert_eq!(d.mac.desk().arrangement, arrangement);
    // The arrangement also went back on the other machine.
    let mut pc = d.pc.subscribe_desk();
    tokio::time::timeout(LIMIT, pc.wait_for(|p| p.arrangement == arrangement))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.mac.state().scene, None);
}

/// A scene that does not exist, or with a monitor that cannot be controlled, changes nothing.
#[tokio::test(start_paused = true)]
async fn an_impossible_scene_touches_nothing() {
    let d = Desk::new().await.unwrap();
    paired(&d).await;
    let Reply::Rejected(why) = d.mac.handle(UiCommand::ApplyScene { id: "none".into() }) else {
        panic!("a scene that does not exist was accepted");
    };
    assert!(why.contains("does not exist"), "{why}");
    // A monitor that is not seen does not go into a scene (T-111: the editor rejects it).
    let reply = d.mac.handle(UiCommand::EditDesk {
        patch: DeskPatch::AddScene(Scene {
            monitors: vec![("ghost".into(), Machine::B)],
            ..Scene::new("f", "Ghost")
        }),
    });
    let Reply::Rejected(why) = reply else {
        panic!("a monitor that does not exist was accepted: {reply:?}");
    };
    assert!(why.contains("ghost"), "{why}");
    assert!(d.mac.desk().scenes.iter().all(|s| s.id != "f"));
    assert!(d.scene_screens().log().is_empty());
}

/// Waits for `m`'s desk to satisfy `pred`.
async fn desk_until(d: &Desk, m: Machine, what: &str, pred: impl Fn(&tandem_core::Desk) -> bool) {
    let mut desk = d.agent(m).subscribe_desk();
    tokio::time::timeout(LIMIT, desk.wait_for(|d| pred(d)))
        .await
        .unwrap_or_else(|_| panic!("{m}: {what}"))
        .unwrap();
}

/// Review of #82 (D4/D5): a scene created on A reaches B; edited on B it reaches A; B deletes
/// the active scene and both are left without it, also as the active scene.
#[tokio::test(start_paused = true)]
async fn scenes_are_created_edited_and_deleted_on_both() {
    let d = Desk::new().await.unwrap();
    paired(&d).await;
    add(
        &d,
        Machine::A,
        Scene {
            require_on: Some(Machine::B),
            shortcut: vec!["ControlLeft".into(), "AltLeft".into(), "Digit4".into()],
            ..Scene::new("", "Game")
        },
    );
    let game = d.mac.desk().scenes[3].clone();
    assert_eq!(game.id, "game");
    desk_until(&d, Machine::B, "the new scene", |p| {
        p.scenes.contains(&game)
    })
    .await;

    let edited = Scene {
        icon: "play".into(),
        name: "Game on the PC".into(),
        ..game.clone()
    };
    let reply = d.pc.handle(UiCommand::EditDesk {
        patch: DeskPatch::EditScene(edited.clone()),
    });
    assert!(matches!(reply, Reply::Desk(_)), "{reply:?}");
    desk_until(&d, Machine::A, "the edited scene", |p| {
        p.scenes.contains(&edited)
    })
    .await;

    // Active on both.
    let run = d.apply_scene(Machine::A, "game").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert_eq!(d.pc_agrees().await.unwrap().scene.as_deref(), Some("game"));

    // B deletes it: it disappears from both and neither has it as the active one.
    let reply = d.pc.handle(UiCommand::EditDesk {
        patch: DeskPatch::DeleteScene { id: "game".into() },
    });
    assert!(matches!(reply, Reply::Desk(_)), "{reply:?}");
    desk_until(&d, Machine::A, "the scene deleted", |p| {
        p.scenes.iter().all(|s| s.id != "game")
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

/// T-129: a scene's shortcut and the lock one are edited from either machine and reach
/// the other through the desk; a clash (with the lock or with another scene) is rejected without touching
/// either.
#[tokio::test(start_paused = true)]
async fn shortcuts_are_edited_on_one_and_reach_the_other() {
    let d = Desk::new().await.unwrap();
    paired(&d).await;
    let work = d.mac.desk().scenes[0].clone();
    assert_eq!(work.id, "work");

    // A scene's shortcut, edited on A, reaches B.
    let new_keys = vec![
        "ControlLeft".to_owned(),
        "AltLeft".to_owned(),
        "Digit7".to_owned(),
    ];
    let edited = Scene {
        shortcut: new_keys.clone(),
        ..work.clone()
    };
    let reply = d.mac.handle(UiCommand::EditDesk {
        patch: DeskPatch::EditScene(edited.clone()),
    });
    assert!(matches!(reply, Reply::Desk(_)), "{reply:?}");
    desk_until(&d, Machine::B, "the scene shortcut", |p| {
        p.scenes.contains(&edited)
    })
    .await;

    // The lock one, edited on B, reaches A.
    let lock = vec!["ControlLeft".to_owned(), "KeyL".to_owned()];
    let reply = d.pc.handle(UiCommand::EditDesk {
        patch: DeskPatch::Shortcuts(tandem_core::Shortcuts { lock: lock.clone() }),
    });
    assert!(matches!(reply, Reply::Desk(_)), "{reply:?}");
    desk_until(&d, Machine::A, "the lock shortcut", |p| {
        p.shortcuts.lock == lock
    })
    .await;

    // Clashes: the lock with a scene's, and a scene with the lock.
    let before = d.mac.desk();
    let reply = d.mac.handle(UiCommand::EditDesk {
        patch: DeskPatch::Shortcuts(tandem_core::Shortcuts { lock: new_keys }),
    });
    assert!(!matches!(reply, Reply::Desk(_)), "{reply:?}");
    let other_scene = d.mac.desk().scenes[1].clone();
    let reply = d.mac.handle(UiCommand::EditDesk {
        patch: DeskPatch::EditScene(Scene {
            shortcut: lock,
            ..other_scene
        }),
    });
    assert!(!matches!(reply, Reply::Desk(_)), "{reply:?}");
    assert_eq!(d.mac.desk(), before);
    assert_eq!(d.pc.desk(), before);
}
