//! Teclado y mouse sin dueño (DECISIONS D7, DESIGN §7): las dos máquinas capturan y
//! manda la que recibe entrada física. Máquina de estados sin E/S; el agente le pasa
//! la entrada física de aquí y los [`FocusMsg`] del otro, y manda lo que devuelve.
//!
//! - La que no manda y recibe entrada física la **pide** (`Claim`), a lo sumo una vez
//!   cada [`FollowConfig::claim_every`] mientras siga.
//! - La que manda la **cede** (`Grant`) si lleva [`FollowConfig::quiet`] (300 ms) sin
//!   entrada física propia; si no, el pedido se ignora y el siguiente vuelve a intentar.
//!   Así, con entrada en las dos a la vez, gana la que se movió último después de
//!   300 ms de quietud de la otra.
//! - Cada cambio sube `epoch`. Un pedido de otra época se contesta con el estado
//!   (`State`); con dos vistas distintas gana la de época mayor y, empatadas, la de `A`
//!   (las dos lo saben sin hablarse, DECISIONS A1).
//! - Sin enlace no hay con quién arbitrar: manda la que se usa.

use std::time::Duration;

use tandem_core::Machine;
use tandem_proto::FocusMsg;
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FollowConfig {
    /// Quietud de la que manda para ceder (DESIGN §7: 300 ms).
    pub quiet: Duration,
    /// Cada cuánto se repite el pedido mientras siga la entrada física.
    pub claim_every: Duration,
}

impl Default for FollowConfig {
    fn default() -> Self {
        Self {
            quiet: Duration::from_millis(300),
            claim_every: Duration::from_millis(50),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Follow {
    me: Machine,
    server: Machine,
    epoch: u64,
    cfg: FollowConfig,
    /// Última entrada física de aquí.
    last_input: Option<Instant>,
    /// Último pedido mandado.
    last_claim: Option<Instant>,
}

impl Follow {
    /// `server`: quién manda al arrancar (lo declarado, o `A`).
    pub fn new(me: Machine, server: Machine, cfg: FollowConfig) -> Self {
        Self {
            me,
            server,
            epoch: 0,
            cfg,
            last_input: None,
            last_claim: None,
        }
    }

    pub fn me(&self) -> Machine {
        self.me
    }

    /// La máquina que manda el teclado y el mouse.
    pub fn server(&self) -> Machine {
        self.server
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Lo que esta máquina cree, para mandarlo al conectar y cada tanto.
    pub fn state(&self) -> FocusMsg {
        FocusMsg::State {
            epoch: self.epoch,
            server: self.server,
        }
    }

    fn set(&mut self, server: Machine, epoch: u64) {
        if server != self.server {
            tracing::info!(manda = %server, epoch, "el teclado y mouse sigue a la otra máquina");
        }
        self.server = server;
        self.epoch = epoch;
        self.last_claim = None;
    }

    /// Entrada física en esta máquina. `linked`: hay enlace con la otra. Devuelve el
    /// pedido a mandar, si toca.
    pub fn local_input(&mut self, now: Instant, linked: bool) -> Option<FocusMsg> {
        self.last_input = Some(now);
        if self.server == self.me {
            return None;
        }
        if !linked {
            self.set(self.me, self.epoch + 1);
            return None;
        }
        let due = self
            .last_claim
            .is_none_or(|t| now.saturating_duration_since(t) >= self.cfg.claim_every);
        if !due {
            return None;
        }
        self.last_claim = Some(now);
        Some(FocusMsg::Claim { epoch: self.epoch })
    }

    /// Lo declarado (asistente o configuración) manda sobre todo: una época nueva.
    /// Devuelve el estado para mandar.
    pub fn force(&mut self, server: Machine) -> FocusMsg {
        self.set(server, self.epoch + 1);
        self.state()
    }

    fn quiet(&self, now: Instant) -> bool {
        self.last_input
            .is_none_or(|t| now.saturating_duration_since(t) >= self.cfg.quiet)
    }

    /// Un mensaje del otro. Devuelve la respuesta, si hay.
    pub fn receive(&mut self, msg: FocusMsg, now: Instant) -> Option<FocusMsg> {
        match msg {
            FocusMsg::Claim { epoch } => {
                if epoch != self.epoch || self.server != self.me {
                    // Pide con una vista vieja o distinta: que se ponga al día.
                    return Some(self.state());
                }
                if !self.quiet(now) {
                    return None;
                }
                let next = self.epoch + 1;
                self.set(self.me.other(), next);
                Some(FocusMsg::Grant { epoch: next })
            }
            FocusMsg::Grant { epoch } => {
                if epoch > self.epoch {
                    self.set(self.me, epoch);
                }
                None
            }
            FocusMsg::State { epoch, server } => {
                let theirs_wins = epoch > self.epoch
                    || (epoch == self.epoch && server != self.server && self.me == Machine::B);
                if theirs_wins {
                    self.set(server, epoch);
                    None
                } else if epoch < self.epoch || server != self.server {
                    // La nuestra gana: que la adopte.
                    Some(self.state())
                } else {
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use proptest::prelude::*;

    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn pair() -> (Follow, Follow) {
        let cfg = FollowConfig::default();
        (
            Follow::new(Machine::A, Machine::A, cfg),
            Follow::new(Machine::B, Machine::A, cfg),
        )
    }

    #[test]
    fn la_que_no_manda_pide_y_la_quieta_cede() {
        let t0 = Instant::now();
        let (mut a, mut b) = pair();
        // A nunca se usó: está quieta.
        let claim = b.local_input(t0, true).unwrap();
        assert_eq!(claim, FocusMsg::Claim { epoch: 0 });
        let grant = a.receive(claim, t0 + MS).unwrap();
        assert_eq!(grant, FocusMsg::Grant { epoch: 1 });
        assert_eq!(a.server(), Machine::B);
        assert_eq!(b.receive(grant, t0 + 2 * MS), None);
        assert_eq!(b.server(), Machine::B);
        assert_eq!((a.epoch(), b.epoch()), (1, 1));
        // La que manda no pide nada.
        assert_eq!(b.local_input(t0 + 3 * MS, true), None);
    }

    #[test]
    fn con_entrada_en_las_dos_gana_la_ultima_tras_300_ms_de_quietud() {
        let t0 = Instant::now();
        let (mut a, mut b) = pair();
        a.local_input(t0, true);
        // B se usa 100 ms después: A no está quieta, no cede.
        let claim = b.local_input(t0 + 100 * MS, true).unwrap();
        assert_eq!(a.receive(claim, t0 + 100 * MS), None);
        // Mientras B sigue, no vuelve a pedir antes de `claim_every`.
        assert_eq!(b.local_input(t0 + 120 * MS, true), None);
        // A a 299 ms de su última entrada todavía manda.
        let claim = b.local_input(t0 + 299 * MS, true).unwrap();
        assert_eq!(a.receive(claim, t0 + 299 * MS), None);
        // A los 300 ms cede.
        let claim = b.local_input(t0 + 360 * MS, true).unwrap();
        let grant = a.receive(claim, t0 + 360 * MS).unwrap();
        b.receive(grant, t0 + 361 * MS);
        assert_eq!((a.server(), b.server()), (Machine::B, Machine::B));
    }

    #[test]
    fn sin_enlace_manda_la_que_se_usa_y_al_volver_se_ponen_de_acuerdo() {
        let t0 = Instant::now();
        let (mut a, mut b) = pair();
        assert_eq!(b.local_input(t0, false), None);
        assert_eq!((b.server(), b.epoch()), (Machine::B, 1));
        // A no se enteró. Al volver el enlace, cada una manda su estado.
        let from_b = b.state();
        let reply = a.receive(from_b, t0);
        assert_eq!(reply, None, "B tiene una época nueva: A la adopta");
        assert_eq!(a.server(), Machine::B);
        // Dos decisiones sin enlace en la misma época: gana la de A.
        let (mut a, mut b) = pair();
        a.force(Machine::A);
        b.local_input(t0, false);
        assert_eq!((a.epoch(), b.epoch()), (1, 1));
        let to_a = a.receive(b.state(), t0).unwrap();
        assert_eq!(a.server(), Machine::A);
        assert_eq!(b.receive(to_a, t0), None);
        assert_eq!(b.server(), Machine::A);
    }

    #[test]
    fn un_pedido_viejo_se_contesta_con_el_estado() {
        let t0 = Instant::now();
        let (mut a, mut b) = pair();
        let claim = b.local_input(t0, true).unwrap();
        let grant = a.receive(claim, t0).unwrap();
        // Un segundo pedido de la época 0 llega cuando A ya cedió.
        let late = a.receive(FocusMsg::Claim { epoch: 0 }, t0 + MS).unwrap();
        assert_eq!(
            late,
            FocusMsg::State {
                epoch: 1,
                server: Machine::B
            }
        );
        b.receive(grant, t0);
        assert_eq!(b.receive(late, t0), None);
        // Una cesión vieja no cambia nada.
        b.receive(FocusMsg::Grant { epoch: 1 }, t0);
        assert_eq!((b.server(), b.epoch()), (Machine::B, 1));
    }

    #[test]
    fn lo_declarado_manda() {
        let (mut a, mut b) = pair();
        let msg = a.force(Machine::B);
        assert_eq!(
            msg,
            FocusMsg::State {
                epoch: 1,
                server: Machine::B
            }
        );
        b.receive(msg, Instant::now());
        assert_eq!(b.server(), Machine::B);
    }

    /// Lo que puede pasar en una corrida: entrada física en una máquina, tiempo que
    /// pasa, entregar el próximo mensaje en vuelo hacia una, o cortar/volver el enlace.
    #[derive(Debug, Clone)]
    enum Op {
        Input(Machine),
        Wait(u64),
        Deliver(Machine),
        Link(bool),
    }

    fn op() -> impl Strategy<Value = Op> {
        let m = prop_oneof![Just(Machine::A), Just(Machine::B)];
        prop_oneof![
            4 => m.clone().prop_map(Op::Input),
            3 => (0u64..400).prop_map(Op::Wait),
            4 => m.prop_map(Op::Deliver),
            1 => any::<bool>().prop_map(Op::Link),
        ]
    }

    struct World {
        a: Follow,
        b: Follow,
        /// Mensajes en vuelo hacia cada máquina, en orden (el plano de control es un
        /// stream ordenado).
        to_a: VecDeque<FocusMsg>,
        to_b: VecDeque<FocusMsg>,
        linked: bool,
        now: Instant,
    }

    impl World {
        fn new() -> Self {
            let (a, b) = pair();
            World {
                a,
                b,
                to_a: VecDeque::new(),
                to_b: VecDeque::new(),
                linked: true,
                now: Instant::now(),
            }
        }

        fn send(&mut self, from: Machine, msg: Option<FocusMsg>) {
            if let (Some(m), true) = (msg, self.linked) {
                match from {
                    Machine::A => self.to_b.push_back(m),
                    Machine::B => self.to_a.push_back(m),
                }
            }
        }

        fn apply(&mut self, op: Op) {
            match op {
                Op::Input(m) => {
                    let (now, linked) = (self.now, self.linked);
                    let out = match m {
                        Machine::A => self.a.local_input(now, linked),
                        Machine::B => self.b.local_input(now, linked),
                    };
                    self.send(m, out);
                }
                Op::Wait(ms) => self.now += Duration::from_millis(ms),
                Op::Deliver(m) => {
                    let now = self.now;
                    let (queue, f) = match m {
                        Machine::A => (&mut self.to_a, &mut self.a),
                        Machine::B => (&mut self.to_b, &mut self.b),
                    };
                    if let Some(msg) = queue.pop_front() {
                        let out = f.receive(msg, now);
                        self.send(m, out);
                    }
                }
                Op::Link(up) => {
                    self.linked = up;
                    if !up {
                        // Lo que iba por el enlace caído se pierde.
                        self.to_a.clear();
                        self.to_b.clear();
                    } else {
                        // Al conectar cada una manda su estado.
                        let (sa, sb) = (self.a.state(), self.b.state());
                        self.send(Machine::A, Some(sa));
                        self.send(Machine::B, Some(sb));
                    }
                }
            }
        }

        /// Entrega todo lo que esté en vuelo (y lo que eso genere).
        fn drain(&mut self) {
            for _ in 0..64 {
                if self.to_a.is_empty() && self.to_b.is_empty() {
                    return;
                }
                self.apply(Op::Deliver(Machine::A));
                self.apply(Op::Deliver(Machine::B));
            }
            panic!("los mensajes no se acaban: hay un bucle");
        }
    }

    proptest! {
        /// Pase lo que pase (entrada en las dos, cortes, mensajes demorados), con el
        /// enlace de vuelta y lo pendiente entregado, las dos coinciden en quién manda.
        #[test]
        fn las_dos_terminan_de_acuerdo(ops in prop::collection::vec(op(), 0..120)) {
            let mut w = World::new();
            for op in ops {
                w.apply(op);
            }
            w.apply(Op::Link(true));
            w.drain();
            prop_assert_eq!(w.a.server(), w.b.server());
            prop_assert_eq!(w.a.epoch(), w.b.epoch());
        }

        /// Si una sola máquina se sigue usando (cada 10 ms), en algún momento manda ella,
        /// y una vez que manda no lo pierde mientras la otra no se use.
        #[test]
        fn la_que_se_usa_termina_mandando(
            ops in prop::collection::vec(op(), 0..60),
            user in prop_oneof![Just(Machine::A), Just(Machine::B)],
        ) {
            let mut w = World::new();
            for op in ops {
                w.apply(op);
            }
            w.apply(Op::Link(true));
            w.drain();
            let mut since = None;
            for i in 0..100u32 {
                w.apply(Op::Input(user));
                w.apply(Op::Wait(10));
                w.drain();
                let both = w.a.server() == user && w.b.server() == user;
                match since {
                    None if both => since = Some(i),
                    Some(_) => prop_assert!(both, "perdió el teclado en el paso {}", i),
                    None => {}
                }
            }
            // Quietud (300 ms) más un pedido.
            prop_assert!(since.is_some_and(|i| i <= 36), "{:?}", since);
        }
    }
}
