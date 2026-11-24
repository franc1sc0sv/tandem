//! Who is the keyboard and mouse server (DESIGN §7).

use serde::{Deserialize, Serialize};
use tandem_core::Machine;

/// Role of this machine in the input session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Has the keyboard and mouse: captures and sends.
    Server,
    /// Receives and injects.
    Client,
}

/// UI selector. `Manual` wins over hub detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "mode", content = "server")]
pub enum Selector {
    /// The server is the machine where the hub is.
    #[default]
    Auto,
    /// The server is this machine, wherever the hub is.
    Manual(Machine),
}

/// What one agent reports to the other: whether the hub is on its machine and whom
/// it has as server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerReport {
    pub hub: bool,
    pub server: Machine,
}

/// What this agent knows about the other one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Peer {
    /// No connection, or no report yet.
    Unknown,
    /// Assumed off (this machine's state says so): a powered-off machine does not
    /// see any USB device, so it does not have the hub.
    Off,
    /// Its last report. `server` is missing if it only reported the hub.
    Seen { hub: bool, server: Option<Machine> },
}

/// Decides which machine is the server from what this agent knows: whether the hub is
/// here (already debounced), what the other agent reports and the selector.
///
/// Each of the two machines runs its own arbiter; if they see the same thing (what is here for
/// one is what is there for the other, and the same selector), they reach the same server.
/// In automatic mode a decision is made only with both readings and the hub on a single
/// machine. If a reading is missing (no connection with the other agent) or the hub
/// appears on neither or on both, the previous server stays: a long
/// hub or link disconnection does not invert the roles, and the two
/// machines cannot split by seeing it differently while they are not talking.
///
/// The exception is a powered-off other agent ([`RoleArbiter::set_peer_off`], T-066):
/// the one still on decides alone, because the off one does not have the hub. The off one
/// does not find out, so on return it may bring another server; if the hub does not decide
/// then either (on neither or on both), the `A` side rules and `B` adopts it from its
/// [`PeerReport`]: this way the two agree again without depending on which system each
/// one has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleArbiter {
    me: Machine,
    selector: Selector,
    /// `None` until the first reading.
    local_hub: Option<bool>,
    peer: Peer,
    server: Machine,
}

impl RoleArbiter {
    /// `server` is the server to start with (the last saved one, for example).
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

    /// Whether the hub is on the other machine, according to its report; `Some(false)` if it is off.
    pub fn peer_hub(&self) -> Option<bool> {
        match self.peer {
            Peer::Unknown => None,
            Peer::Off => Some(false),
            Peer::Seen { hub, .. } => Some(hub),
        }
    }

    /// Whether the other agent is assumed off.
    pub fn peer_off(&self) -> bool {
        self.peer == Peer::Off
    }

    /// What this agent reports to the other.
    pub fn report(&self) -> Option<PeerReport> {
        self.local_hub.map(|hub| PeerReport {
            hub,
            server: self.server,
        })
    }

    /// Changes the selector. Returns the new server if it changed.
    /// The user said where the keyboard is (the setup, T-087). With no hub to decide, it is
    /// the server; with a hub, the next hub reading rules again.
    pub fn declare(&mut self, server: Machine) -> Option<Machine> {
        let before = self.server;
        self.server = server;
        (before != server).then_some(server)
    }

    pub fn set_selector(&mut self, selector: Selector) -> Option<Machine> {
        self.selector = selector;
        self.resolve()
    }

    /// The (stable) hub connected or disconnected here.
    pub fn set_local_hub(&mut self, present: bool) -> Option<Machine> {
        self.local_hub = Some(present);
        self.resolve()
    }

    /// What the other agent reports, only the hub; `None` on losing the connection with it.
    pub fn set_peer_hub(&mut self, present: Option<bool>) -> Option<Machine> {
        self.peer = match present {
            Some(hub) => Peer::Seen { hub, server: None },
            None => Peer::Unknown,
        };
        self.resolve()
    }

    /// The complete report of the other agent; `None` on losing the connection with it.
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

    /// The other agent is assumed off (according to this machine's state): counts as
    /// not having the hub. Removed by its next report or by losing the connection.
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
                // Tie: on B the server of A rules (see above).
                _ => match self.peer {
                    Peer::Seen {
                        server: Some(theirs),
                        ..
                    } if self.me == Machine::B => theirs,
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
        let mut a = RoleArbiter::new(Machine::B, Machine::A);
        assert_eq!(a.role(), Role::Client);
        assert_eq!(a.set_peer_hub(Some(false)), None);
        assert_eq!(a.set_local_hub(true), Some(Machine::B));
        assert_eq!(a.role(), Role::Server);
        assert_eq!(a.set_local_hub(true), None);
    }

    #[test]
    fn hub_moving_to_the_peer_inverts_the_roles() {
        let mut a = RoleArbiter::new(Machine::B, Machine::B);
        a.set_local_hub(true);
        a.set_peer_hub(Some(false));
        // The user moves the hub: it disappears here and appears there, in any order.
        assert_eq!(a.set_local_hub(false), None);
        assert_eq!(a.server(), Machine::B);
        assert_eq!(a.set_peer_hub(Some(true)), Some(Machine::A));
        assert_eq!(a.role(), Role::Client);

        // And back, in the opposite order: both see it for a moment.
        assert_eq!(a.set_local_hub(true), None);
        assert_eq!(a.server(), Machine::A);
        assert_eq!(a.set_peer_hub(Some(false)), Some(Machine::B));
    }

    #[test]
    fn hub_nowhere_keeps_the_last_server() {
        let mut a = RoleArbiter::new(Machine::A, Machine::A);
        a.set_local_hub(true);
        a.set_peer_hub(Some(false));
        assert_eq!(a.set_local_hub(false), None);
        assert_eq!(a.server(), Machine::A);
        assert_eq!(a.set_peer_hub(None), None);
        assert_eq!(a.server(), Machine::A);
    }

    #[test]
    fn nothing_changes_without_the_peer_report() {
        let mut a = RoleArbiter::new(Machine::A, Machine::B);
        assert_eq!(a.set_local_hub(true), None);
        assert_eq!(a.server(), Machine::B);
        // The other agent's report arrives: now it does.
        assert_eq!(a.set_peer_hub(Some(false)), Some(Machine::A));
        // Losing the connection changes nothing, even if the hub leaves here.
        assert_eq!(a.set_peer_hub(None), None);
        assert_eq!(a.set_local_hub(false), None);
        assert_eq!(a.server(), Machine::A);
    }

    #[test]
    fn hub_seen_on_both_keeps_the_server() {
        let mut a = RoleArbiter::new(Machine::A, Machine::B);
        a.set_peer_hub(Some(true));
        assert_eq!(a.set_local_hub(true), None);
        assert_eq!(a.server(), Machine::B);
    }

    #[test]
    fn manual_selector_wins_over_detection() {
        let mut a = RoleArbiter::new(Machine::A, Machine::A);
        a.set_local_hub(true);
        assert_eq!(
            a.set_selector(Selector::Manual(Machine::B)),
            Some(Machine::B)
        );
        assert_eq!(a.role(), Role::Client);
        // Detection keeps arriving and changes nothing.
        assert_eq!(a.set_local_hub(false), None);
        assert_eq!(a.set_peer_hub(Some(true)), None);
        assert_eq!(a.set_local_hub(true), None);
        assert_eq!(a.set_peer_hub(Some(false)), None);
        assert_eq!(a.server(), Machine::B);
        // Going back to automatic applies what was detected.
        assert_eq!(a.set_selector(Selector::Auto), Some(Machine::A));
    }

    #[test]
    fn selector_serializes_for_the_ui() {
        let auto = serde_json_like(&Selector::Auto);
        let manual = serde_json_like(&Selector::Manual(Machine::B));
        assert_eq!(auto, r#"mode = "auto""#);
        assert_eq!(manual, "mode = \"manual\"\nserver = \"b\"");
    }

    fn serde_json_like(s: &Selector) -> String {
        toml::to_string(s).unwrap().trim().to_owned()
    }

    /// A fact about the desk. Each one reaches both agents before the next:
    /// the debounce is seconds and the report takes milliseconds.
    #[derive(Debug, Clone, Copy)]
    enum Op {
        Selector(Selector),
        /// The (stable) hub appears or disappears on a machine.
        Hub(Machine, bool),
        PeerLost,
        /// The connection is recovered and each one sends what it sees.
        PeerBack,
    }

    fn machine() -> impl Strategy<Value = Machine> {
        prop_oneof![Just(Machine::A), Just(Machine::B)]
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

    /// The two agents and the real desk.
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
                mac: RoleArbiter::new(Machine::A, start),
                pc: RoleArbiter::new(Machine::B, start),
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

        /// Applies the fact and returns the change each agent notified.
        fn apply(&mut self, op: Op) -> [Option<Machine>; 2] {
            match op {
                Op::Selector(s) => [self.mac.set_selector(s), self.pc.set_selector(s)],
                Op::Hub(Machine::A, present) => {
                    self.hub_on_mac = present;
                    let a = self.mac.set_local_hub(present);
                    let b = self
                        .connected
                        .then(|| self.pc.set_peer_hub(Some(present)))
                        .flatten();
                    [a, b]
                }
                Op::Hub(Machine::B, present) => {
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
        /// With the selector on manual, the server is the chosen one no matter what happens
        /// with the hub; and only a real change is notified.
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

        /// The two machines always agree on the server (one is server and the
        /// other client); and in automatic mode and connected, if the hub is on a single
        /// machine, that one is the server.
        #[test]
        fn both_agents_agree(start in machine(), ops in proptest::collection::vec(op(), 0..40)) {
            let mut desk = Desk::new(start);
            for op in ops {
                desk.apply(op);
                prop_assert_eq!(desk.mac.server(), desk.pc.server());
                prop_assert_ne!(desk.mac.role(), desk.pc.role());
                if desk.connected && desk.mac.selector() == Selector::Auto && desk.hub_on_mac != desk.hub_on_pc {
                    let at = if desk.hub_on_mac { Machine::A } else { Machine::B };
                    prop_assert_eq!(desk.mac.server(), at);
                }
            }
        }
    }

    // --- The PC off (T-066) ---

    #[test]
    fn with_the_pc_off_the_mac_decides_alone() {
        // The PC had the hub; it powers off and the hub moves to the Mac.
        let mut mac = RoleArbiter::new(Machine::A, Machine::B);
        mac.set_local_hub(false);
        mac.set_peer_hub(Some(true));
        assert_eq!(mac.server(), Machine::B);
        assert_eq!(
            mac.set_peer_off(),
            None,
            "without the hub here nothing changes"
        );
        assert!(mac.peer_off());
        assert_eq!(mac.peer_hub(), Some(false));
        assert_eq!(mac.set_local_hub(true), Some(Machine::A));
        // Losing the hub here does not return the server to the powered-off PC either.
        assert_eq!(mac.set_local_hub(false), None);
        assert_eq!(mac.server(), Machine::A);
    }

    #[test]
    fn peer_off_ends_with_its_next_report_or_without_connection() {
        let mut mac = RoleArbiter::new(Machine::A, Machine::A);
        mac.set_peer_off();
        mac.set_peer(Some(PeerReport {
            hub: true,
            server: Machine::A,
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
        // The Mac stayed as server while the PC was off; on return the
        // PC brings the last thing it saved, and the hub is on neither.
        let mut pc = RoleArbiter::new(Machine::B, Machine::B);
        let mut mac = RoleArbiter::new(Machine::A, Machine::A);
        mac.set_local_hub(false);
        assert_eq!(pc.set_local_hub(false), None);
        assert_eq!(pc.set_peer(mac.report()), Some(Machine::A));
        assert_eq!(mac.set_peer(pc.report()), None);
        assert_eq!(mac.server(), Machine::A);
    }

    #[test]
    fn the_mac_keeps_its_server_on_a_tie() {
        let mut mac = RoleArbiter::new(Machine::A, Machine::A);
        mac.set_local_hub(true);
        let theirs = PeerReport {
            hub: true,
            server: Machine::B,
        };
        assert_eq!(mac.set_peer(Some(theirs)), None);
        assert_eq!(mac.server(), Machine::A);
    }

    #[test]
    fn the_hub_still_wins_over_the_reported_server() {
        let mut pc = RoleArbiter::new(Machine::B, Machine::A);
        pc.set_local_hub(true);
        let mac = PeerReport {
            hub: false,
            server: Machine::A,
        };
        assert_eq!(pc.set_peer(Some(mac)), Some(Machine::B));
    }

    #[test]
    fn the_report_needs_a_local_reading() {
        let mut a = RoleArbiter::new(Machine::B, Machine::A);
        assert_eq!(a.report(), None);
        a.set_local_hub(true);
        assert_eq!(
            a.report(),
            Some(PeerReport {
                hub: true,
                server: Machine::A
            })
        );
    }

    /// A fact about the desk with the PC that powers off and comes back.
    #[derive(Debug, Clone, Copy)]
    enum PowerOp {
        Selector(Selector),
        /// Where the hub really is (on the powered-off PC it is not seen).
        Hub(Machine, bool),
        LinkDown,
        LinkUp,
        /// The PC powers off (or loses power) and the Mac, past the threshold, knows it.
        PcOff,
        /// The links drop long enough for the Mac to consider the PC
        /// off, even if it is still on (T-063).
        PresumedOff,
        /// The PC starts with whatever server it saved, whichever it is.
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
                mac: RoleArbiter::new(Machine::A, start),
                pc: RoleArbiter::new(Machine::B, start),
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
                PowerOp::Hub(Machine::A, p) => self.hub_on_mac = p,
                PowerOp::Hub(Machine::B, p) => self.hub_on_pc = p,
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
                        // The selector is applied on both agents.
                        self.pc = RoleArbiter::new(Machine::B, saved);
                        self.pc.set_selector(self.mac.selector());
                        self.pc_running = true;
                        self.link = true;
                    }
                }
            }
            self.deliver();
        }

        /// Each agent reads its hub and, if they talk, they exchange reports until
        /// nothing changes (each server change is reported again).
        fn deliver(&mut self) {
            self.mac.set_local_hub(self.hub_on_mac);
            if self.pc_running {
                self.pc.set_local_hub(self.hub_on_pc);
            }
            if self.connected() {
                // Rising edge: the Mac considers the PC on again.
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
        /// With the PC off and the hub on the Mac, the Mac is the server; and when
        /// the two talk, they agree, whichever server the PC
        /// brings on startup.
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
                        let at = if desk.hub_on_mac { Machine::A } else { Machine::B };
                        prop_assert_eq!(desk.mac.server(), at);
                    }
                }
                if auto && desk.mac_thinks_off && desk.hub_on_mac {
                    prop_assert_eq!(desk.mac.server(), Machine::A);
                }
                if let Selector::Manual(m) = desk.mac.selector() {
                    prop_assert_eq!(desk.mac.server(), m);
                }
            }
        }
    }
}
