//! A monitor recognized by its EDID without a DDC channel (T-340), with two real agents and
//! the `monitor-ddc` doubles. The 34" only answers DDC on the input it shows: the PC (B, over DP)
//! has the channel while it shows the PC, and the Mac (A, over HDMI) only has the EDID. The desk
//! still gets one monitor with both connections, learns which screen it is on each machine and
//! the geometry drops the screen of the machine the monitor does not show.

use std::time::Duration;

use monitor_ddc::fake::{FakeMonitorConfig, Personality, test_edid};
use tandem_core::{DeskPatch, DisplayKey, Machine, MonitorInput, Scale, ScreenSpec};
use tandem_input::{Rect, Screen};
use tandem_proto::{MonitorCompat, MonitorView};
use tandem_sim::{Desk, MonitorSpec, Outcome};
use tandemd::input::screens;

const LIMIT: Duration = Duration::from_secs(600);
const M34: usize = 0;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn specs() -> Vec<MonitorSpec> {
    vec![MonitorSpec {
        config: FakeMonitorConfig::new(Personality::Standard),
        cables: vec![(Machine::A, ms(30)), (Machine::B, ms(5))],
        via: vec![
            (Machine::A, MonitorInput::Hdmi),
            (Machine::B, MonitorInput::DisplayPort),
        ],
    }]
}

fn key34() -> DisplayKey {
    DisplayKey::from_edid(&FakeMonitorConfig::new(Personality::Standard).edid).unwrap()
}

fn builtin() -> DisplayKey {
    DisplayKey::from_edid(&test_edid("APP", 0xA050, "Color LCD", "BUILTIN0001")).unwrap()
}

fn screen(id: &str, x: i32, width: u32, height: u32) -> Screen {
    Screen::new(id.into(), Rect::new(x, 0, width, height), 1.0)
}

/// What each input engine reads: the Mac its panel and the 34" (HDMI), the PC the 34" (DP).
fn read_screens(of: Machine) -> Vec<Screen> {
    match of {
        Machine::A => vec![
            screen("screen-1", 0, 1512, 982),
            screen("screen-2", 1512, 3440, 1440),
        ],
        Machine::B => vec![screen("screen-1", 0, 3440, 1440)],
    }
}

fn feed_screen_keys(d: &Desk) {
    d.agent(Machine::A).set_screen_keys(vec![
        ("screen-1".into(), builtin()),
        ("screen-2".into(), key34()),
    ]);
    d.agent(Machine::B)
        .set_screen_keys(vec![("screen-1".into(), key34())]);
}

/// Both on and paired, with the Mac's HDMI silent: the PC owns the 34" and introduced it.
async fn desk() -> (Desk, String) {
    let d = Desk::builder().monitors(specs()).build().await.unwrap();
    let sim = d.monitors().unwrap().clone();
    sim.set_ddc(M34, Machine::A, false);
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    let m34 = sim.id(M34).to_owned();
    for m in Machine::ALL {
        desk_until(&d, m, "the 34\" introduced", |p| p.cable(&m34).is_some()).await;
    }
    (d, m34)
}

fn view<'a>(views: &'a [MonitorView], id: &str) -> Option<&'a MonitorView> {
    views.iter().find(|v| v.id == id)
}

async fn desk_until(d: &Desk, m: Machine, what: &str, pred: impl Fn(&tandem_core::Desk) -> bool) {
    let mut desk = d.agent(m).subscribe_desk();
    tokio::time::timeout(LIMIT, desk.wait_for(|d| pred(d)))
        .await
        .unwrap_or_else(|_| panic!("{m}: {what}; desk: {:#?}", d.agent(m).desk().cables))
        .unwrap();
}

/// A's and B's screens in `on`'s geometry, with what each one reads and what each monitor shows
/// (the same as its input engine computes).
fn geometry(d: &Desk, on: Machine) -> (Vec<String>, Vec<String>) {
    let agent = d.agent(on);
    let desk = agent.desk();
    let shows = tandemd::monitors::shows(&agent.monitors(), &desk);
    let (mine, theirs) = (read_screens(on), read_screens(on.other()));
    let arr = screens::arrangement(&desk, &agent.state(), on, &mine, &theirs, &shows);
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

#[tokio::test(start_paused = true)]
async fn the_desk_learns_each_screen_and_drops_the_one_the_monitor_does_not_show() {
    let (d, m34) = desk().await;
    let sim = d.monitors().unwrap().clone();

    // One monitor, both connections, owned by the PC; the Mac announces it without a channel.
    for m in Machine::ALL {
        let views = d
            .wait_monitors(m, "both connections", |v| {
                view(v, &m34).is_some_and(|x| x.via.len() == 2 && x.owner == Some(Machine::B))
            })
            .await
            .unwrap();
        assert_eq!(views.len(), 1, "{views:#?}");
        let v = &views[0];
        assert!(v.compat.is_compatible());
        assert_eq!(v.seen_by, [Machine::A, Machine::B]);
        assert_eq!(
            v.via,
            [
                (Machine::A, MonitorInput::Hdmi),
                (Machine::B, MonitorInput::DisplayPort)
            ]
        );
    }
    let mac = d.agent(Machine::A).monitor_hub().local_channels();
    assert_eq!(mac.len(), 1);
    assert_eq!(mac[0].compat, MonitorCompat::NoChannel);
    assert!(
        sim.sets_to(sim.handle(M34))
            .iter()
            .all(|(m, _, _)| *m == Machine::B),
        "only the owner writes"
    );

    // Each input engine tells which monitor each of its screens is: the desk learns both ends.
    feed_screen_keys(&d);
    for m in Machine::ALL {
        desk_until(&d, m, "both screens learned", |p| {
            p.cable(&m34).is_some_and(|c| {
                c.a.screen.as_deref() == Some("screen-2")
                    && c.b.screen.as_deref() == Some("screen-1")
            })
        })
        .await;
    }

    // The 34" shows the PC: the Mac's HDMI screen leaves the geometry of both.
    for on in Machine::ALL {
        geometry_until(&d, on, &["screen-1"], &["screen-1"]).await;
    }

    // The scene gives the 34" to the Mac: now its HDMI answers and the PC's DP is silent.
    let run = d
        .switch_monitor(Machine::A, &m34, MonitorInput::Hdmi)
        .await
        .unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    sim.set_ddc(M34, Machine::A, true);
    sim.set_ddc(M34, Machine::B, false);
    for on in Machine::ALL {
        d.wait_monitors(on, "the Mac owns the 34\"", |v| {
            view(v, &m34).is_some_and(|x| x.owner == Some(Machine::A))
        })
        .await
        .unwrap();
        geometry_until(&d, on, &["screen-1", "screen-2"], &[]).await;
    }
    // The learned screens stay: nothing to learn again.
    let cable = d.mac.desk().cable(&m34).unwrap().clone();
    assert_eq!(
        (cable.a.screen.as_deref(), cable.b.screen.as_deref()),
        (Some("screen-2"), Some("screen-1"))
    );
}

/// A screen declared by hand is never overwritten, and a machine with `[[screens]]` declared
/// learns nothing (their ids are the user's, not `screen-N`).
#[tokio::test(start_paused = true)]
async fn a_declared_screen_is_never_overwritten() {
    let (d, m34) = desk().await;
    let mut cable = d.mac.desk().cable(&m34).unwrap().clone();
    cable.a.screen = Some("hdmi-by-hand".into());
    d.mac.edit_desk(DeskPatch::Cable(cable)).unwrap();
    for m in Machine::ALL {
        desk_until(&d, m, "the hand-declared screen", |p| {
            p.cable(&m34)
                .is_some_and(|c| c.a.screen.as_deref() == Some("hdmi-by-hand"))
        })
        .await;
    }
    feed_screen_keys(&d);
    for m in Machine::ALL {
        desk_until(&d, m, "the PC's screen learned", |p| {
            p.cable(&m34)
                .is_some_and(|c| c.b.screen.as_deref() == Some("screen-1"))
        })
        .await;
    }
    let cable = d.pc.desk().cable(&m34).unwrap().clone();
    assert_eq!(cable.a.screen.as_deref(), Some("hdmi-by-hand"));

    // With `[[screens]]` declared, the PC learns nothing more.
    let mut cable = cable;
    cable.b.screen = None;
    d.pc.edit_desk(DeskPatch::Cable(cable)).unwrap();
    d.pc.edit_desk(DeskPatch::Screens {
        machine: Machine::B,
        screens: vec![ScreenSpec {
            machine: Machine::B,
            id: "pc-34".into(),
            x: 0,
            y: 0,
            width: 3440,
            height: 1440,
            scale: Scale::default(),
        }],
    })
    .unwrap();
    d.agent(Machine::B).set_screen_keys(Vec::new());
    feed_screen_keys(&d);
    tokio::time::sleep(Duration::from_secs(30)).await;
    for m in Machine::ALL {
        let cable = d.agent(m).desk().cable(&m34).unwrap().clone();
        assert_eq!(cable.b.screen, None, "{m}");
        assert_eq!(cable.a.screen.as_deref(), Some("hdmi-by-hand"), "{m}");
    }
}
