//! Las dos máquinas con detectores simulados: mover el hub invierte los roles
//! después del antirrebote, y el selector manual gana sobre la detección.

use std::time::Duration;

use tandem_core::Machine;
use tandem_peripherals::fake::{FakeDevices, FakeScanner};
use tandem_peripherals::{
    HubConfig, HubWatcher, Presence, Role, RoleArbiter, Selector, UsbId, WatchConfig,
};
use tokio::sync::watch;

const KB: UsbId = UsbId::new(0x046d, 0xc33f);
const MOUSE: UsbId = UsbId::new(0x046d, 0xc539);

fn cfg() -> HubConfig {
    HubConfig {
        keyboard: KB,
        mouse: MOUSE,
    }
}

/// Un agente: su escáner simulado, su sondeo y su árbitro. El reporte al otro
/// agente es instantáneo (en la realidad tarda milisegundos frente a 3 s).
struct Agent {
    devices: FakeDevices,
    presence: watch::Receiver<Presence>,
    arbiter: RoleArbiter,
}

impl Agent {
    fn start(me: Machine, server: Machine, connected: &[UsbId]) -> Self {
        let scanner = FakeScanner::new(connected.to_vec());
        let devices = scanner.handle();
        let watcher = HubWatcher::new(scanner, cfg(), WatchConfig::default());
        let presence = watcher.subscribe();
        tokio::spawn(watcher.run());
        Agent {
            devices,
            presence,
            arbiter: RoleArbiter::new(me, server),
        }
    }
}

struct Desk {
    mac: Agent,
    pc: Agent,
}

impl Desk {
    /// Lleva lo que publicó cada sondeo a los dos árbitros.
    fn sync(&mut self) {
        let mac = self.mac.presence.borrow_and_update().hub();
        let pc = self.pc.presence.borrow_and_update().hub();
        self.mac.arbiter.set_local_hub(mac);
        self.mac.arbiter.set_peer_hub(Some(pc));
        self.pc.arbiter.set_local_hub(pc);
        self.pc.arbiter.set_peer_hub(Some(mac));
    }

    async fn run_for(&mut self, d: Duration) {
        let step = Duration::from_millis(100);
        let mut left = d;
        while left > Duration::ZERO {
            tokio::time::advance(step.min(left)).await;
            tokio::task::yield_now().await;
            self.sync();
            left = left.saturating_sub(step);
        }
    }

    fn server(&self) -> Machine {
        assert_eq!(
            self.mac.arbiter.server(),
            self.pc.arbiter.server(),
            "las dos máquinas no coinciden"
        );
        self.mac.arbiter.server()
    }

    fn select(&mut self, s: Selector) {
        self.mac.arbiter.set_selector(s);
        self.pc.arbiter.set_selector(s);
    }

    fn move_hub_to(&self, m: Machine) {
        let (to, from) = match m {
            Machine::Mac => (&self.mac, &self.pc),
            Machine::Pc => (&self.pc, &self.mac),
        };
        from.devices.set(vec![]);
        to.devices.set(vec![KB, MOUSE]);
    }
}

fn desk(hub_on: Machine) -> Desk {
    let on = |m: Machine| if m == hub_on { vec![KB, MOUSE] } else { vec![] };
    let mut d = Desk {
        mac: Agent::start(Machine::Mac, hub_on, &on(Machine::Mac)),
        pc: Agent::start(Machine::Pc, hub_on, &on(Machine::Pc)),
    };
    d.sync();
    d
}

#[tokio::test(start_paused = true)]
async fn moving_the_hub_inverts_roles_after_debounce() {
    let mut d = desk(Machine::Mac);
    assert_eq!(d.server(), Machine::Mac);
    assert_eq!(d.mac.arbiter.role(), Role::Server);

    d.move_hub_to(Machine::Pc);
    d.run_for(Duration::from_millis(2_500)).await;
    assert_eq!(d.server(), Machine::Mac, "invirtió antes de 3 s");
    d.run_for(Duration::from_secs(1)).await;
    assert_eq!(d.server(), Machine::Pc);
    assert_eq!(d.pc.arbiter.role(), Role::Server);
    assert_eq!(d.mac.arbiter.role(), Role::Client);
}

#[tokio::test(start_paused = true)]
async fn brief_disconnection_does_not_invert_roles() {
    let mut d = desk(Machine::Pc);
    // El cable del hub falla 2 s y vuelve.
    d.pc.devices.set(vec![]);
    d.run_for(Duration::from_secs(2)).await;
    d.pc.devices.set(vec![KB, MOUSE]);
    d.run_for(Duration::from_secs(10)).await;
    assert_eq!(d.server(), Machine::Pc);

    // Un ir y venir rápido a la Mac (menos de 3 s) tampoco.
    d.move_hub_to(Machine::Mac);
    d.run_for(Duration::from_secs(2)).await;
    d.move_hub_to(Machine::Pc);
    d.run_for(Duration::from_secs(10)).await;
    assert_eq!(d.server(), Machine::Pc);
}

#[tokio::test(start_paused = true)]
async fn manual_selector_wins_over_detection() {
    let mut d = desk(Machine::Mac);
    d.select(Selector::Manual(Machine::Pc));
    assert_eq!(d.server(), Machine::Pc);

    // El hub va y viene; manda el selector.
    d.move_hub_to(Machine::Pc);
    d.run_for(Duration::from_secs(5)).await;
    d.move_hub_to(Machine::Mac);
    d.run_for(Duration::from_secs(5)).await;
    assert_eq!(d.server(), Machine::Pc);

    // En automático vuelve a mandar el hub, que está en la Mac.
    d.select(Selector::Auto);
    assert_eq!(d.server(), Machine::Mac);
}
