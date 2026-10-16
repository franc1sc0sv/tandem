//! Quién es el servidor de teclado y mouse (DESIGN §7).

use serde::{Deserialize, Serialize};
use tandem_core::Machine;

/// Papel de esta máquina en la sesión de entrada.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Tiene el teclado y el mouse: captura y manda.
    Server,
    /// Recibe e inyecta.
    Client,
}

/// Selector de la UI. `Manual` gana sobre la detección del hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "mode", content = "server")]
pub enum Selector {
    /// El servidor es la máquina donde está el hub.
    #[default]
    Auto,
    /// El servidor es esta máquina, esté donde esté el hub.
    Manual(Machine),
}

/// Lo que un agente le reporta al otro: si el hub está en su máquina y a quién
/// tiene él de servidor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerReport {
    pub hub: bool,
    pub server: Machine,
}

/// Lo que este agente sabe del otro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Peer {
    /// Sin conexión, o todavía sin reporte.
    Unknown,
    /// Se lo da por apagado (`pc_on = false` en la Mac): una máquina apagada no
    /// ve ningún dispositivo USB, así que no tiene el hub.
    Off,
    /// Su último reporte. `server` falta si solo reportó el hub.
    Seen { hub: bool, server: Option<Machine> },
}

/// Decide qué máquina es el servidor con lo que sabe este agente: si el hub está
/// aquí (ya con antirrebote), lo que reporta el otro agente y el selector.
///
/// Las dos máquinas corren cada una su árbitro; si ven lo mismo (lo de aquí de
/// una es lo del otro de la otra, y el mismo selector), llegan al mismo servidor.
/// En automático solo se decide con las dos lecturas y el hub en una sola
/// máquina. Si falta una lectura (sin conexión con el otro agente) o el hub no
/// aparece en ninguna o aparece en las dos, se queda el servidor que había: una
/// desconexión larga del hub o del enlace no invierte los roles, y las dos
/// máquinas no pueden separarse por verlo distinto mientras no se hablan.
///
/// La excepción es un otro agente apagado ([`RoleArbiter::set_peer_off`], T-066):
/// la Mac decide sola, porque la PC apagada no tiene el hub. Mientras tanto la PC
/// no se entera, así que al volver puede traer otro servidor; si el hub tampoco
/// decide entonces (en ninguna o en las dos), manda el de la Mac, que es la que
/// siguió prendida. La PC lo adopta del [`PeerReport`] de la Mac.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleArbiter {
    me: Machine,
    selector: Selector,
    /// `None` hasta la primera lectura.
    local_hub: Option<bool>,
    peer: Peer,
    server: Machine,
}

impl RoleArbiter {
    /// `server` es el servidor con el que se arranca (el último guardado, por ejemplo).
    pub fn new(me: Machine, server: Machine) -> Self {
        RoleArbiter {
            me,
            selector: Selector::Auto,
            local_hub: None,
            peer: Peer::Unknown,
            server,
        }
    }

    pub fn me(&self) -> Machine {
        self.me
    }

    pub fn server(&self) -> Machine {
        self.server
    }

    pub fn role(&self) -> Role {
        if self.server == self.me {
            Role::Server
        } else {
            Role::Client
        }
    }

    pub fn selector(&self) -> Selector {
        self.selector
    }

    pub fn local_hub(&self) -> Option<bool> {
        self.local_hub
    }

    /// Si el hub está en la otra máquina, según su reporte; `Some(false)` si está apagada.
    pub fn peer_hub(&self) -> Option<bool> {
        match self.peer {
            Peer::Unknown => None,
            Peer::Off => Some(false),
            Peer::Seen { hub, .. } => Some(hub),
        }
    }

    /// Si se da al otro agente por apagado.
    pub fn peer_off(&self) -> bool {
        self.peer == Peer::Off
    }

    /// Lo que este agente le reporta al otro.
    pub fn report(&self) -> Option<PeerReport> {
        self.local_hub.map(|hub| PeerReport {
            hub,
            server: self.server,
        })
    }

    /// Cambia el selector. Devuelve el servidor nuevo si cambió.
    pub fn set_selector(&mut self, selector: Selector) -> Option<Machine> {
        self.selector = selector;
        self.resolve()
    }

    /// El hub (estable) se conectó o desconectó aquí.
    pub fn set_local_hub(&mut self, present: bool) -> Option<Machine> {
        self.local_hub = Some(present);
        self.resolve()
    }

    /// Lo que reporta el otro agente, solo el hub; `None` al perder la conexión con él.
    pub fn set_peer_hub(&mut self, present: Option<bool>) -> Option<Machine> {
        self.peer = match present {
            Some(hub) => Peer::Seen { hub, server: None },
            None => Peer::Unknown,
        };
        self.resolve()
    }

    /// El reporte completo del otro agente; `None` al perder la conexión con él.
    pub fn set_peer(&mut self, report: Option<PeerReport>) -> Option<Machine> {
        self.peer = match report {
            Some(r) => Peer::Seen {
                hub: r.hub,
                server: Some(r.server),
            },
            None => Peer::Unknown,
        };
        self.resolve()
    }

    /// El otro agente se da por apagado (en la Mac, `pc_on = false`): cuenta como
    /// que no tiene el hub. Lo quita el siguiente reporte suyo o perder la conexión.
    pub fn set_peer_off(&mut self) -> Option<Machine> {
        self.peer = Peer::Off;
        self.resolve()
    }

    fn resolve(&mut self) -> Option<Machine> {
        let next = match self.selector {
            Selector::Manual(machine) => machine,
            Selector::Auto => match (self.local_hub, self.peer_hub()) {
                (Some(true), Some(false)) => self.me,
                (Some(false), Some(true)) => self.me.other(),
                // Empate: en la PC manda el servidor de la Mac (ver arriba).
                _ => match self.peer {
                    Peer::Seen {
                        server: Some(theirs),
                        ..
                    } if self.me == Machine::Pc => theirs,
                    _ => self.server,
                },
            },
        };
        (next != self.server).then(|| {
            self.server = next;
            next
        })
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn hub_here_makes_this_machine_the_server() {
        let mut a = RoleArbiter::new(Machine::Pc, Machine::Mac);
        assert_eq!(a.role(), Role::Client);
        assert_eq!(a.set_peer_hub(Some(false)), None);
        assert_eq!(a.set_local_hub(true), Some(Machine::Pc));
        assert_eq!(a.role(), Role::Server);
        assert_eq!(a.set_local_hub(true), None);
    }

    #[test]
    fn hub_moving_to_the_peer_inverts_the_roles() {
        let mut a = RoleArbiter::new(Machine::Pc, Machine::Pc);
        a.set_local_hub(true);
        a.set_peer_hub(Some(false));
        // El usuario mueve el hub: aquí desaparece y allá aparece, en cualquier orden.
        assert_eq!(a.set_local_hub(false), None);
        assert_eq!(a.server(), Machine::Pc);
        assert_eq!(a.set_peer_hub(Some(true)), Some(Machine::Mac));
        assert_eq!(a.role(), Role::Client);

        // Y de vuelta, con el orden contrario: los dos un momento lo ven.
        assert_eq!(a.set_local_hub(true), None);
        assert_eq!(a.server(), Machine::Mac);
        assert_eq!(a.set_peer_hub(Some(false)), Some(Machine::Pc));
    }

    #[test]
    fn hub_nowhere_keeps_the_last_server() {
        let mut a = RoleArbiter::new(Machine::Mac, Machine::Mac);
        a.set_local_hub(true);
        a.set_peer_hub(Some(false));
        assert_eq!(a.set_local_hub(false), None);
        assert_eq!(a.server(), Machine::Mac);
        assert_eq!(a.set_peer_hub(None), None);
        assert_eq!(a.server(), Machine::Mac);
    }

    #[test]
    fn nothing_changes_without_the_peer_report() {
        let mut a = RoleArbiter::new(Machine::Mac, Machine::Pc);
        assert_eq!(a.set_local_hub(true), None);
        assert_eq!(a.server(), Machine::Pc);
        // Llega el reporte del otro agente: ahora sí.
        assert_eq!(a.set_peer_hub(Some(false)), Some(Machine::Mac));
        // Perder la conexión no cambia nada, aunque el hub se vaya de aquí.
        assert_eq!(a.set_peer_hub(None), None);
        assert_eq!(a.set_local_hub(false), None);
        assert_eq!(a.server(), Machine::Mac);
    }

    #[test]
    fn hub_seen_on_both_keeps_the_server() {
        let mut a = RoleArbiter::new(Machine::Mac, Machine::Pc);
        a.set_peer_hub(Some(true));
        assert_eq!(a.set_local_hub(true), None);
        assert_eq!(a.server(), Machine::Pc);
    }

    #[test]
    fn manual_selector_wins_over_detection() {
        let mut a = RoleArbiter::new(Machine::Mac, Machine::Mac);
        a.set_local_hub(true);
        assert_eq!(
            a.set_selector(Selector::Manual(Machine::Pc)),
            Some(Machine::Pc)
        );
        assert_eq!(a.role(), Role::Client);
        // La detección sigue llegando y no cambia nada.
        assert_eq!(a.set_local_hub(false), None);
        assert_eq!(a.set_peer_hub(Some(true)), None);
        assert_eq!(a.set_local_hub(true), None);
        assert_eq!(a.set_peer_hub(Some(false)), None);
        assert_eq!(a.server(), Machine::Pc);
        // Volver a automático aplica lo detectado.
        assert_eq!(a.set_selector(Selector::Auto), Some(Machine::Mac));
    }

    #[test]
    fn selector_serializes_for_the_ui() {
        let auto = serde_json_like(&Selector::Auto);
        let manual = serde_json_like(&Selector::Manual(Machine::Pc));
        assert_eq!(auto, r#"mode = "auto""#);
        assert_eq!(manual, "mode = \"manual\"\nserver = \"pc\"");
    }

    fn serde_json_like(s: &Selector) -> String {
        toml::to_string(s).unwrap().trim().to_owned()
    }

    /// Un hecho del puesto. Cada uno llega a los dos agentes antes del siguiente:
    /// el antirrebote es de segundos y el reporte tarda milisegundos.
    #[derive(Debug, Clone, Copy)]
    enum Op {
        Selector(Selector),
        /// El hub (estable) aparece o desaparece en una máquina.
        Hub(Machine, bool),
        PeerLost,
        /// Se recupera la conexión y cada uno manda lo que ve.
        PeerBack,
    }

    fn machine() -> impl Strategy<Value = Machine> {
        prop_oneof![Just(Machine::Mac), Just(Machine::Pc)]
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            prop_oneof![Just(Selector::Auto), machine().prop_map(Selector::Manual)]
                .prop_map(Op::Selector),
            (machine(), any::<bool>()).prop_map(|(m, p)| Op::Hub(m, p)),
            (machine(), any::<bool>()).prop_map(|(m, p)| Op::Hub(m, p)),
            Just(Op::PeerLost),
            Just(Op::PeerBack),
        ]
    }

    /// Los dos agentes y el puesto real.
    struct Desk {
        mac: RoleArbiter,
        pc: RoleArbiter,
        hub_on_mac: bool,
        hub_on_pc: bool,
        connected: bool,
    }

    impl Desk {
        fn new(start: Machine) -> Self {
            let mut d = Desk {
                mac: RoleArbiter::new(Machine::Mac, start),
                pc: RoleArbiter::new(Machine::Pc, start),
                hub_on_mac: false,
                hub_on_pc: false,
                connected: true,
            };
            d.reconnect();
            d
        }

        fn reconnect(&mut self) {
            self.connected = true;
            self.mac.set_local_hub(self.hub_on_mac);
            self.pc.set_local_hub(self.hub_on_pc);
            self.mac.set_peer_hub(Some(self.hub_on_pc));
            self.pc.set_peer_hub(Some(self.hub_on_mac));
        }

        /// Aplica el hecho y devuelve el cambio que avisó cada agente.
        fn apply(&mut self, op: Op) -> [Option<Machine>; 2] {
            match op {
                Op::Selector(s) => [self.mac.set_selector(s), self.pc.set_selector(s)],
                Op::Hub(Machine::Mac, present) => {
                    self.hub_on_mac = present;
                    let a = self.mac.set_local_hub(present);
                    let b = self
                        .connected
                        .then(|| self.pc.set_peer_hub(Some(present)))
                        .flatten();
                    [a, b]
                }
                Op::Hub(Machine::Pc, present) => {
                    self.hub_on_pc = present;
                    let b = self.pc.set_local_hub(present);
                    let a = self
                        .connected
                        .then(|| self.mac.set_peer_hub(Some(present)))
                        .flatten();
                    [a, b]
                }
                Op::PeerLost => {
                    self.connected = false;
                    [self.mac.set_peer_hub(None), self.pc.set_peer_hub(None)]
                }
                Op::PeerBack => {
                    let before = [self.mac.server(), self.pc.server()];
                    self.reconnect();
                    let after = [self.mac.server(), self.pc.server()];
                    [
                        (before[0] != after[0]).then_some(after[0]),
                        (before[1] != after[1]).then_some(after[1]),
                    ]
                }
            }
        }
    }

    proptest! {
        /// Con el selector en manual, el servidor es el elegido pase lo que pase
        /// con el hub; y solo se avisa un cambio de verdad.
        #[test]
        fn manual_always_wins(start in machine(), ops in proptest::collection::vec(op(), 0..40)) {
            let mut desk = Desk::new(start);
            for op in ops {
                let before = [desk.mac.server(), desk.pc.server()];
                let changed = desk.apply(op);
                for (i, a) in [&desk.mac, &desk.pc].into_iter().enumerate() {
                    if let Selector::Manual(m) = a.selector() {
                        prop_assert_eq!(a.server(), m);
                    }
                    match changed[i] {
                        Some(m) => {
                            prop_assert_eq!(m, a.server());
                            prop_assert_ne!(m, before[i]);
                        }
                        None => prop_assert_eq!(a.server(), before[i]),
                    }
                }
            }
        }

        /// Las dos máquinas coinciden siempre en el servidor (una es servidor y la
        /// otra cliente); y en automático y conectadas, si el hub está en una sola
        /// máquina, esa es el servidor.
        #[test]
        fn both_agents_agree(start in machine(), ops in proptest::collection::vec(op(), 0..40)) {
            let mut desk = Desk::new(start);
            for op in ops {
                desk.apply(op);
                prop_assert_eq!(desk.mac.server(), desk.pc.server());
                prop_assert_ne!(desk.mac.role(), desk.pc.role());
                if desk.connected && desk.mac.selector() == Selector::Auto && desk.hub_on_mac != desk.hub_on_pc {
                    let at = if desk.hub_on_mac { Machine::Mac } else { Machine::Pc };
                    prop_assert_eq!(desk.mac.server(), at);
                }
            }
        }
    }

    // --- La PC apagada (T-066) ---

    #[test]
    fn with_the_pc_off_the_mac_decides_alone() {
        // La PC tenía el hub; se apaga y el hub pasa a la Mac.
        let mut mac = RoleArbiter::new(Machine::Mac, Machine::Pc);
        mac.set_local_hub(false);
        mac.set_peer_hub(Some(true));
        assert_eq!(mac.server(), Machine::Pc);
        assert_eq!(mac.set_peer_off(), None, "sin el hub aquí no cambia nada");
        assert!(mac.peer_off());
        assert_eq!(mac.peer_hub(), Some(false));
        assert_eq!(mac.set_local_hub(true), Some(Machine::Mac));
        // Perder el hub aquí tampoco devuelve el servidor a la PC apagada.
        assert_eq!(mac.set_local_hub(false), None);
        assert_eq!(mac.server(), Machine::Mac);
    }

    #[test]
    fn peer_off_ends_with_its_next_report_or_without_connection() {
        let mut mac = RoleArbiter::new(Machine::Mac, Machine::Mac);
        mac.set_peer_off();
        mac.set_peer(Some(PeerReport {
            hub: true,
            server: Machine::Mac,
        }));
        assert!(!mac.peer_off());
        assert_eq!(mac.peer_hub(), Some(true));
        mac.set_peer_off();
        mac.set_peer(None);
        assert!(!mac.peer_off());
        assert_eq!(mac.peer_hub(), None);
    }

    #[test]
    fn the_pc_coming_back_follows_the_mac_on_a_tie() {
        // La Mac quedó de servidor mientras la PC estaba apagada; al volver la
        // PC trae lo último que guardó, y el hub no está en ninguna.
        let mut pc = RoleArbiter::new(Machine::Pc, Machine::Pc);
        let mut mac = RoleArbiter::new(Machine::Mac, Machine::Mac);
        mac.set_local_hub(false);
        assert_eq!(pc.set_local_hub(false), None);
        assert_eq!(pc.set_peer(mac.report()), Some(Machine::Mac));
        assert_eq!(mac.set_peer(pc.report()), None);
        assert_eq!(mac.server(), Machine::Mac);
    }

    #[test]
    fn the_mac_keeps_its_server_on_a_tie() {
        let mut mac = RoleArbiter::new(Machine::Mac, Machine::Mac);
        mac.set_local_hub(true);
        let theirs = PeerReport {
            hub: true,
            server: Machine::Pc,
        };
        assert_eq!(mac.set_peer(Some(theirs)), None);
        assert_eq!(mac.server(), Machine::Mac);
    }

    #[test]
    fn the_hub_still_wins_over_the_reported_server() {
        let mut pc = RoleArbiter::new(Machine::Pc, Machine::Mac);
        pc.set_local_hub(true);
        let mac = PeerReport {
            hub: false,
            server: Machine::Mac,
        };
        assert_eq!(pc.set_peer(Some(mac)), Some(Machine::Pc));
    }

    #[test]
    fn the_report_needs_a_local_reading() {
        let mut a = RoleArbiter::new(Machine::Pc, Machine::Mac);
        assert_eq!(a.report(), None);
        a.set_local_hub(true);
        assert_eq!(
            a.report(),
            Some(PeerReport {
                hub: true,
                server: Machine::Mac
            })
        );
    }

    /// Un hecho del puesto con la PC que se apaga y vuelve.
    #[derive(Debug, Clone, Copy)]
    enum PowerOp {
        Selector(Selector),
        /// Dónde está el hub de verdad (en la PC apagada no se ve).
        Hub(Machine, bool),
        LinkDown,
        LinkUp,
        /// La PC se apaga (o pierde la corriente) y la Mac, pasado el umbral, lo sabe.
        PcOff,
        /// Los enlaces caen el tiempo suficiente para que la Mac dé la PC por
        /// apagada, aunque siga prendida (T-063).
        PresumedOff,
        /// La PC arranca con el servidor que haya guardado, sea cual sea.
        PcOn(Machine),
    }

    fn power_op() -> impl Strategy<Value = PowerOp> {
        prop_oneof![
            prop_oneof![Just(Selector::Auto), machine().prop_map(Selector::Manual)]
                .prop_map(PowerOp::Selector),
            (machine(), any::<bool>()).prop_map(|(m, p)| PowerOp::Hub(m, p)),
            (machine(), any::<bool>()).prop_map(|(m, p)| PowerOp::Hub(m, p)),
            Just(PowerOp::LinkDown),
            Just(PowerOp::LinkUp),
            Just(PowerOp::PcOff),
            Just(PowerOp::PresumedOff),
            machine().prop_map(PowerOp::PcOn),
        ]
    }

    struct PoweredDesk {
        mac: RoleArbiter,
        pc: RoleArbiter,
        hub_on_mac: bool,
        hub_on_pc: bool,
        pc_running: bool,
        link: bool,
        mac_thinks_off: bool,
    }

    impl PoweredDesk {
        fn new(start: Machine) -> Self {
            let mut d = PoweredDesk {
                mac: RoleArbiter::new(Machine::Mac, start),
                pc: RoleArbiter::new(Machine::Pc, start),
                hub_on_mac: false,
                hub_on_pc: false,
                pc_running: true,
                link: true,
                mac_thinks_off: false,
            };
            d.deliver();
            d
        }

        fn connected(&self) -> bool {
            self.pc_running && self.link
        }

        fn apply(&mut self, op: PowerOp) {
            match op {
                PowerOp::Selector(s) => {
                    self.mac.set_selector(s);
                    if self.pc_running {
                        self.pc.set_selector(s);
                    }
                }
                PowerOp::Hub(Machine::Mac, p) => self.hub_on_mac = p,
                PowerOp::Hub(Machine::Pc, p) => self.hub_on_pc = p,
                PowerOp::LinkDown => self.link = false,
                PowerOp::LinkUp => self.link = true,
                PowerOp::PcOff => {
                    self.pc_running = false;
                    self.mac_thinks_off = true;
                }
                PowerOp::PresumedOff => {
                    self.link = false;
                    self.mac_thinks_off = true;
                }
                PowerOp::PcOn(saved) => {
                    if !self.pc_running {
                        // El selector se aplica en los dos agentes.
                        self.pc = RoleArbiter::new(Machine::Pc, saved);
                        self.pc.set_selector(self.mac.selector());
                        self.pc_running = true;
                        self.link = true;
                    }
                }
            }
            self.deliver();
        }

        /// Cada agente lee su hub y, si se hablan, intercambian reportes hasta
        /// que no cambia nada (cada cambio de servidor se vuelve a reportar).
        fn deliver(&mut self) {
            self.mac.set_local_hub(self.hub_on_mac);
            if self.pc_running {
                self.pc.set_local_hub(self.hub_on_pc);
            }
            if self.connected() {
                // Flanco de subida: la Mac vuelve a dar la PC por prendida.
                self.mac_thinks_off = false;
                for _ in 0..4 {
                    self.mac.set_peer(self.pc.report());
                    self.pc.set_peer(self.mac.report());
                }
            } else {
                if self.mac_thinks_off {
                    self.mac.set_peer_off();
                } else {
                    self.mac.set_peer(None);
                }
                if self.pc_running {
                    self.pc.set_peer(None);
                }
            }
        }
    }

    proptest! {
        /// Con la PC apagada y el hub en la Mac, la Mac es el servidor; y cuando
        /// las dos se hablan, coinciden, sea cual sea el servidor que la PC
        /// traiga al arrancar.
        #[test]
        fn roles_converge_after_the_pc_is_off(
            start in machine(),
            ops in proptest::collection::vec(power_op(), 0..40),
        ) {
            let mut desk = PoweredDesk::new(start);
            for op in ops {
                desk.apply(op);
                let auto = desk.mac.selector() == Selector::Auto;
                if desk.connected() {
                    prop_assert_eq!(desk.mac.server(), desk.pc.server(), "{:?}", op);
                    let pc_hub = desk.hub_on_pc;
                    if auto && desk.hub_on_mac != pc_hub {
                        let at = if desk.hub_on_mac { Machine::Mac } else { Machine::Pc };
                        prop_assert_eq!(desk.mac.server(), at);
                    }
                }
                if auto && desk.mac_thinks_off && desk.hub_on_mac {
                    prop_assert_eq!(desk.mac.server(), Machine::Mac);
                }
                if let Selector::Manual(m) = desk.mac.selector() {
                    prop_assert_eq!(desk.mac.server(), m);
                }
            }
        }
    }
}
