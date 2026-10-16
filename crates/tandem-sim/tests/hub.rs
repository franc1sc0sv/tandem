//! Cambio de hub sobre el puesto simulado (QA ronda 2): el sondeo de cada máquina
//! lee los dispositivos USB que de verdad ve (ninguno con la PC apagada), y los
//! árbitros de roles reciben el reporte del otro solo mientras hay enlace.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tandem_core::{Machine, Mode};
use tandem_link::manager::LinkKind;
use tandem_peripherals::{
    DeviceScanner, HubConfig, HubWatcher, Presence, RoleArbiter, UsbId, WatchConfig,
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
        let (world, net, r) = (d.world.clone(), d.net.clone(), roles.clone());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let pc_on = world.power() == Power::On;
                let connected = pc_on && LinkKind::ALL.iter().any(|k| net.is_up(*k));
                let (mac, pc) = (mac_rx.borrow().hub(), pc_rx.borrow().hub());
                let mut a = r.0.lock().unwrap_or_else(PoisonError::into_inner);
                a[0].set_local_hub(mac);
                a[0].set_peer_hub(connected.then_some(pc));
                // Con la PC apagada su agente no corre.
                if pc_on {
                    a[1].set_local_hub(pc);
                    a[1].set_peer_hub(connected.then_some(mac));
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

    // Hub en la PC y la PC se apaga: sin el reporte de la PC no se invierte nada
    // (no hay con quién ponerse de acuerdo)...
    d.world.plug_hub(Machine::Pc);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(roles.server(true), Machine::Pc);
    let run = d.shutdown_pc().await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed));
    d.world.plug_hub(Machine::Mac);
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(d.world.power(), Power::Off);
    assert_eq!(roles.server(false), Machine::Pc);

    // ...y en cuanto la PC vuelve y reporta que no lo tiene, la Mac es el servidor.
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
}
