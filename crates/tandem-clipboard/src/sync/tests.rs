use std::collections::VecDeque;

use proptest::prelude::*;

use super::*;
use crate::content::Image;

fn cfg() -> SyncConfig {
    SyncConfig {
        inline_max: 64,
        chunk_len: 100,
        max_len: 10_000,
    }
}

/// Without a wall clock: the order is given only by Lamport and the hash.
fn side(focused: bool) -> ClipSync {
    ClipSync::with_clock(cfg(), focused, || 0)
}

fn text(s: &str) -> ClipContent {
    ClipContent::text(s)
}

/// Larger than `inline_max`: goes lazy.
fn big(fill: u8) -> ClipContent {
    ClipContent {
        image: Some(Image {
            width: 10,
            height: 10,
            rgba: vec![fill; 400],
        }),
        ..ClipContent::default()
    }
}

fn sends(actions: Vec<Action>) -> Vec<ClipMsg> {
    actions
        .into_iter()
        .map(|a| match a {
            Action::Send(m) => m,
            Action::Write(c) => panic!("only sends expected, got Write({c:?})"),
        })
        .collect()
}

fn one_send(actions: Vec<Action>) -> ClipMsg {
    let mut m = sends(actions);
    assert_eq!(m.len(), 1, "{m:?}");
    m.remove(0)
}

fn written(actions: &[Action]) -> Vec<ClipContent> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Write(c) => Some(c.clone()),
            Action::Send(_) => None,
        })
        .collect()
}

/// All the chunks of the transfer in progress.
fn drain_chunks(s: &mut ClipSync) -> Vec<ClipMsg> {
    let mut out = Vec::new();
    while let Some(c) = s.next_chunk() {
        out.push(c);
    }
    assert!(!s.is_sending());
    out
}

#[test]
fn a_small_copy_goes_inside_the_announcement_and_the_other_writes_it() {
    let mut mac = side(false);
    let mut pc = side(false);
    let offer = one_send(mac.local_changed(Some(text("hello"))));
    let ClipMsg::Offer { content, len, .. } = &offer else {
        panic!("{offer:?}")
    };
    assert_eq!(content.as_ref(), Some(&text("hello")));
    assert!(*len > 0);
    // Even if the PC does not have focus: small content is written on arrival.
    let acts = pc.on_message(offer);
    assert_eq!(acts, vec![Action::Write(text("hello"))]);
    assert_eq!(pc.stats().written, 1);
    assert_eq!(mac.stats().offers_sent, 1);
}

#[test]
fn the_own_write_is_not_announced_back() {
    let mut mac = side(false);
    let mut pc = side(false);
    let offer = one_send(mac.local_changed(Some(text("echo"))));
    assert_eq!(written(&pc.on_message(offer)), vec![text("echo")]);
    // The system reports that the PC's clipboard changed: it is what we wrote.
    assert!(pc.local_changed(Some(text("echo"))).is_empty());
    assert_eq!(pc.stats().offers_sent, 0);
    // A change that changes nothing, neither.
    assert!(mac.local_changed(Some(text("echo"))).is_empty());
    assert_eq!(mac.stats().offers_sent, 1);
}

#[test]
fn an_offer_with_what_we_already_have_is_not_written() {
    let mut mac = side(true);
    let mut pc = side(true);
    let _ = pc.local_changed(Some(big(3)));
    let offer = one_send(mac.local_changed(Some(big(3))));
    assert!(pc.on_message(offer).is_empty());
    assert!(!pc.has_pending());
    assert_eq!(pc.stats().written, 0);
}

#[test]
fn a_large_copy_is_transferred_on_taking_focus() {
    let mut mac = side(true);
    let mut pc = side(false);
    let content = big(9);
    let offer = one_send(mac.local_changed(Some(content.clone())));
    let ClipMsg::Offer {
        id,
        content: None,
        len,
        ..
    } = offer
    else {
        panic!("{offer:?}")
    };
    // Without focus it is only noted.
    assert!(pc.on_message(offer).is_empty());
    assert!(pc.has_pending());

    let request = one_send(pc.set_focus(true));
    assert_eq!(request, ClipMsg::Request { id });
    // Getting focus again does not request it twice.
    assert!(pc.set_focus(true).is_empty());

    assert!(mac.on_message(request).is_empty());
    assert!(mac.is_sending());
    let chunks = drain_chunks(&mut mac);
    assert!(chunks.len() > 1);
    let mut total = 0;
    let mut acts = Vec::new();
    for (i, c) in chunks.iter().enumerate() {
        let ClipMsg::Chunk { data, last, .. } = c else {
            panic!("{c:?}")
        };
        assert!(data.len() <= cfg().chunk_len);
        assert_eq!(*last, i == chunks.len() - 1);
        total += data.len();
        acts.extend(pc.on_message(c.clone()));
    }
    assert_eq!(total as u64, len);
    assert_eq!(written(&acts), vec![content.clone()]);
    assert!(!pc.has_pending());
    // And the write does not come back as an echo.
    assert!(pc.local_changed(Some(content)).is_empty());
}

#[test]
fn with_focus_it_is_requested_as_soon_as_the_announcement_arrives() {
    let mut mac = side(false);
    let mut pc = side(true);
    let offer = one_send(mac.local_changed(Some(big(1))));
    let ClipMsg::Offer { id, .. } = offer else {
        panic!()
    };
    assert_eq!(one_send(pc.on_message(offer)), ClipMsg::Request { id });
}

#[test]
fn a_request_for_an_old_copy_answers_unavailable() {
    let mut mac = side(false);
    let mut pc = side(false);
    let old = one_send(mac.local_changed(Some(big(1))));
    let ClipMsg::Offer { id: old_id, .. } = old else {
        panic!()
    };
    let _ = pc.on_message(old);
    // The Mac copies something else before the new announcement reaches the PC.
    let newer = one_send(mac.local_changed(Some(text("new"))));
    let request = one_send(pc.set_focus(true));
    let reply = one_send(mac.on_message(request));
    assert_eq!(reply, ClipMsg::Unavailable { id: old_id });
    assert!(!mac.is_sending());
    assert!(pc.on_message(reply).is_empty());
    assert!(!pc.has_pending());
    assert_eq!(written(&pc.on_message(newer)), vec![text("new")]);
}

#[test]
fn copying_midway_through_a_transfer_cuts_it() {
    let mut mac = side(false);
    let mut pc = side(true);
    let offer = one_send(mac.local_changed(Some(big(1))));
    let request = one_send(pc.on_message(offer));
    let _ = mac.on_message(request);
    let first = mac.next_chunk().unwrap();
    let _ = pc.on_message(first);
    let newer = one_send(mac.local_changed(Some(big(2))));
    assert!(!mac.is_sending());
    assert_eq!(mac.next_chunk(), None);
    // The new announcement replaces the other side's half-done transfer.
    let ClipMsg::Offer { id, .. } = newer else {
        panic!()
    };
    assert_eq!(one_send(pc.on_message(newer)), ClipMsg::Request { id });
    let _ = mac.on_message(ClipMsg::Request { id });
    let mut acts = Vec::new();
    for c in drain_chunks(&mut mac) {
        acts.extend(pc.on_message(c));
    }
    assert_eq!(written(&acts), vec![big(2)]);
}

#[test]
fn a_local_copy_replaces_what_is_pending_from_the_other_side() {
    let mut mac = side(false);
    let mut pc = side(false);
    let offer = one_send(mac.local_changed(Some(big(1))));
    let _ = pc.on_message(offer);
    assert!(pc.has_pending());
    let _ = pc.local_changed(Some(text("mine")));
    assert!(!pc.has_pending());
    // On taking focus the old one is not requested.
    assert!(pc.set_focus(true).is_empty());
}

#[test]
fn chunks_of_something_not_requested_are_ignored() {
    let mut pc = side(true);
    let acts = pc.on_message(ClipMsg::Chunk {
        id: 5,
        data: vec![1, 2, 3],
        last: true,
    });
    assert!(acts.is_empty());
    assert_eq!(pc.stats(), SyncStats::default());
}

/// Carries a large offer until it has the requested transfer.
fn requested(mac: &mut ClipSync, pc: &mut ClipSync, content: ClipContent) -> (u64, Vec<u8>) {
    let offer = one_send(mac.local_changed(Some(content.clone())));
    let ClipMsg::Offer { id, .. } = offer else {
        panic!()
    };
    let _ = one_send(pc.on_message(offer));
    (id, tandem_proto::encode(&content).unwrap())
}

#[test]
fn a_corrupt_transfer_is_discarded() {
    let mut mac = side(false);
    let mut pc = side(true);
    let (id, mut bytes) = requested(&mut mac, &mut pc, big(4));
    let n = bytes.len();
    bytes[n - 1] ^= 0xFF;
    let acts = pc.on_message(ClipMsg::Chunk {
        id,
        data: bytes,
        last: true,
    });
    assert!(acts.is_empty());
    assert_eq!(pc.stats().rejected, 1);
    assert!(!pc.has_pending());
}

#[test]
fn a_transfer_longer_or_shorter_than_announced_is_discarded() {
    let mut mac = side(false);
    let mut pc = side(true);
    let (id, mut bytes) = requested(&mut mac, &mut pc, big(4));
    bytes.push(0);
    assert!(
        pc.on_message(ClipMsg::Chunk {
            id,
            data: bytes,
            last: false,
        })
        .is_empty()
    );
    assert_eq!(pc.stats().rejected, 1);
    assert!(!pc.has_pending());

    let (id, mut bytes) = requested(&mut mac, &mut pc, big(5));
    bytes.pop();
    assert!(
        pc.on_message(ClipMsg::Chunk {
            id,
            data: bytes,
            last: true,
        })
        .is_empty()
    );
    assert_eq!(pc.stats().rejected, 2);
    assert_eq!(pc.stats().written, 0);
}

#[test]
fn invalid_offers_are_discarded() {
    let mut pc = side(true);
    // Content that is not that of its hash.
    let acts = pc.on_message(ClipMsg::Offer {
        id: 1,
        hash: text("a").hash(),
        len: 3,
        content: Some(text("b")),
    });
    assert!(acts.is_empty());
    // Larger than the maximum.
    let acts = pc.on_message(ClipMsg::Offer {
        id: 2,
        hash: text("a").hash(),
        len: cfg().max_len as u64 + 1,
        content: None,
    });
    assert!(acts.is_empty());
    assert!(!pc.has_pending());
    assert_eq!(pc.stats().rejected, 2);
    assert_eq!(pc.stats().written, 0);
}

#[test]
fn a_copy_that_exceeds_the_maximum_is_not_announced() {
    let mut mac = side(false);
    let huge = ClipContent {
        image: Some(Image {
            width: 100,
            height: 100,
            rgba: vec![0; 40_000],
        }),
        ..ClipContent::default()
    };
    assert!(mac.local_changed(Some(huge)).is_empty());
    assert_eq!(mac.stats().offers_sent, 0);
}

#[test]
fn an_empty_clipboard_or_one_without_formats_is_not_announced_and_cuts_what_was_offered() {
    let mut mac = side(false);
    let offer = one_send(mac.local_changed(Some(big(1))));
    let ClipMsg::Offer { id, .. } = offer else {
        panic!()
    };
    // The user copies files: they are not synchronized.
    assert!(mac.local_changed(None).is_empty());
    assert_eq!(
        one_send(mac.on_message(ClipMsg::Request { id })),
        ClipMsg::Unavailable { id }
    );
}

#[test]
fn crossed_copies_end_up_equal_on_both_sides() {
    for (a, b) in [
        (text("x"), text("y")),
        (big(1), big(2)),
        (text("x"), big(2)),
    ] {
        let mut mac = side(true);
        let mut pc = side(true);
        // Both copies go out before the other's arrives.
        let from_mac = one_send(mac.local_changed(Some(a.clone())));
        let from_pc = one_send(pc.local_changed(Some(b.clone())));
        let winner = if a.hash() > b.hash() { &a } else { &b };
        let mut model = Model::new(mac, pc);
        model.to_pc.push_back(from_mac);
        model.to_mac.push_back(from_pc);
        model.mac.clip = Some(a.clone());
        model.pc.clip = Some(b.clone());
        model.settle(1000);
        assert_eq!(model.mac.clip.as_ref(), Some(winner));
        assert_eq!(model.pc.clip.as_ref(), Some(winner));
    }
}

#[test]
fn a_copy_made_after_seeing_the_other_one_always_wins() {
    // Whatever the order of the hashes, the copy the other saw is not crossed.
    for (a, b) in [(text("x"), text("y")), (text("y"), text("x"))] {
        let mut mac = side(false);
        let mut pc = side(false);
        let offer = one_send(mac.local_changed(Some(a.clone())));
        let _ = pc.on_message(offer);
        let reply = one_send(pc.local_changed(Some(b.clone())));
        assert_eq!(written(&mac.on_message(reply)), vec![b]);
    }
}

#[test]
fn on_start_what_was_copied_without_a_link_is_announced_and_the_current_one_is_re_announced() {
    let mut mac = side(false);
    // No changes and nothing copied: nothing.
    assert!(mac.start(None).is_empty());
    // Copied while there was no link.
    let first = one_send(mac.start(Some(text("a"))));
    let ClipMsg::Offer { id, .. } = first else {
        panic!()
    };
    // New link, same content: the same copy is announced again.
    let again = one_send(mac.start(Some(text("a"))));
    assert!(matches!(again, ClipMsg::Offer { id: i, content: Some(_), .. } if i == id));
    // Without being able to read the clipboard, either.
    assert_eq!(one_send(mac.resume()), again);
    // If the current one came from the other side, it is not ours: it is not announced.
    let mut pc = side(false);
    let _ = pc.on_message(again);
    assert!(pc.start(Some(text("a"))).is_empty());
}

#[test]
fn what_is_pending_survives_the_link_change_and_is_requested_again() {
    let mut mac = side(false);
    let mut pc = side(true);
    let offer = one_send(mac.local_changed(Some(big(7))));
    let request = one_send(pc.on_message(offer));
    let _ = mac.on_message(request.clone());
    let first = mac.next_chunk().unwrap();
    let _ = pc.on_message(first);
    // The link drops midway through the transfer.
    let mac_start = sends(mac.start(Some(big(7))));
    assert!(!mac.is_sending());
    let pc_start = sends(pc.start(None));
    assert_eq!(pc_start, vec![request.clone()]);
    // The Mac's re-announcement arrives and does not duplicate the request... it renews it.
    let mut model = Model::new(mac, pc);
    model.mac.clip = Some(big(7));
    model.to_pc.extend(mac_start);
    model.to_mac.extend(pc_start);
    model.settle(1000);
    assert_eq!(model.pc.clip, Some(big(7)));
    assert_eq!(model.pc.sync.stats().written, 1);
}

#[test]
fn the_copies_of_a_restarted_agent_win_by_the_wall_clock() {
    let mut mac = side(false);
    let mut pc = side(false);
    let offer = one_send(mac.local_changed(Some(text("x"))));
    let _ = pc.on_message(offer);
    let offer = one_send(pc.local_changed(Some(text("y"))));
    let _ = mac.on_message(offer);
    // The PC restarts and forgets everything: without a wall clock, its new copy
    // would have a lower id than the current one and the Mac would ignore it.
    let mut forgetful = side(false);
    let stale = one_send(forgetful.local_changed(Some(text("z"))));
    assert!(mac.clone().on_message(stale).is_empty());
    let mut restarted = ClipSync::with_clock(cfg(), false, || 1_000_000);
    let offer = one_send(restarted.local_changed(Some(text("z"))));
    assert_eq!(written(&mac.on_message(offer)), vec![text("z")]);
}

#[test]
fn chunk_len_and_inline_fit_the_frame() {
    let s = ClipSync::new(
        SyncConfig {
            inline_max: usize::MAX,
            chunk_len: 0,
            max_len: 1,
        },
        false,
    );
    assert!(s.cfg.inline_max < MAX_FRAME_LEN);
    assert_eq!(s.cfg.chunk_len, 1);
}

// --- Two-agent model with queues, for the properties ---

struct Agent {
    sync: ClipSync,
    /// What the system clipboard holds.
    clip: Option<ClipContent>,
}

struct Model {
    mac: Agent,
    pc: Agent,
    to_mac: VecDeque<ClipMsg>,
    to_pc: VecDeque<ClipMsg>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    Mac,
    Pc,
}

impl Model {
    fn new(mac: ClipSync, pc: ClipSync) -> Self {
        Self {
            mac: Agent {
                sync: mac,
                clip: None,
            },
            pc: Agent {
                sync: pc,
                clip: None,
            },
            to_mac: VecDeque::new(),
            to_pc: VecDeque::new(),
        }
    }

    fn parts(&mut self, who: Who) -> (&mut Agent, &mut VecDeque<ClipMsg>) {
        match who {
            Who::Mac => (&mut self.mac, &mut self.to_pc),
            Who::Pc => (&mut self.pc, &mut self.to_mac),
        }
    }

    /// Applies actions like the driver: a `Write` changes the clipboard and the
    /// system reports the change, which has to go out without an announcement (no echo).
    fn apply(&mut self, who: Who, actions: Vec<Action>) {
        let (agent, outbox) = self.parts(who);
        for a in actions {
            match a {
                Action::Send(m) => outbox.push_back(m),
                Action::Write(c) => {
                    agent.clip = Some(c.clone());
                    let echo = agent.sync.local_changed(Some(c));
                    assert!(echo.is_empty(), "echo: {echo:?}");
                }
            }
        }
        // Chunks go out one at a time, like in the driver.
        while let Some(c) = agent.sync.next_chunk() {
            outbox.push_back(c);
        }
    }

    fn copy(&mut self, who: Who, content: ClipContent) {
        let (agent, _) = self.parts(who);
        agent.clip = Some(content.clone());
        let acts = agent.sync.local_changed(Some(content));
        self.apply(who, acts);
    }

    fn focus(&mut self, who: Who, focused: bool) {
        let (agent, _) = self.parts(who);
        let acts = agent.sync.set_focus(focused);
        self.apply(who, acts);
    }

    /// Delivers the next message to `who`; `false` if there was none.
    fn deliver(&mut self, who: Who) -> bool {
        let msg = match who {
            Who::Mac => self.to_mac.pop_front(),
            Who::Pc => self.to_pc.pop_front(),
        };
        let Some(msg) = msg else { return false };
        let acts = match who {
            Who::Mac => self.mac.sync.on_message(msg),
            Who::Pc => self.pc.sync.on_message(msg),
        };
        self.apply(who, acts);
        true
    }

    /// New link: what was in flight is lost.
    fn restart(&mut self) {
        self.to_mac.clear();
        self.to_pc.clear();
        for who in [Who::Mac, Who::Pc] {
            let (agent, _) = self.parts(who);
            let clip = agent.clip.clone();
            let acts = agent.sync.start(clip);
            self.apply(who, acts);
        }
    }

    /// Delivers everything; the number of messages has to be finite (no infinite
    /// echo).
    fn settle(&mut self, max: usize) {
        let mut n = 0;
        while self.deliver(Who::Mac) | self.deliver(Who::Pc) {
            n += 1;
            assert!(n < max, "the messages never end");
        }
    }
}

#[derive(Clone, Debug)]
enum Op {
    Copy(Who, ClipContent),
    Focus(Who),
    Deliver(Who),
    Restart,
}

fn who() -> impl Strategy<Value = Who> {
    prop_oneof![Just(Who::Mac), Just(Who::Pc)]
}

fn clip() -> impl Strategy<Value = ClipContent> {
    // Few values, so they repeat (echo, equal offers).
    prop_oneof![
        (0u8..4).prop_map(|i| text(&format!("t{i}"))),
        (0u8..3).prop_map(big),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (who(), clip()).prop_map(|(w, c)| Op::Copy(w, c)),
        2 => who().prop_map(Op::Focus),
        6 => who().prop_map(Op::Deliver),
        1 => Just(Op::Restart),
    ]
}

proptest! {
    /// Any sequence of copies, focus changes, partial deliveries and
    /// link changes (losing what was in flight) ends with both
    /// clipboards equal once both have had focus, without echo and with a
    /// finite number of messages.
    #[test]
    fn both_sides_converge_without_echo(ops in proptest::collection::vec(op(), 1..60)) {
        let mut m = Model::new(side(false), side(true));
        let mut copied = false;
        for op in ops {
            match op {
                Op::Copy(w, c) => {
                    copied = true;
                    m.copy(w, c);
                }
                Op::Focus(w) => {
                    // Focus is on a single machine.
                    let other = if w == Who::Mac { Who::Pc } else { Who::Mac };
                    m.focus(other, false);
                    m.focus(w, true);
                }
                Op::Deliver(w) => {
                    m.deliver(w);
                }
                Op::Restart => m.restart(),
            }
        }
        m.settle(10_000);
        for w in [Who::Mac, Who::Pc, Who::Mac] {
            let other = if w == Who::Mac { Who::Pc } else { Who::Mac };
            m.focus(other, false);
            m.focus(w, true);
            m.settle(10_000);
        }
        prop_assert_eq!(&m.mac.clip, &m.pc.clip);
        prop_assert_eq!(m.mac.clip.is_some(), copied);
        prop_assert_eq!(m.mac.sync.stats().rejected + m.pc.sync.stats().rejected, 0);
    }
}
