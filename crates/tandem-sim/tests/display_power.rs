//! No phantom display on the Mac (T-341), with two real agents, the `monitor-ddc` doubles and a
//! display double on the Mac (A, the 34" over HDMI). While the 34" shows the PC (B, over DP) the
//! Mac turns its HDMI display off; a switch back to the Mac turns it on before the monitor
//! changes input, whichever machine asks for it; at exit it goes back on.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use monitor_ddc::Input;
use monitor_ddc::fake::{FakeMonitorConfig, Personality, test_edid};
use tandem_core::{DisplayKey, Machine, MonitorInput};
use tandem_input::displays::{
    DisplayControl, DisplayPower, DisplayPowerError, FakeDisplayPower, NoDisplayPower, PowerDisplay,
};
use tandem_sim::{Desk, MonitorSpec, Outcome, SimMonitors};
use tandemd::displays::DisplayGuard;

const LIMIT: Duration = Duration::from_secs(600);
const M34: usize = 0;
const BUILTIN: u32 = 1;
const HDMI: u32 = 7;

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

/// The Mac's displays: each time one goes back on, the input the 34" shows at that moment.
#[derive(Debug)]
struct MacDisplays {
    fake: FakeDisplayPower,
    sim: SimMonitors,
    on_while: Mutex<Vec<Option<Input>>>,
}

impl MacDisplays {
    fn new(sim: SimMonitors) -> Self {
        MacDisplays {
            fake: FakeDisplayPower::new([
                PowerDisplay {
                    id: BUILTIN,
                    key: Some(builtin()),
                },
                PowerDisplay {
                    id: HDMI,
                    key: Some(key34()),
                },
            ]),
            sim,
            on_while: Mutex::new(Vec::new()),
        }
    }

    fn woken_while(&self) -> Vec<Option<Input>> {
        self.on_while.lock().unwrap().clone()
    }
}

impl DisplayPower for MacDisplays {
    fn control(&self) -> DisplayControl {
        self.fake.control()
    }
    fn active(&self) -> Result<Vec<PowerDisplay>, DisplayPowerError> {
        self.fake.active()
    }
    fn set_enabled(&self, id: u32, on: bool) -> Result<(), DisplayPowerError> {
        if on {
            let shown = self.sim.fake().screen_input(self.sim.handle(M34));
            self.on_while.lock().unwrap().push(shown);
        }
        self.fake.set_enabled(id, on)
    }
}

/// Both on and paired, the PC owning the 34" (the Mac's HDMI is silent), and display control
/// on both: the Mac with its double, the PC with nothing to turn off.
async fn desk() -> (Desk, String, Arc<MacDisplays>) {
    let d = Desk::builder().monitors(specs()).build().await.unwrap();
    let sim = d.monitors().unwrap().clone();
    sim.set_ddc(M34, Machine::A, false);
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    let m34 = sim.id(M34).to_owned();
    for m in Machine::ALL {
        let mut desk = d.agent(m).subscribe_desk();
        tokio::time::timeout(LIMIT, desk.wait_for(|p| p.cable(&m34).is_some()))
            .await
            .unwrap()
            .unwrap();
    }
    let mac = Arc::new(MacDisplays::new(sim));
    drop(
        d.agent(Machine::A)
            .start_displays(Arc::new(DisplayGuard::new(mac.clone(), None))),
    );
    drop(
        d.agent(Machine::B)
            .start_displays(Arc::new(DisplayGuard::new(Arc::new(NoDisplayPower), None))),
    );
    (d, m34, mac)
}

async fn hdmi_until(mac: &MacDisplays, on: bool) {
    tokio::time::timeout(LIMIT, async {
        while mac.fake.is_on(HDMI) != on {
            tokio::time::sleep(ms(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the Mac's HDMI display never went {on}"));
}

fn on_screen(d: &Desk) -> Option<Input> {
    let sim = d.monitors().unwrap();
    sim.fake().screen_input(sim.handle(M34))
}

#[tokio::test(start_paused = true)]
async fn the_mac_display_is_off_while_the_monitor_shows_the_pc() {
    let (d, m34, mac) = desk().await;

    // The 34" shows the PC: the Mac's HDMI display goes off, the built-in one stays.
    hdmi_until(&mac, false).await;
    assert!(mac.fake.is_on(BUILTIN));
    assert_eq!(on_screen(&d), Some(Input::DisplayPort));

    // The PC gives it to the Mac: the Mac turns its display on while the 34" still shows DP.
    let run = d
        .switch_monitor(Machine::B, &m34, MonitorInput::Hdmi)
        .await
        .unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert_eq!(on_screen(&d), Some(Input::Hdmi));
    assert!(mac.fake.is_on(HDMI));
    assert_eq!(mac.woken_while(), [Some(Input::DisplayPort)]);
    // It stays on: the 34" shows the Mac.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(mac.fake.is_on(HDMI));

    // Back to the PC, asked from the Mac: off again.
    let run = d
        .switch_monitor(Machine::A, &m34, MonitorInput::DisplayPort)
        .await
        .unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    hdmi_until(&mac, false).await;

    // The Mac asks for it itself: the same order, without asking the PC.
    let run = d
        .switch_monitor(Machine::A, &m34, MonitorInput::Hdmi)
        .await
        .unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert_eq!(
        mac.woken_while(),
        [Some(Input::DisplayPort), Some(Input::DisplayPort)]
    );
}

/// "Only B" moves the 34" to the PC and the display goes off; at exit it goes back on.
#[tokio::test(start_paused = true)]
async fn a_scene_turns_it_off_and_exit_turns_it_back_on() {
    let (d, m34, mac) = desk().await;
    hdmi_until(&mac, false).await;
    let run = d.apply_scene(Machine::B, "only-a").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert_eq!(mac.woken_while(), [Some(Input::DisplayPort)]);
    assert!(mac.fake.is_on(HDMI));

    let run = d.apply_scene(Machine::A, "only-b").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    hdmi_until(&mac, false).await;
    let guard = d.agent(Machine::A).display_guard().unwrap();
    assert_eq!(guard.off(), [m34]);

    d.agent(Machine::A).restore_displays();
    assert!(mac.fake.is_on(HDMI));
    assert!(guard.off().is_empty());
}
