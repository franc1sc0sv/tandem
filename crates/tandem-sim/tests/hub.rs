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
#[derive(Clone)]
struct Roles(Arc<Mutex<[RoleArbiter; 2]>>);

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
        let (mac_rx, pc_rx): (watch::Receiver<Presence>, _) =
            (watch(Machine::Mac), watch(Machine::Pc));
        let roles = Roles(Arc::new(Mutex::new([
            RoleArbiter::new(Machine::Mac, Machine::Mac),
            RoleArbiter::new(Machine::Pc, Machine::Mac),
        ])));
        let (world, net, mac_agent, r) =
            (d.world.clone(), d.net.clone(), d.mac.clone(), roles.clone());
        tokio::spawn(async move {
            let mut was_on = world.power() == Power::On;
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let pc_on = world.power() == Power::On;
                let connected = pc_on && LinkKind::ALL.iter().any(|k| net.is_up(*k));
                let (mac, pc) = (mac_rx.borrow().hub(), pc_rx.borrow().hub());
                let mut a = r.0.lock().unwrap_or_else(PoisonError::into_inner);
                // Con la PC apagada su agente no corre; al arrancar empieza de
                // cero con el último servidor que guardó.
                if pc_on && !was_on {
                    let saved = a[1].server();
                    a[1] = RoleArbiter::new(Machine::Pc, saved);
                }
                was_on = pc_on;
                a[0].set_local_hub(mac);
                if pc_on {
                    a[1].set_local_hub(pc);
                }
                if connected {
                    let (to_mac, to_pc) = (a[1].report(), a[0].report());
                    a[0].set_peer(to_mac);
                    a[1].set_peer(to_pc);
                } else {
                    // La Mac no sabe si la PC está prendida: lo que cree su agente (T-063).
                    if mac_agent.state().pc_on {
                        a[0].set_peer(None);
                    } else {
                        a[0].set_peer_off();
                    }
                    if pc_on {
                        a[1].set_peer(None);
                    }
                }
            }
        });
        roles
    }

    /// El servidor según la Mac y, si la PC está prendida, también según la PC.
    fn server(&self, pc_on: bool) -> Machine {
        let a = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if pc_on {
            assert_eq!(
                a[0].server(),
                a[1].server(),
                "las dos máquinas no coinciden"
            );
        }
        a[0].server()
    }

    /// El chip de teclado y mouse que muestra la UI de cada máquina: el papel que
    /// le da su árbitro.
    fn chip(&self, on: Machine) -> Role {
        let a = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match on {
            Machine::Mac => a[0].role(),
            Machine::Pc => a[1].role(),
        }
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
    assert_eq!(d.world.power(), Power::Off);
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

    let lost_after = d.mac.config().power.pc_lost_after();
    d.pull_pc_plug();
    d.world.plug_hub(Machine::Mac);
    // Dentro del umbral la Mac todavía no sabe si la PC sigue ahí.
    tokio::time::sleep(lost_after - Duration::from_secs(1)).await;
    assert!(d.mac.state().pc_on);
    assert_eq!(roles.server(false), Machine::Pc);

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!d.mac.state().pc_on);
    assert_eq!(roles.server(false), Machine::Mac);
    assert_eq!(roles.chip(Machine::Mac), Role::Server);

    // La PC vuelve con su botón: el hub sigue en la Mac y la PC se pone de cliente.
    d.press_pc_power_button();
    let mut states = d.mac.subscribe();
    tokio::time::timeout(Duration::from_secs(3600), states.wait_for(|s| s.pc_on))
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
