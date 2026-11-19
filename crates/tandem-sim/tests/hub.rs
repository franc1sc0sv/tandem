//! Hub switch over the simulated desk (QA round 2): each machine's probe
//! reads the USB devices it really sees (none with the PC off), and the
//! role arbiters receive the other's report only while there is a link.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::Machine;
use tandem_link::manager::LinkKind;
use tandem_peripherals::{
    DeviceScanner, HubConfig, HubWatcher, Presence, Role, RoleArbiter, UsbId, WatchConfig,
};
use tandem_sim::world::World;
use tandem_sim::{Desk, Outcome, Power};
use tokio::sync::watch;

/// The keyboard and mouse receiver that the simulated desk's hub carries.
const RECEIVER: UsbId = UsbId::new(0x046D, 0xC548);

struct WorldScanner {
    world: World,
    machine: Machine,
}

impl DeviceScanner for WorldScanner {
    type Error = std::convert::Infallible;

    fn scan(&mut self) -> Result<Vec<UsbId>, Self::Error> {
        Ok(self
            .world
            .usb_devices(self.machine)
            .into_iter()
            .map(|d| UsbId::new(d.vid, d.pid))
            .collect())
    }
}

/// The two arbiters, fed every 100 ms as each agent would.
///
/// The same for both machines (DESIGN §3.7): the one that is off runs nothing and, on
/// coming back, starts from zero with the last thing it saved; the one that is on, without a link,
/// considers the other off only when its agent says so (`peer_watch`).
#[derive(Clone)]
struct Roles(Arc<Mutex<[RoleArbiter; 2]>>);

const BOTH: [Machine; 2] = [Machine::A, Machine::B];

fn index(m: Machine) -> usize {
    match m {
        Machine::A => 0,
        Machine::B => 1,
    }
}

impl Roles {
    fn start(d: &Desk) -> Self {
        let cfg = HubConfig {
            keyboard: RECEIVER,
            mouse: RECEIVER,
        };
        let watch = |machine| {
            let w = HubWatcher::new(
                WorldScanner {
                    world: d.world.clone(),
                    machine,
                },
                cfg.clone(),
                WatchConfig::default(),
            );
            let rx = w.subscribe();
            tokio::spawn(w.run());
            rx
        };
        let presence: [watch::Receiver<Presence>; 2] = [watch(Machine::A), watch(Machine::B)];
        // At startup, the keyboard is on the main machine (that is where the hub is).
        let first = d.primary();
        let roles = Roles(Arc::new(Mutex::new([
            RoleArbiter::new(Machine::A, first),
            RoleArbiter::new(Machine::B, first),
        ])));
        let (world, net, agents, r) = (
            d.world.clone(),
            d.net.clone(),
            [d.mac.clone(), d.pc.clone()],
            roles.clone(),
        );
        tokio::spawn(async move {
            let mut was_on = BOTH.map(|m| world.power_of(m) == Power::On);
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let on = BOTH.map(|m| world.power_of(m) == Power::On);
                let connected =
                    on.iter().all(|o| *o) && LinkKind::ALL.iter().any(|k| net.is_up(*k));
                let mut a = r.0.lock().unwrap_or_else(PoisonError::into_inner);
                for m in BOTH {
                    let i = index(m);
                    // Off, its agent does not run; on startup it starts from zero with the
                    // last keyboard location it saved.
                    if on[i] && !was_on[i] {
                        let saved = a[i].server();
                        a[i] = RoleArbiter::new(m, saved);
                    }
                    if on[i] {
                        a[i].set_local_hub(presence[i].borrow().hub());
                    }
                }
                was_on = on;
                if connected {
                    let (to_mac, to_pc) = (a[1].report(), a[0].report());
                    a[0].set_peer(to_mac);
                    a[1].set_peer(to_pc);
                } else {
                    for m in BOTH {
                        let i = index(m);
                        if !on[i] {
                            continue;
                        }
                        // It does not know whether the other is still on: only what its agent believes.
                        if agents[i].state().is_on(m.other()) {
                            a[i].set_peer(None);
                        } else {
                            a[i].set_peer_off();
                        }
                    }
                }
            }
        });
        roles
    }

    /// Where the keyboard is according to `from` and, if the other is on, also according to it.
    fn keyboard_from(&self, from: Machine, both_on: bool) -> Machine {
        let a = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if both_on {
            assert_eq!(
                a[0].server(),
                a[1].server(),
                "the two machines do not agree"
            );
        }
        a[index(from)].server()
    }

    /// Seen from the Mac (the usual scenarios).
    fn server(&self, pc_on: bool) -> Machine {
        self.keyboard_from(Machine::A, pc_on)
    }

    /// The keyboard and mouse chip that each machine's UI shows: the role
    /// its arbiter gives it.
    fn chip(&self, on: Machine) -> Role {
        let a = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        a[index(on)].role()
    }
}

#[tokio::test(start_paused = true)]
async fn move_the_hub_with_the_whole_desk() {
    let d = Desk::new().await.unwrap();
    let roles = Roles::start(&d);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(false), Machine::A);

    // The PC starts: it does not see the hub and the Mac stays as the server.
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed));
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(true), Machine::A);

    // The hub moves to the PC: at 2.5 s nothing, past 3 s they swap.
    d.world.plug_hub(Machine::B);
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert_eq!(roles.server(true), Machine::A);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(roles.server(true), Machine::B);

    // A 1 s tug toward the Mac changes nothing.
    d.world.plug_hub(Machine::A);
    tokio::time::sleep(Duration::from_secs(1)).await;
    d.world.plug_hub(Machine::B);
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(roles.server(true), Machine::B);

    // With USB4 and Ethernet down, the report continues over Wi-Fi.
    d.net.cut(LinkKind::Usb4);
    d.net.cut(LinkKind::Ethernet);
    d.world.plug_hub(Machine::A);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(true), Machine::A);
    d.net.restore(LinkKind::Usb4);
    d.net.restore(LinkKind::Ethernet);

    // Hub on the PC and the PC shuts down: the Mac considers it off and, with the hub
    // now on it, is the server without waiting for the PC (T-066).
    d.world.plug_hub(Machine::B);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(true), Machine::B);
    let run = d.shutdown_pc().await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed));
    d.world.plug_hub(Machine::A);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(d.world.power_of(Machine::B), Power::Off);
    assert_eq!(roles.server(false), Machine::A);
    assert_eq!(roles.chip(Machine::A), Role::Server);

    // The PC comes back, does not have the hub and becomes a client.
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed));
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(true), Machine::A);
    assert_eq!(roles.chip(Machine::B), Role::Client);
}

/// QA round 2: the PC loses power with the hub connected and the hub moves to the
/// Mac. While the Mac does not consider the PC off nothing changes; past the threshold of
/// T-063, the Mac is the server and its UI shows it. When the PC comes back (with the
/// server it had saved, the PC) the roles converge as usual.
#[tokio::test(start_paused = true)]
async fn the_pc_shuts_down_with_the_hub() {
    let d = Desk::new().await.unwrap();
    let roles = Roles::start(&d);
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed));
    d.world.plug_hub(Machine::B);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(true), Machine::B);
    assert_eq!(roles.chip(Machine::A), Role::Client);

    let lost_after = d.mac.config().power.peer_lost_after();
    d.pull_pc_plug();
    d.world.plug_hub(Machine::A);
    // Within the threshold the Mac does not yet know whether the PC is still there.
    tokio::time::sleep(lost_after - Duration::from_secs(1)).await;
    assert!(d.mac.state().is_on(Machine::B));
    assert_eq!(roles.server(false), Machine::B);

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!d.mac.state().is_on(Machine::B));
    assert_eq!(roles.server(false), Machine::A);
    assert_eq!(roles.chip(Machine::A), Role::Server);

    // The PC comes back with its button: the hub stays on the Mac and the PC becomes a client.
    d.press_pc_power_button();
    let mut states = d.mac.subscribe();
    tokio::time::timeout(
        Duration::from_secs(3600),
        states.wait_for(|s| s.is_on(Machine::B)),
    )
    .await
    .unwrap()
    .unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(true), Machine::A);
    assert_eq!(roles.chip(Machine::B), Role::Client);

    // And moving the hub to the PC swaps the roles just as before.
    d.world.plug_hub(Machine::B);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(true), Machine::B);
    assert_eq!(roles.chip(Machine::A), Role::Client);
}

// The same scenarios with the PC as main: the user works on the PC and the Mac
// is suspended or off (DESIGN §3.7). Nothing changes except the names.

/// Mirror of [`move_the_hub_with_the_whole_desk`].
#[tokio::test(start_paused = true)]
async fn mirror_move_the_hub_with_the_pc_as_main() {
    let d = Desk::builder().primary(Machine::B).build().await.unwrap();
    let roles = Roles::start(&d);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.keyboard_from(Machine::B, false), Machine::B);
    assert_eq!(roles.chip(Machine::B), Role::Server);

    // The Mac wakes up: it does not see the hub and the keyboard stays on the PC.
    let run = d.power_on(Machine::A).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.keyboard_from(Machine::B, true), Machine::B);
    assert_eq!(roles.chip(Machine::A), Role::Client);

    // The hub moves to the Mac: past 3 s they swap, on both.
    d.world.plug_hub(Machine::A);
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert_eq!(roles.keyboard_from(Machine::B, true), Machine::B);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(roles.keyboard_from(Machine::B, true), Machine::A);

    // Hub on the Mac and the PC shuts it down: the PC keeps everything and, with the hub now on
    // it, has the keyboard without waiting for the Mac.
    let run = d.shutdown(Machine::B, Machine::A).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    d.world.plug_hub(Machine::B);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(d.world.power_of(Machine::A), Power::Off);
    assert_eq!(roles.keyboard_from(Machine::B, false), Machine::B);
    assert_eq!(roles.chip(Machine::B), Role::Server);
}

/// Mirror of [`the_pc_shuts_down_with_the_hub`]: the Mac suspends with the hub connected and
/// the hub moves to the PC.
#[tokio::test(start_paused = true)]
async fn mirror_the_mac_suspends_with_the_hub() {
    let d = Desk::builder().primary(Machine::B).build().await.unwrap();
    let roles = Roles::start(&d);
    let run = d.power_on(Machine::A).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    d.world.plug_hub(Machine::A);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.keyboard_from(Machine::B, true), Machine::A);
    assert_eq!(roles.chip(Machine::B), Role::Client);

    let lost_after = d.pc.config().power.peer_lost_after();
    d.pull_plug(Machine::A);
    d.world.plug_hub(Machine::B);
    tokio::time::sleep(lost_after - Duration::from_secs(1)).await;
    assert!(d.pc.state().is_on(Machine::A));
    assert_eq!(roles.keyboard_from(Machine::B, false), Machine::A);

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!d.pc.state().is_on(Machine::A));
    assert!(
        d.pc.state().is_on(Machine::B),
        "the PC never considers itself off"
    );
    assert_eq!(roles.keyboard_from(Machine::B, false), Machine::B);
    assert_eq!(roles.chip(Machine::B), Role::Server);

    // The Mac comes back with its button: the hub stays on the PC and the Mac receives the keyboard.
    d.press_power_button(Machine::A);
    let mut states = d.pc.subscribe();
    tokio::time::timeout(
        Duration::from_secs(3600),
        states.wait_for(|s| s.is_on(Machine::A)),
    )
    .await
    .unwrap()
    .unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.keyboard_from(Machine::B, true), Machine::B);
    assert_eq!(roles.chip(Machine::A), Role::Client);
}
