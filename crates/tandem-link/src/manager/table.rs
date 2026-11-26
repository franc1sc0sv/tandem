//! Pure state of the manager: which links are healthy and which is the active one.
//! No I/O: time and whether a transaction is in progress arrive as arguments,
//! so it can be tested with properties.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::time::Instant;

use super::{LinkCandidate, LinkKind, ManagerError};

/// Change of the active link. `seq` grows one by one from 0, so a
/// consumer can check that none was lost or reordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkEvent {
    pub seq: u64,
    pub from: Option<LinkKind>,
    pub to: Option<LinkKind>,
    /// If `from` dropped: from its last answer until the change (how long input
    /// may have been left without a path, at most). `None` if `from` was still healthy (an upgrade to a
    /// better link, or the first one).
    pub took: Option<Duration>,
}

/// Health of a link according to the last ping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkStatus {
    pub kind: LinkKind,
    pub addr: SocketAddr,
    /// Round trip of the last ping; `None` if the link is down.
    pub rtt: Option<Duration>,
}

pub(crate) struct LinkTable<H> {
    links: BTreeMap<LinkKind, Slot<H>>,
    active: Option<LinkKind>,
    next_seq: u64,
    /// Links that were removed while healthy: when they return they enter with hysteresis, like those
    /// that drop (a flickering cable does not drag traffic).
    flapped: std::collections::BTreeSet<LinkKind>,
    /// How long a link that already dropped before has to be healthy before going back to it.
    settle: Duration,
    /// Recent drops of each link, kept across its interface going away and coming back
    /// (T-364): every drop within [`FLAP_WINDOW`] of the last one doubles the wait before
    /// going back to it.
    flaps: BTreeMap<LinkKind, Flaps>,
}

/// A link that keeps dropping is only gone back to once it has proved steady for longer and
/// longer, up to this: each switch restarts the input session, so a cable that drops every
/// minute or two must not drag the keyboard and mouse back and forth (T-364).
pub(crate) const MAX_SETTLE: Duration = Duration::from_secs(600);

/// Drops further apart than this are not counted together: the link starts over at `settle`.
pub(crate) const FLAP_WINDOW: Duration = Duration::from_secs(1800);

#[derive(Debug, Clone, Copy, Default)]
struct Flaps {
    count: u32,
    last: Option<Instant>,
}

struct Slot<H> {
    addr: SocketAddr,
    up: Option<(H, Duration)>,
    /// First good ping of the current streak.
    up_since: Option<Instant>,
    /// Last good ping.
    last_ok: Option<Instant>,
    /// It dropped at some point after having been healthy: since then, to
    /// go back to it one has to wait `settle`.
    dropped: bool,
    /// A new address that came while the link was healthy: it is dialled once the link drops,
    /// so a flickering discovery never cuts a connection that works.
    next_addr: Option<SocketAddr>,
}

/// What [`LinkTable::add`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Added {
    /// Already there with that address.
    Unchanged,
    /// The link is healthy over another address: it keeps it, and the new one is used once
    /// it drops.
    Deferred,
    /// A new link, or a new address for one that is down: it has to be pinged from scratch.
    New,
}

impl<H> Slot<H> {
    /// Whether this link can be switched to without fear that it flickers.
    fn settled(&self, now: Instant, settle: Duration) -> bool {
        match self.up_since {
            Some(since) => !self.dropped || now.saturating_duration_since(since) >= settle,
            None => false,
        }
    }
}

impl<H> LinkTable<H> {
    pub(crate) fn new(
        candidates: &[LinkCandidate],
        settle: Duration,
    ) -> Result<Self, ManagerError> {
        if candidates.is_empty() {
            return Err(ManagerError::NoCandidates);
        }
        let mut links = BTreeMap::new();
        for c in candidates {
            let slot = Slot {
                addr: c.addr,
                up: None,
                up_since: None,
                last_ok: None,
                dropped: false,
                next_addr: None,
            };
            if links.insert(c.kind, slot).is_some() {
                return Err(ManagerError::DuplicateLink(c.kind));
            }
        }
        Ok(Self {
            links,
            active: None,
            next_seq: 0,
            flapped: Default::default(),
            settle,
            flaps: BTreeMap::new(),
        })
    }

    /// A new link (its interface appeared) or a new address for one. If it was already there
    /// with that address nothing changes. A healthy link keeps the address it answers on and
    /// takes the new one when it drops ([`Self::readdress`]); one that is down is replaced.
    pub(crate) fn add(&mut self, c: LinkCandidate) -> Added {
        if let Some(slot) = self.links.get_mut(&c.kind) {
            if slot.addr == c.addr {
                slot.next_addr = None;
                return Added::Unchanged;
            }
            if slot.up.is_some() {
                slot.next_addr = Some(c.addr);
                return Added::Deferred;
            }
        }
        let dropped =
            self.flapped.contains(&c.kind) || self.links.get(&c.kind).is_some_and(|s| s.dropped);
        self.links.insert(
            c.kind,
            Slot {
                addr: c.addr,
                up: None,
                up_since: None,
                last_ok: None,
                dropped,
                next_addr: None,
            },
        );
        Added::New
    }

    /// The address a link that is down should be dialled on now, if a new one came while it
    /// was healthy. The link keeps its place and its history.
    pub(crate) fn readdress(&mut self, kind: LinkKind) -> Option<LinkCandidate> {
        let slot = self.links.get_mut(&kind)?;
        if slot.up.is_some() {
            return None;
        }
        let addr = slot.next_addr.take()?;
        slot.addr = addr;
        Some(LinkCandidate { kind, addr })
    }

    /// Removes a link (its interface went down). If it was the active one, the switch to the next comes out
    /// at once, whether or not a transaction is in progress.
    pub(crate) fn remove(&mut self, kind: LinkKind, now: Instant) -> Option<LinkEvent> {
        let slot = self.links.get_mut(&kind)?;
        let was_up = slot.up.is_some();
        if was_up {
            self.flapped.insert(kind);
        }
        // First down, so the event counts how long it was without a path; then it leaves.
        slot.up = None;
        slot.up_since = None;
        if was_up {
            self.note_drop(kind, now);
        }
        let event = self.reevaluate(now, false);
        self.links.remove(&kind);
        event
    }

    /// Applies the result of a ping: `Some((handle, rtt))` if it answered.
    /// Returns the event if the active link changed.
    ///
    /// If the active one drops, the switch is immediate, whether or not there is a transaction
    /// (`held`). Going up to a better link is only done without a transaction in progress and,
    /// if that link already dropped once, after `settle` of consecutive good pings:
    /// that way a flickering cable does not drag traffic back and forth.
    pub(crate) fn report(
        &mut self,
        kind: LinkKind,
        up: Option<(H, Duration)>,
        now: Instant,
        held: bool,
    ) -> Option<LinkEvent> {
        let Some(slot) = self.links.get_mut(&kind) else {
            return self.reevaluate(now, held);
        };
        let mut dropped = false;
        match (&slot.up, &up) {
            (_, Some(_)) if slot.up_since.is_none() => slot.up_since = Some(now),
            (Some(_), None) => {
                slot.up_since = None;
                slot.dropped = true;
                dropped = true;
            }
            _ => {}
        }
        if up.is_some() {
            slot.last_ok = Some(now);
        }
        slot.up = up;
        if dropped {
            self.note_drop(kind, now);
        }
        self.reevaluate(now, held)
    }

    /// A backup's ping is late: it stays healthy, but its streak of good pings starts over,
    /// so the manager does not move to it until it answers steadily for `settle` again.
    pub(crate) fn stalled(&mut self, kind: LinkKind) {
        if let Some(slot) = self.links.get_mut(&kind)
            && slot.up.is_some()
        {
            slot.up_since = None;
            slot.dropped = true;
        }
    }

    /// Chooses the active one again without a new ping (for example, when a
    /// transaction ends).
    pub(crate) fn reevaluate(&mut self, now: Instant, held: bool) -> Option<LinkEvent> {
        let best = self.choose(now, held);
        if best == self.active {
            return None;
        }
        let took = self
            .active
            .and_then(|k| self.links.get(&k))
            .filter(|s| s.up.is_none())
            .and_then(|s| s.last_ok)
            .map(|t| now.saturating_duration_since(t));
        let event = LinkEvent {
            seq: self.next_seq,
            from: self.active,
            to: best,
            took,
        };
        self.next_seq += 1;
        self.active = best;
        Some(event)
    }

    fn note_drop(&mut self, kind: LinkKind, now: Instant) {
        let f = self.flaps.entry(kind).or_default();
        let recent = f
            .last
            .is_some_and(|t| now.saturating_duration_since(t) < FLAP_WINDOW);
        f.count = if recent { f.count.saturating_add(1) } else { 1 };
        f.last = Some(now);
    }

    /// The steady time a link that dropped needs before it is chosen again: `settle`, doubled
    /// for each further drop within [`FLAP_WINDOW`], at most [`MAX_SETTLE`].
    pub(crate) fn settle_for(&self, kind: LinkKind, now: Instant) -> Duration {
        let count = self
            .flaps
            .get(&kind)
            .filter(|f| {
                f.last
                    .is_some_and(|t| now.saturating_duration_since(t) < FLAP_WINDOW)
            })
            .map_or(0, |f| f.count);
        let doublings = count.saturating_sub(1).min(16);
        self.settle
            .saturating_mul(1u32 << doublings)
            .min(MAX_SETTLE.max(self.settle))
    }

    fn choose(&self, now: Instant, held: bool) -> Option<LinkKind> {
        let active = self
            .active
            .filter(|k| self.links.get(k).is_some_and(|s| s.up.is_some()));
        let settled = self
            .links
            .iter()
            .find(|(k, s)| s.settled(now, self.settle_for(**k, now)))
            .map(|(k, _)| *k);
        match active {
            Some(a) if held => Some(a),
            Some(a) => Some(settled.filter(|b| *b < a).unwrap_or(a)),
            // Without a healthy active one: the best stable one and, if none, any healthy one;
            // better a flickering link than none.
            None => settled.or_else(|| {
                self.links
                    .iter()
                    .find(|(_, s)| s.up.is_some())
                    .map(|(k, _)| *k)
            }),
        }
    }

    /// Some link other than `active` is healthy and answering steadily (its ping is not
    /// stalled): the active one can be dropped early, since there is somewhere to go.
    pub(crate) fn steady_backup(&self, active: Option<LinkKind>) -> bool {
        self.links
            .iter()
            .any(|(k, s)| Some(*k) != active && s.up.is_some() && s.up_since.is_some())
    }

    pub(crate) fn active(&self) -> Option<(LinkKind, &H)> {
        let kind = self.active?;
        let (handle, _) = self.links.get(&kind)?.up.as_ref()?;
        Some((kind, handle))
    }

    /// The connection of each healthy link, to use one that is not the active one (to measure its
    /// bandwidth).
    pub(crate) fn handles(&self) -> BTreeMap<LinkKind, H>
    where
        H: Clone,
    {
        self.links
            .iter()
            .filter_map(|(kind, s)| Some((*kind, s.up.as_ref()?.0.clone())))
            .collect()
    }

    pub(crate) fn statuses(&self) -> Vec<LinkStatus> {
        self.links
            .iter()
            .map(|(kind, s)| LinkStatus {
                kind: *kind,
                addr: s.addr,
                rtt: s.up.as_ref().map(|(_, rtt)| *rtt),
            })
            .collect()
    }

    pub(crate) fn candidates(&self) -> impl Iterator<Item = LinkCandidate> + '_ {
        self.links.iter().map(|(kind, s)| LinkCandidate {
            kind: *kind,
            addr: s.addr,
        })
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const RTT: Duration = Duration::from_millis(1);
    const SETTLE: Duration = Duration::from_secs(3);

    fn cand(kind: LinkKind, port: u16) -> LinkCandidate {
        LinkCandidate {
            kind,
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
        }
    }

    fn all() -> LinkTable<u32> {
        LinkTable::new(
            &[
                cand(LinkKind::Wifi, 3),
                cand(LinkKind::Usb4, 1),
                cand(LinkKind::Ethernet, 2),
            ],
            SETTLE,
        )
        .unwrap()
    }

    /// Table with its own clock, to write the tests in milliseconds.
    struct Clock<H> {
        t: LinkTable<H>,
        start: Instant,
        held: bool,
    }

    impl<H> Clock<H> {
        fn new(t: LinkTable<H>) -> Self {
            Self {
                t,
                start: Instant::now(),
                held: false,
            }
        }

        fn at(&mut self, ms: u64, kind: LinkKind, up: Option<H>) -> Option<LinkEvent> {
            let now = self.start + Duration::from_millis(ms);
            self.t.report(kind, up.map(|h| (h, RTT)), now, self.held)
        }

        fn release(&mut self, ms: u64) -> Option<LinkEvent> {
            self.held = false;
            let now = self.start + Duration::from_millis(ms);
            self.t.reevaluate(now, false)
        }

        fn active(&self) -> Option<LinkKind> {
            self.t.active().map(|(k, _)| k)
        }
    }

    #[test]
    fn rejects_an_empty_or_repeated_config() {
        assert!(matches!(
            LinkTable::<u32>::new(&[], SETTLE),
            Err(ManagerError::NoCandidates)
        ));
        assert!(matches!(
            LinkTable::<u32>::new(&[cand(LinkKind::Wifi, 1), cand(LinkKind::Wifi, 2)], SETTLE),
            Err(ManagerError::DuplicateLink(LinkKind::Wifi))
        ));
    }

    #[test]
    fn picks_the_best_healthy_one_and_returns_to_the_preferred_after_settling() {
        let mut c = Clock::new(all());
        assert_eq!(c.active(), None);

        let e = c.at(0, LinkKind::Wifi, Some(3)).unwrap();
        assert_eq!((e.seq, e.from, e.to), (0, None, Some(LinkKind::Wifi)));
        // A link that never dropped enters right away (startup does not wait).
        let e = c.at(1, LinkKind::Usb4, Some(1)).unwrap();
        assert_eq!(e.to, Some(LinkKind::Usb4));
        // A worse link that comes up changes nothing.
        assert_eq!(c.at(2, LinkKind::Ethernet, Some(2)), None);
        // Repeated pings neither.
        assert_eq!(c.at(1000, LinkKind::Usb4, Some(1)), None);

        // The active one dropping switches instantly.
        let e = c.at(2000, LinkKind::Usb4, None).unwrap();
        assert_eq!(
            (e.from, e.to),
            (Some(LinkKind::Usb4), Some(LinkKind::Ethernet))
        );
        assert_eq!(c.t.active(), Some((LinkKind::Ethernet, &2)));

        // It comes back, but not until 3 s of consecutive good pings does it go back.
        for ms in [3000, 4000, 5000, 5999] {
            assert_eq!(c.at(ms, LinkKind::Usb4, Some(10)), None, "a {ms} ms");
        }
        let e = c.at(6000, LinkKind::Usb4, Some(10)).unwrap();
        assert_eq!((e.seq, e.to), (3, Some(LinkKind::Usb4)));
        // The handle is that of the new connection.
        assert_eq!(c.t.active(), Some((LinkKind::Usb4, &10)));
    }

    /// T-102: "switched to Wi-Fi in 0.6 s". The duration goes from the last answer of the one
    /// that dropped; an upgrade without a cut has none.
    #[test]
    fn the_switch_by_a_drop_says_how_long_it_took() {
        let mut c = Clock::new(all());
        let first = c.at(0, LinkKind::Wifi, Some(3)).unwrap();
        assert_eq!(first.took, None);
        let up = c.at(10, LinkKind::Usb4, Some(1)).unwrap();
        assert_eq!((up.to, up.took), (Some(LinkKind::Usb4), None));
        c.at(1010, LinkKind::Usb4, Some(1));
        let down = c.at(1610, LinkKind::Usb4, None).unwrap();
        assert_eq!(
            (down.from, down.to, down.took),
            (
                Some(LinkKind::Usb4),
                Some(LinkKind::Wifi),
                Some(Duration::from_millis(600))
            )
        );
        // Without any path: it is also measured.
        let gone = c.at(2000, LinkKind::Wifi, None).unwrap();
        assert_eq!(
            (gone.to, gone.took),
            (None, Some(Duration::from_millis(2000)))
        );
        assert_eq!(c.t.handles().len(), 0);
        c.at(2100, LinkKind::Ethernet, Some(2));
        assert_eq!(
            c.t.handles().into_iter().collect::<Vec<_>>(),
            [(LinkKind::Ethernet, 2)]
        );
    }

    #[test]
    fn a_usb4_flickering_every_700_ms_does_not_move_the_link() {
        let mut c = Clock::new(all());
        c.at(0, LinkKind::Usb4, Some(1));
        c.at(0, LinkKind::Ethernet, Some(2));
        assert_eq!(c.active(), Some(LinkKind::Usb4));

        // Cut from [k·700, k·700 + 350) ms; one ping per second.
        let mut events = Vec::new();
        for ms in (1000..30_000).step_by(1000) {
            let up = (ms % 700) >= 350;
            events.extend(c.at(ms, LinkKind::Usb4, up.then_some(1)));
            events.extend(c.at(ms, LinkKind::Ethernet, Some(2)));
        }
        let tos: Vec<_> = events.iter().map(|e| e.to).collect();
        assert_eq!(tos, [Some(LinkKind::Ethernet)], "{events:?}");
    }

    /// T-364: a USB4 cable that drops every couple of minutes. Each drop doubles the steady
    /// time it needs before the link goes back to it, so the switches (each one restarts the
    /// input session) thin out instead of happening twice per drop.
    #[test]
    fn a_link_that_keeps_dropping_waits_longer_each_time() {
        let mut c = Clock::new(all());
        c.at(0, LinkKind::Usb4, Some(1));
        c.at(0, LinkKind::Wifi, Some(3));
        let at = |c: &Clock<u32>, ms: u64| c.start + Duration::from_millis(ms);
        let mut ms = 1000;
        let mut waits = Vec::new();
        for _ in 0..5 {
            c.at(ms, LinkKind::Usb4, None);
            assert_eq!(c.active(), Some(LinkKind::Wifi));
            waits.push(c.t.settle_for(LinkKind::Usb4, at(&c, ms)).as_secs());
            ms += 1000;
            // It answers again: it is not chosen before its wait is over.
            let back = ms;
            loop {
                c.at(ms, LinkKind::Usb4, Some(1));
                c.at(ms, LinkKind::Wifi, Some(3));
                if c.active() == Some(LinkKind::Usb4) {
                    break;
                }
                assert!(ms < 1_000_000, "it never went back: {waits:?}");
                ms += 1000;
            }
            assert_eq!(c.active(), Some(LinkKind::Usb4));
            assert!(ms - back >= waits.last().unwrap() * 1000, "{waits:?}");
            ms += 1000;
        }
        assert_eq!(waits, [3, 6, 12, 24, 48]);
    }

    #[test]
    fn the_wait_is_capped_and_forgiven_after_a_quiet_window() {
        let mut t = all();
        let t0 = Instant::now();
        for i in 0..20 {
            t.note_drop(LinkKind::Usb4, t0 + Duration::from_secs(i));
        }
        assert_eq!(
            t.settle_for(LinkKind::Usb4, t0 + Duration::from_secs(20)),
            MAX_SETTLE
        );
        assert_eq!(t.settle_for(LinkKind::Usb4, t0 + FLAP_WINDOW * 2), SETTLE);
        t.note_drop(LinkKind::Usb4, t0 + FLAP_WINDOW * 2);
        assert_eq!(t.settle_for(LinkKind::Usb4, t0 + FLAP_WINDOW * 2), SETTLE);
        assert_eq!(t.settle_for(LinkKind::Wifi, t0), SETTLE);
    }

    /// The interface going away and coming back (the PC's USB4 adapter does) keeps the count.
    #[test]
    fn the_drop_count_survives_the_interface_coming_back() {
        let mut t = all();
        let t0 = Instant::now();
        t.report(LinkKind::Usb4, Some((1, RTT)), t0, false);
        t.remove(LinkKind::Usb4, t0 + Duration::from_secs(1));
        t.add(cand(LinkKind::Usb4, 1));
        t.report(
            LinkKind::Usb4,
            Some((1, RTT)),
            t0 + Duration::from_secs(2),
            false,
        );
        t.report(LinkKind::Usb4, None, t0 + Duration::from_secs(3), false);
        assert_eq!(
            t.settle_for(LinkKind::Usb4, t0 + Duration::from_secs(3)),
            SETTLE * 2
        );
    }

    #[test]
    fn during_a_transaction_it_only_switches_if_the_active_one_drops() {
        let mut c = Clock::new(all());
        c.at(0, LinkKind::Usb4, Some(1));
        c.at(0, LinkKind::Ethernet, Some(2));
        c.at(0, LinkKind::Wifi, Some(3));
        c.at(1000, LinkKind::Usb4, None);
        assert_eq!(c.active(), Some(LinkKind::Ethernet));

        c.held = true;
        // USB4 comes back and settles, but there is a transaction: it stays on Ethernet.
        for ms in (2000..=9000).step_by(1000) {
            assert_eq!(c.at(ms, LinkKind::Usb4, Some(1)), None, "a {ms} ms");
        }
        // When it ends, it goes up right away.
        let e = c.release(9100).unwrap();
        assert_eq!(e.to, Some(LinkKind::Usb4));

        // If the active one drops in the middle of a transaction, it switches anyway.
        c.held = true;
        let e = c.at(10_000, LinkKind::Usb4, None).unwrap();
        assert_eq!(e.to, Some(LinkKind::Ethernet));
        assert_eq!(c.release(10_100), None);
    }

    #[test]
    fn without_an_active_one_prefers_a_stable_link_to_a_flickering_one() {
        let mut c = Clock::new(all());
        c.at(0, LinkKind::Usb4, Some(1));
        c.at(0, LinkKind::Ethernet, Some(2));
        c.at(0, LinkKind::Wifi, Some(3));
        c.at(1000, LinkKind::Usb4, None);
        c.at(1500, LinkKind::Usb4, Some(1));
        let e = c.at(2000, LinkKind::Ethernet, None).unwrap();
        assert_eq!(e.to, Some(LinkKind::Wifi));
        // But with no other option, the flickering one is used.
        let e = c.at(2100, LinkKind::Wifi, None).unwrap();
        assert_eq!(e.to, Some(LinkKind::Usb4));
    }

    #[test]
    fn without_healthy_links_there_is_no_active_one() {
        let mut c = Clock::new(all());
        c.at(0, LinkKind::Ethernet, Some(2));
        let e = c.at(1, LinkKind::Ethernet, None).unwrap();
        assert_eq!((e.from, e.to), (Some(LinkKind::Ethernet), None));
        assert_eq!(c.active(), None);
    }

    #[test]
    fn an_unconfigured_link_is_ignored() {
        let mut c = Clock::new(LinkTable::new(&[cand(LinkKind::Wifi, 3)], SETTLE).unwrap());
        assert_eq!(c.at(0, LinkKind::Usb4, Some(1)), None);
        assert_eq!(c.active(), None);
    }

    #[test]
    fn the_state_lists_each_link_in_priority_order() {
        let mut t = all();
        t.report(
            LinkKind::Ethernet,
            Some((2, Duration::from_millis(3))),
            Instant::now(),
            false,
        );
        let s = t.statuses();
        assert_eq!(
            s.iter().map(|s| s.kind).collect::<Vec<_>>(),
            LinkKind::ALL.to_vec()
        );
        assert_eq!(s[1].rtt, Some(Duration::from_millis(3)));
        assert_eq!(s[0].rtt, None);
        assert_eq!(s[2].addr.port(), 3);
    }

    fn kind() -> impl Strategy<Value = LinkKind> {
        prop::sample::select(LinkKind::ALL.to_vec())
    }

    proptest! {
        /// After any sequence of pings, with or without a transaction in progress:
        /// - there is an active one if and only if some link is healthy, and the active one is healthy;
        /// - with a transaction, the active one only changes if it dropped;
        /// - without a transaction, no link better than the active one has its `settle_for`
        ///   healthy (or never dropped);
        /// - while no link has dropped, the active one is the best healthy one;
        /// - events come out numbered and chained, and the data plane
        ///   exists if and only if the new active one is a cable.
        #[test]
        fn table_invariants(
            configured in prop::sample::subsequence(LinkKind::ALL.to_vec(), 1..=3),
            reports in prop::collection::vec(
                (kind(), any::<bool>(), 0u64..2500, any::<bool>()),
                0..80,
            ),
        ) {
            let cands: Vec<_> = configured.iter().enumerate()
                .map(|(i, k)| cand(*k, i as u16 + 1)).collect();
            let mut t = LinkTable::<u8>::new(&cands, SETTLE).unwrap();
            let mut now = Instant::now();
            let mut up = std::collections::BTreeMap::<LinkKind, Instant>::new();
            let mut ever_dropped = std::collections::BTreeSet::new();
            let mut last: Option<LinkKind> = None;
            let mut seq = 0;
            for (k, ok, step_ms, held) in reports {
                now += Duration::from_millis(step_ms);
                let event = t.report(k, ok.then_some((0, RTT)), now, held);
                if configured.contains(&k) {
                    if ok {
                        up.entry(k).or_insert(now);
                    } else if up.remove(&k).is_some() {
                        ever_dropped.insert(k);
                    }
                }
                let active = t.active().map(|(k, _)| k);
                prop_assert_eq!(active.is_some(), !up.is_empty());
                if let Some(a) = active {
                    prop_assert!(up.contains_key(&a));
                }
                if held && last.is_some_and(|l| up.contains_key(&l)) {
                    prop_assert_eq!(active, last);
                }
                if !held && let Some(a) = active {
                    for (b, since) in up.range(..a) {
                        prop_assert!(
                            ever_dropped.contains(b) && now - *since < t.settle_for(*b, now),
                            "{:?} settled and better than {:?}", b, a
                        );
                    }
                }
                if ever_dropped.is_empty() && !held {
                    prop_assert_eq!(active, up.keys().next().copied());
                }
                match event {
                    Some(e) => {
                        prop_assert_eq!(e.seq, seq);
                        prop_assert_eq!(e.from, last);
                        prop_assert_ne!(e.from, e.to);
                        prop_assert_eq!(e.to, active);
                        seq += 1;
                        last = e.to;
                    }
                    None => prop_assert_eq!(last, active),
                }
            }
        }
    }
}
