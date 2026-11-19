//! Keyboard and mouse without an owner (DECISIONS D7, DESIGN §7): both machines capture and
//! the one that receives physical input sends. State machine without I/O; the agent hands it
//! the physical input from here and the other's [`FocusMsg`]s, and sends what it returns.
//!
//! - The one that does not send and receives physical input **asks** (`Claim`), at most once
//!   every [`FollowConfig::claim_every`] while it continues.
//! - The one that sends **yields** (`Grant`) if it has gone [`FollowConfig::quiet`] (300 ms) without
//!   physical input of its own; otherwise, the request is ignored and the next one tries again.
//!   That way, with input on both at once, the one that moved last wins after
//!   300 ms of quiet from the other.
//! - Every change raises `epoch`. A request from another epoch is answered with the state
//!   (`State`); with two different views the one with the higher epoch wins and, if tied, that of `A`
//!   (both know it without talking, DECISIONS A1).
//! - Without a link there is nobody to arbitrate with: the one in use sends.

use std::time::Duration;

use tandem_core::Machine;
use tandem_proto::FocusMsg;
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FollowConfig {
    /// Quiet of the one that sends before yielding (DESIGN §7: 300 ms).
    pub quiet: Duration,
    /// How often the request is repeated while physical input continues.
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
    /// Last physical input from here.
    last_input: Option<Instant>,
    /// Last request sent.
    last_claim: Option<Instant>,
}

impl Follow {
    /// `server`: who sends at startup (what is declared, or `A`).
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

    /// The machine that sends the keyboard and mouse.
    pub fn server(&self) -> Machine {
        self.server
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// What this machine believes, to send on connecting and every so often.
    pub fn state(&self) -> FocusMsg {
        FocusMsg::State {
            epoch: self.epoch,
            server: self.server,
        }
    }

    fn set(&mut self, server: Machine, epoch: u64) {
        if server != self.server {
            tracing::info!(sender = %server, epoch, "the keyboard and mouse follow the other machine");
        }
        self.server = server;
        self.epoch = epoch;
        self.last_claim = None;
    }

    /// Physical input on this machine. `linked`: there is a link with the other. Returns the
    /// request to send, if due.
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

    /// What is declared (setup or configuration) overrides everything: a new epoch.
    /// Returns the state to send.
    pub fn force(&mut self, server: Machine) -> FocusMsg {
        self.set(server, self.epoch + 1);
        self.state()
    }

    fn quiet(&self, now: Instant) -> bool {
        self.last_input
            .is_none_or(|t| now.saturating_duration_since(t) >= self.cfg.quiet)
    }

    /// A message from the other. Returns the reply, if any.
    pub fn receive(&mut self, msg: FocusMsg, now: Instant) -> Option<FocusMsg> {
        match msg {
            FocusMsg::Claim { epoch } => {
                if epoch != self.epoch || self.server != self.me {
                    // It asks with an old or different view: let it catch up.
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
                    // Ours wins: let it adopt it.
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
    fn the_one_that_does_not_send_asks_and_the_quiet_one_yields() {
        let t0 = Instant::now();
        let (mut a, mut b) = pair();
        // A was never used: it is quiet.
        let claim = b.local_input(t0, true).unwrap();
        assert_eq!(claim, FocusMsg::Claim { epoch: 0 });
        let grant = a.receive(claim, t0 + MS).unwrap();
        assert_eq!(grant, FocusMsg::Grant { epoch: 1 });
        assert_eq!(a.server(), Machine::B);
        assert_eq!(b.receive(grant, t0 + 2 * MS), None);
        assert_eq!(b.server(), Machine::B);
        assert_eq!((a.epoch(), b.epoch()), (1, 1));
        // The one that sends asks for nothing.
        assert_eq!(b.local_input(t0 + 3 * MS, true), None);
    }

    #[test]
    fn with_input_on_both_the_last_wins_after_300_ms_of_quiet() {
        let t0 = Instant::now();
        let (mut a, mut b) = pair();
        a.local_input(t0, true);
        // B is used 100 ms later: A is not quiet, it does not yield.
        let claim = b.local_input(t0 + 100 * MS, true).unwrap();
        assert_eq!(a.receive(claim, t0 + 100 * MS), None);
        // While B continues, it does not ask again before `claim_every`.
        assert_eq!(b.local_input(t0 + 120 * MS, true), None);
        // A at 299 ms from its last input still sends.
        let claim = b.local_input(t0 + 299 * MS, true).unwrap();
        assert_eq!(a.receive(claim, t0 + 299 * MS), None);
        // At 300 ms it yields.
        let claim = b.local_input(t0 + 360 * MS, true).unwrap();
        let grant = a.receive(claim, t0 + 360 * MS).unwrap();
        b.receive(grant, t0 + 361 * MS);
        assert_eq!((a.server(), b.server()), (Machine::B, Machine::B));
    }

    #[test]
    fn without_a_link_the_one_in_use_sends_and_on_return_they_agree() {
        let t0 = Instant::now();
        let (mut a, mut b) = pair();
        assert_eq!(b.local_input(t0, false), None);
        assert_eq!((b.server(), b.epoch()), (Machine::B, 1));
        // A did not find out. When the link returns, each one sends its state.
        let from_b = b.state();
        let reply = a.receive(from_b, t0);
        assert_eq!(reply, None, "B has a new epoch: A adopts it");
        assert_eq!(a.server(), Machine::B);
        // Two decisions without a link in the same epoch: A's wins.
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
    fn an_old_request_is_answered_with_the_state() {
        let t0 = Instant::now();
        let (mut a, mut b) = pair();
        let claim = b.local_input(t0, true).unwrap();
        let grant = a.receive(claim, t0).unwrap();
        // A second request from epoch 0 arrives when A already yielded.
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
        // An old yield changes nothing.
        b.receive(FocusMsg::Grant { epoch: 1 }, t0);
        assert_eq!((b.server(), b.epoch()), (Machine::B, 1));
    }

    #[test]
    fn what_is_declared_wins() {
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

    /// What can happen in a run: physical input on a machine, time that
    /// passes, delivering the next in-flight message toward one, or cutting/restoring the link.
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
        /// Messages in flight toward each machine, in order (the control plane is an
        /// ordered stream).
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
                        // What was going over the dropped link is lost.
                        self.to_a.clear();
                        self.to_b.clear();
                    } else {
                        // On connecting each one sends its state.
                        let (sa, sb) = (self.a.state(), self.b.state());
                        self.send(Machine::A, Some(sa));
                        self.send(Machine::B, Some(sb));
                    }
                }
            }
        }

        /// Delivers everything in flight (and what that generates).
        fn drain(&mut self) {
            for _ in 0..64 {
                if self.to_a.is_empty() && self.to_b.is_empty() {
                    return;
                }
                self.apply(Op::Deliver(Machine::A));
                self.apply(Op::Deliver(Machine::B));
            }
            panic!("the messages never run out: there is a loop");
        }
    }

    proptest! {
        /// Whatever happens (input on both, cuts, delayed messages), with the
        /// link back and what is pending delivered, both agree on who sends.
        #[test]
        fn both_end_up_in_agreement(ops in prop::collection::vec(op(), 0..120)) {
            let mut w = World::new();
            for op in ops {
                w.apply(op);
            }
            w.apply(Op::Link(true));
            w.drain();
            prop_assert_eq!(w.a.server(), w.b.server());
            prop_assert_eq!(w.a.epoch(), w.b.epoch());
        }

        /// If a single machine keeps being used (every 10 ms), at some point it sends,
        /// and once it sends it does not lose it while the other is not used.
        #[test]
        fn the_one_in_use_ends_up_sending(
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
                    Some(_) => prop_assert!(both, "lost the keyboard at step {}", i),
                    None => {}
                }
            }
            // Quiet (300 ms) plus a request.
            prop_assert!(since.is_some_and(|i| i <= 36), "{:?}", since);
        }
    }
}
