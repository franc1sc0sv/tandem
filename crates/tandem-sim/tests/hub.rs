//! Cambio de hub sobre el puesto simulado (QA ronda 2): el sondeo de cada máquina
//! lee los dispositivos USB que de verdad ve (ninguno con la PC apagada), y los
//! árbitros de roles reciben el reporte del otro solo mientras hay enlace.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::{Machine, Mode};
use tandem_link::manager::LinkKind;
use tandem_peripherals::{
    DeviceScanner, HubConfig, HubWatcher, Presence, Role, RoleArbiter, UsbId, WatchConfig,
};
use tandem_sim::world::World;
use tandem_sim::{Desk, Outcome, Power};
use tokio::sync::watch;

/// El receptor del teclado y mouse que trae el hub del puesto simulado.
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

/// Los dos árbitros, alimentados cada 100 ms como lo haría cada agente.
///
/// Igual para las dos máquinas (DESIGN §3.7): la que está apagada no corre nada y, al
/// volver, empieza de cero con lo último que guardó; la que está prendida, sin enlace,
/// da a la otra por apagada solo cuando su agente lo dice (`peer_watch`).
#[derive(Clone)]
struct Roles(Arc<Mutex<[RoleArbiter; 2]>>);

const BOTH: [Machine; 2] = [Machine::Mac, Machine::Pc];

fn index(m: Machine) -> usize {
    match m {
        Machine::Mac => 0,
        Machine::Pc => 1,
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
        let presence: [watch::Receiver<Presence>; 2] = [watch(Machine::Mac), watch(Machine::Pc)];
        // Al arrancar, el teclado está en la principal (es donde está el hub).
        let first = d.principal();
        let roles = Roles(Arc::new(Mutex::new([
            RoleArbiter::new(Machine::Mac, first),
            RoleArbiter::new(Machine::Pc, first),
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
                    // Apagada, su agente no corre; al arrancar empieza de cero con el
                    // último lugar del teclado que guardó.
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
                        // No sabe si la otra sigue prendida: lo que cree su agente.
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

    /// Dónde está el teclado según `from` y, si la otra está prendida, también según ella.
    fn keyboard_from(&self, from: Machine, both_on: bool) -> Machine {
        let a = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if both_on {
            assert_eq!(
                a[0].server(),
                a[1].server(),
                "las dos máquinas no coinciden"
            );
        }
        a[index(from)].server()
    }

    /// Visto desde la Mac (los escenarios de siempre).
    fn server(&self, pc_on: bool) -> Machine {
        self.keyboard_from(Machine::Mac, pc_on)
    }

    /// El chip de teclado y mouse que muestra la UI de cada máquina: el papel que
    /// le da su árbitro.
    fn chip(&self, on: Machine) -> Role {
        let a = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        a[index(on)].role()
    }
}

#[tokio::test(start_paused = true)]
async fn mover_el_hub_con_el_puesto_completo() {
    let d = Desk::new().await.unwrap();
    let roles = Roles::start(&d);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(false), Machine::Mac);

    // Arranca la PC: no ve el hub y la Mac sigue de servidor.
    let run = d.set_mode(Machine::Mac, Mode::SoloPc).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed));
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(true), Machine::Mac);

    // El hub pasa a la PC: a los 2,5 s nada, pasados los 3 s se invierten.
    d.world.plug_hub(Machine::Pc);
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert_eq!(roles.server(true), Machine::Mac);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(roles.server(true), Machine::Pc);

    // Un tirón de 1 s hacia la Mac no cambia nada.
    d.world.plug_hub(Machine::Mac);
    tokio::time::sleep(Duration::from_secs(1)).await;
    d.world.plug_hub(Machine::Pc);
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(roles.server(true), Machine::Pc);

    // Con el USB4 y Ethernet caídos sigue el reporte por Wi-Fi.
    d.net.cut(LinkKind::Usb4);
    d.net.cut(LinkKind::Ethernet);
    d.world.plug_hub(Machine::Mac);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(true), Machine::Mac);
    d.net.restore(LinkKind::Usb4);
    d.net.restore(LinkKind::Ethernet);

    // Hub en la PC y la PC se apaga: la Mac la da por apagada y, con el hub
    // ahora en ella, es el servidor sin esperar a la PC (T-066).
    d.world.plug_hub(Machine::Pc);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(true), Machine::Pc);
    let run = d.shutdown_pc().await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed));
    d.world.plug_hub(Machine::Mac);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(d.world.power_of(Machine::Pc), Power::Off);
    assert_eq!(roles.server(false), Machine::Mac);
    assert_eq!(roles.chip(Machine::Mac), Role::Server);

    // La PC vuelve, no tiene el hub y se pone de cliente.
    let run = d
        .set_mode(
            Machine::Mac,
            Mode::Shared([("m34".into(), Machine::Pc)].into()),
        )
        .await
        .unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed));
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(true), Machine::Mac);
    assert_eq!(roles.chip(Machine::Pc), Role::Client);
}

/// QA ronda 2: la PC pierde la corriente con el hub conectado y el hub pasa a la
/// Mac. Mientras la Mac no da la PC por apagada nada cambia; pasado el umbral de
/// T-063, la Mac es el servidor y su UI lo muestra. Al volver la PC (con el
/// servidor que tenía guardado, la PC) los roles convergen como siempre.
#[tokio::test(start_paused = true)]
async fn la_pc_se_apaga_con_el_hub() {
    let d = Desk::new().await.unwrap();
    let roles = Roles::start(&d);
    let run = d
        .set_mode(
            Machine::Mac,
            Mode::Shared([("m34".into(), Machine::Pc)].into()),
        )
        .await
        .unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed));
    d.world.plug_hub(Machine::Pc);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(true), Machine::Pc);
    assert_eq!(roles.chip(Machine::Mac), Role::Client);

    let lost_after = d.mac.config().power.peer_lost_after();
    d.pull_pc_plug();
    d.world.plug_hub(Machine::Mac);
    // Dentro del umbral la Mac todavía no sabe si la PC sigue ahí.
    tokio::time::sleep(lost_after - Duration::from_secs(1)).await;
    assert!(d.mac.state().is_on(Machine::Pc));
    assert_eq!(roles.server(false), Machine::Pc);

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!d.mac.state().is_on(Machine::Pc));
    assert_eq!(roles.server(false), Machine::Mac);
    assert_eq!(roles.chip(Machine::Mac), Role::Server);

    // La PC vuelve con su botón: el hub sigue en la Mac y la PC se pone de cliente.
    d.press_pc_power_button();
    let mut states = d.mac.subscribe();
    tokio::time::timeout(
        Duration::from_secs(3600),
        states.wait_for(|s| s.is_on(Machine::Pc)),
    )
    .await
    .unwrap()
    .unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(true), Machine::Mac);
    assert_eq!(roles.chip(Machine::Pc), Role::Client);

    // Y mover el hub a la PC invierte los roles igual que antes.
    d.world.plug_hub(Machine::Pc);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(true), Machine::Pc);
    assert_eq!(roles.chip(Machine::Mac), Role::Client);
}

// Los mismos escenarios con la PC de principal: el usuario trabaja en la PC y la Mac
// está suspendida o apagada (DESIGN §3.7). Nada cambia salvo los nombres.

/// Espejo de [`mover_el_hub_con_el_puesto_completo`].
#[tokio::test(start_paused = true)]
async fn espejo_mover_el_hub_con_la_pc_de_principal() {
    let d = Desk::builder()
        .principal(Machine::Pc)
        .build()
        .await
        .unwrap();
    let roles = Roles::start(&d);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.keyboard_from(Machine::Pc, false), Machine::Pc);
    assert_eq!(roles.chip(Machine::Pc), Role::Server);

    // Se despierta la Mac (un monitor para ella): no ve el hub y el teclado sigue en la PC.
    let run = d
        .set_mode(
            Machine::Pc,
            Mode::Shared([("m32".into(), Machine::Mac)].into()),
        )
        .await
        .unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.keyboard_from(Machine::Pc, true), Machine::Pc);
    assert_eq!(roles.chip(Machine::Mac), Role::Client);

    // El hub pasa a la Mac: pasados los 3 s se invierten, en las dos.
    d.world.plug_hub(Machine::Mac);
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert_eq!(roles.keyboard_from(Machine::Pc, true), Machine::Pc);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(roles.keyboard_from(Machine::Pc, true), Machine::Mac);

    // Hub en la Mac y la PC la apaga: la PC se queda con todo y, con el hub ahora en
    // ella, tiene el teclado sin esperar a la Mac.
    let run = d.shutdown(Machine::Pc, Machine::Mac).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    d.world.plug_hub(Machine::Pc);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(d.world.power_of(Machine::Mac), Power::Off);
    assert_eq!(roles.keyboard_from(Machine::Pc, false), Machine::Pc);
    assert_eq!(roles.chip(Machine::Pc), Role::Server);
}

/// Espejo de [`la_pc_se_apaga_con_el_hub`]: la Mac se suspende con el hub conectado y
/// el hub pasa a la PC.
#[tokio::test(start_paused = true)]
async fn espejo_la_mac_se_suspende_con_el_hub() {
    let d = Desk::builder()
        .principal(Machine::Pc)
        .build()
        .await
        .unwrap();
    let roles = Roles::start(&d);
    let run = d
        .set_mode(
            Machine::Pc,
            Mode::Shared([("m32".into(), Machine::Mac)].into()),
        )
        .await
        .unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    d.world.plug_hub(Machine::Mac);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.keyboard_from(Machine::Pc, true), Machine::Mac);
    assert_eq!(roles.chip(Machine::Pc), Role::Client);

    let lost_after = d.pc.config().power.peer_lost_after();
    d.pull_plug(Machine::Mac);
    d.world.plug_hub(Machine::Pc);
    tokio::time::sleep(lost_after - Duration::from_secs(1)).await;
    assert!(d.pc.state().is_on(Machine::Mac));
    assert_eq!(roles.keyboard_from(Machine::Pc, false), Machine::Mac);

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!d.pc.state().is_on(Machine::Mac));
    assert!(
        d.pc.state().is_on(Machine::Pc),
        "la PC nunca se da por apagada a sí misma"
    );
    assert_eq!(roles.keyboard_from(Machine::Pc, false), Machine::Pc);
    assert_eq!(roles.chip(Machine::Pc), Role::Server);

    // La Mac vuelve con su botón: el hub sigue en la PC y la Mac recibe el teclado.
    d.press_power_button(Machine::Mac);
    let mut states = d.pc.subscribe();
    tokio::time::timeout(
        Duration::from_secs(3600),
        states.wait_for(|s| s.is_on(Machine::Mac)),
    )
    .await
    .unwrap()
    .unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.keyboard_from(Machine::Pc, true), Machine::Pc);
    assert_eq!(roles.chip(Machine::Mac), Role::Client);
}
