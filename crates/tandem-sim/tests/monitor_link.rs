//! Monitor link states (T-349) with two real agents and the `monitor-ddc` doubles: a display
//! change rescans at once and the new cable state reaches the other machine's UI state; a cable
//! that leaves a black screen refuses the switch before any DDC write; the cable safety rule; the
//! power debounce; a monitor with only `NoChannel` ends is never "off".

use std::time::Duration;

use monitor_ddc::Input;
use monitor_ddc::fake::{FakeMonitorConfig, Personality};
use tandem_core::{CableState, LinkState, Machine, MonitorInput, OFF_AFTER, PowerState};
use tandem_input::hotplug::FakeDisplayChanges;
use tandem_proto::MonitorView;
use tandem_sim::{Desk, MonitorSpec, Outcome, SimError};
use tokio::time::Instant;

const LIMIT: Duration = Duration::from_secs(600);
const M34: usize = 0;
const MAC_LATENCY: Duration = Duration::from_millis(30);

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// The 34": the Mac (A) over HDMI, the PC (B) over DP, both with a DDC channel.
fn specs() -> Vec<MonitorSpec> {
    vec![MonitorSpec {
        config: FakeMonitorConfig::new(Personality::Standard),
        cables: vec![(Machine::A, MAC_LATENCY), (Machine::B, ms(5))],
        via: vec![
            (Machine::A, MonitorInput::Hdmi),
            (Machine::B, MonitorInput::DisplayPort),
        ],
    }]
}

struct Setup {
    d: Desk,
    m34: String,
    hotplug: [FakeDisplayChanges; 2],
}

impl Setup {
    fn fire(&self, m: Machine) {
        self.hotplug[usize::from(m == Machine::B)].fire();
    }
}

/// Both on, paired and following display changes, the 34" introduced and showing the PC.
async fn setup(silent: bool) -> Setup {
    let d = Desk::builder().monitors(specs()).build().await.unwrap();
    let sim = d.monitors().unwrap().clone();
    if silent {
        for m in Machine::ALL {
            sim.set_ddc(M34, m, false);
        }
    }
    let hotplug = [FakeDisplayChanges::new(), FakeDisplayChanges::new()];
    for (m, fake) in Machine::ALL.into_iter().zip(&hotplug) {
        d.agent(m).monitor_hub().watch_displays(fake).unwrap();
    }
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    let m34 = sim.id(M34).to_owned();
    Setup { d, m34, hotplug }
}

fn link_of<'a>(views: &'a [MonitorView], id: &str) -> Option<&'a MonitorView> {
    views.iter().find(|v| v.id == id)
}

async fn until(s: &Setup, m: Machine, what: &'static str, ok: impl Fn(&MonitorView) -> bool) {
    s.d.wait_monitors(m, what, |v| link_of(v, &s.m34).is_some_and(&ok))
        .await
        .unwrap_or_else(|e| panic!("{m}: {what}: {e:?}; {:#?}", s.d.agent(m).monitors()));
}

fn own(s: &Setup, m: Machine) -> Option<CableState> {
    s.d.agent(m)
        .monitor_hub()
        .own_cables()
        .into_iter()
        .find(|c| c.monitor == s.m34)
        .map(|c| c.state)
}

async fn own_until(s: &Setup, m: Machine, want: CableState) {
    tokio::time::timeout(LIMIT, async {
        while own(s, m) != Some(want) {
            tokio::time::sleep(ms(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{m}: own end {:?}, {want:?} expected", own(s, m)));
}

/// T-349: a simulated display change rescans within 1 s and the new cable state is in the other
/// agent's UI state within 1.5 s; while the Mac's cable is unplugged and the monitor shows its
/// input, switching to it (selector or scene) is refused with no DDC write; plugging it back
/// clears it.
#[tokio::test(start_paused = true)]
async fn a_display_change_rescans_at_once_and_reaches_the_other_ui() {
    let s = setup(false).await;
    let sim = s.d.monitors().unwrap().clone();
    let h = sim.handle(M34);
    for m in Machine::ALL {
        until(&s, m, "both cables connected, showing the PC", |v| {
            v.link.cables.a == CableState::Connected
                && v.link.cables.b == CableState::Connected
                && v.link.state == LinkState::ShowsMachine(Machine::B)
                && v.link.power == PowerState::On
        })
        .await;
    }
    // The monitor menu puts it on the Mac's input.
    sim.fake().osd_switch(h, Input::Hdmi);
    for m in Machine::ALL {
        until(&s, m, "showing the Mac", |v| {
            v.link.state == LinkState::ShowsMachine(Machine::A)
        })
        .await;
    }
    // Right after a periodic scan, so only the display change can explain a fast rescan.
    tokio::time::sleep(ms(2100)).await;

    let t0 = Instant::now();
    sim.set_cable(M34, Machine::A, None);
    s.fire(Machine::A);
    own_until(&s, Machine::A, CableState::Disconnected).await;
    assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
    until(&s, Machine::B, "the Mac's cable unplugged", |v| {
        v.link.cables.a == CableState::Disconnected
            && v.link.state == LinkState::ActiveInputUnplugged(Machine::A)
    })
    .await;
    assert!(t0.elapsed() < ms(1500), "{:?}", t0.elapsed());

    // Refused before any DDC write: the selector and a scene that puts the 34" on the Mac.
    sim.clear_ops();
    for on in Machine::ALL {
        let e =
            s.d.switch_monitor(on, &s.m34, MonitorInput::Hdmi)
                .await
                .unwrap_err();
        assert!(
            matches!(&e, SimError::Rejected(why) if why.contains("unplugged")),
            "{e:?}"
        );
        let e = s.d.apply_scene(on, "only-a").await.unwrap_err();
        assert!(
            matches!(&e, SimError::Rejected(why) if why.contains("unplugged")),
            "{e:?}"
        );
    }
    assert!(sim.sets_to(h).is_empty(), "{:?}", sim.sets_to(h));

    // Plugged back: connected again, showing the Mac.
    sim.set_cable(M34, Machine::A, Some(MAC_LATENCY));
    s.fire(Machine::A);
    for m in Machine::ALL {
        until(&s, m, "the Mac's cable back", |v| {
            v.link.cables.a == CableState::Connected
                && v.link.state == LinkState::ShowsMachine(Machine::A)
        })
        .await;
    }
}

/// Cable safety: a machine that stops listing a monitor while it shows the other machine gives
/// `unknown`, not `disconnected`, and switching to that machine's input is not refused.
#[tokio::test(start_paused = true)]
async fn a_machine_that_stops_listing_a_monitor_showing_the_other_is_unknown() {
    let s = setup(false).await;
    let sim = s.d.monitors().unwrap().clone();
    until(&s, Machine::B, "both connected", |v| {
        v.link.cables.a == CableState::Connected && v.link.cables.b == CableState::Connected
    })
    .await;
    sim.set_cable(M34, Machine::A, None);
    s.fire(Machine::A);
    // The rescan dropped the monitor from the Mac's list.
    tokio::time::timeout(LIMIT, async {
        while s
            .d
            .agent(Machine::A)
            .monitor_hub()
            .local_channels()
            .iter()
            .any(|c| c.id == s.m34)
        {
            tokio::time::sleep(ms(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(ms(500)).await;
    assert_eq!(own(&s, Machine::A), Some(CableState::Unknown));
    until(&s, Machine::B, "the Mac's end unknown", |v| {
        v.link.cables.a == CableState::Unknown
            && v.link.state == LinkState::ShowsMachine(Machine::B)
    })
    .await;
    match s
        .d
        .switch_monitor(Machine::B, &s.m34, MonitorInput::Hdmi)
        .await
    {
        Err(SimError::Rejected(why)) => assert!(!why.contains("unplugged"), "{why}"),
        Err(e) => panic!("{e:?}"),
        Ok(_) => {}
    }
}

/// Debounce: a monitor whose channels stop answering is not "off" until `OFF_AFTER`, and is "on"
/// again when they answer.
#[tokio::test(start_paused = true)]
async fn a_silent_monitor_is_off_only_after_the_timeout() {
    let s = setup(false).await;
    let sim = s.d.monitors().unwrap().clone();
    for m in Machine::ALL {
        until(&s, m, "on, showing the PC, read by the PC", |v| {
            v.link.power == PowerState::On
                && v.link.state == LinkState::ShowsMachine(Machine::B)
                && v.owner == Some(Machine::B)
                && v.report.is_some()
        })
        .await;
    }
    // A few readings of the owner, so its channel is the one that falls silent.
    tokio::time::sleep(ms(1500)).await;
    let t0 = Instant::now();
    for m in Machine::ALL {
        sim.set_ddc(M34, m, false);
    }
    tokio::time::sleep(OFF_AFTER / 2).await;
    for m in Machine::ALL {
        let v = s.d.agent(m).monitors();
        let link = link_of(&v, &s.m34).unwrap().link;
        assert_ne!(link.power, PowerState::Off, "{m}: {link:?}");
    }
    for m in Machine::ALL {
        until(&s, m, "off", |v| v.link.state == LinkState::Off).await;
    }
    assert!(t0.elapsed() >= OFF_AFTER, "{:?}", t0.elapsed());
    for m in Machine::ALL {
        sim.set_ddc(M34, m, true);
    }
    s.fire(Machine::B);
    for m in Machine::ALL {
        until(&s, m, "back on", |v| {
            v.link.power == PowerState::On && v.link.state == LinkState::ShowsMachine(Machine::B)
        })
        .await;
    }
}

/// A monitor whose ends are all `NoChannel` (DDC/CI off in its menu) stays unknown, never off.
#[tokio::test(start_paused = true)]
async fn a_monitor_without_any_channel_is_unknown_never_off() {
    let s = setup(true).await;
    for m in Machine::ALL {
        until(&s, m, "seen by both, without a channel", |v| {
            v.seen_by.len() == 2 && v.link.cables.a == CableState::Connected
        })
        .await;
    }
    tokio::time::sleep(OFF_AFTER * 6).await;
    for m in Machine::ALL {
        let v = s.d.agent(m).monitors();
        let link = link_of(&v, &s.m34).unwrap().link;
        assert_eq!(link.power, PowerState::Unknown, "{m}: {link:?}");
        assert_eq!(link.state, LinkState::Unknown, "{m}: {link:?}");
    }
}
