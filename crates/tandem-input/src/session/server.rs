//! Server state machine: the machine where the keyboard and mouse are.
//!
//! It receives everything its system captures and decides, with the [`Layout`], when the
//! cursor crosses to the other machine. While the focus is remote it tracks the client
//! cursor's position (it is the authority: the client only follows it) and translates
//! input into messages. It does no I/O: it returns [`ServerOutput`] and the driver
//! ([`super::quic`]) executes them in order.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tandem_core::Machine;

use super::cursor::Crossing;
use super::msg::{MotionDatagram, StreamMsg};
use crate::event::{Change, HidUsage, InputEvent, KeyState, Pressed, Scroll};
use crate::geometry::{Layout, Motion, Point};

/// What the driver has to do, in this order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ServerOutput {
    Stream(StreamMsg),
    Datagram(MotionDatagram),
    /// Swallow local input and freeze/hide the cursor (`true`) or give it back.
    Grab(bool),
    /// Take the local cursor to this position (when coming back from the other machine).
    Warp(Point),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Focus {
    Local,
    /// Position of the client's cursor, in its native coordinates.
    Remote {
        pos: Point,
    },
}

/// Accumulated movement and scroll that has not gone out yet.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Pending {
    dx: f64,
    dy: f64,
    scroll: Option<Scroll>,
    /// Stamp of the first accumulated event.
    t_us: Option<u64>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.dx == 0.0 && self.dy == 0.0 && self.scroll.is_none()
    }
}

#[derive(Debug, Clone)]
pub struct ServerSession {
    machine: Machine,
    layout: Layout,
    /// Local cursor; while the focus is remote it stays frozen here.
    local: Point,
    focus: Focus,
    /// Everything pressed on the physical keyboard and mouse, regardless of focus:
    /// on entering, the modifiers the user already holds are sent.
    pressed: Pressed,
    seq: u64,
    pending: Pending,
    resync_every: u32,
    since_resync: u32,
    /// The client received over the stream the exact position after the last
    /// movement.
    synced: bool,
    /// The focus is remote because this machine had no visible screen (not because the
    /// cursor crossed an edge): it goes back as soon as one is visible again.
    settled: bool,
    /// Shortcut that locks the cursor on the machine where it is; empty = no shortcut.
    lock_shortcut: Vec<HidUsage>,
    locked: bool,
    /// Whether the cursor crosses to the other machine with a button pressed (dragging). By
    /// default no, like Deskflow: a drag that reaches the edge stays on its machine.
    drag_across: bool,
    /// A system file drag is under way (T-121): it crosses even if `drag_across`
    /// is off, and only for as long as it lasts.
    file_drag: bool,
    /// The same, but written by the agent from another task (the system detector).
    file_drag_flag: Option<Arc<AtomicBool>>,
    /// Stamp of the event being processed.
    now_us: u64,
    /// The edge crossings since the last time the driver collected them
    /// ([`Self::take_crossings`]); for the live cursor (T-128).
    crossings: Vec<Crossing>,
}

impl ServerSession {
    /// `resync_every`: how many datagrams apart one goes with the absolute position
    /// (in addition to the one that goes out when the mouse stops). `0` = on all.
    pub fn new(machine: Machine, layout: Layout, cursor: Point, resync_every: u32) -> Self {
        Self {
            machine,
            layout,
            local: cursor,
            focus: Focus::Local,
            pressed: Pressed::new(),
            seq: 0,
            pending: Pending::default(),
            resync_every,
            since_resync: 0,
            synced: true,
            settled: false,
            lock_shortcut: Vec::new(),
            locked: false,
            drag_across: false,
            file_drag: false,
            file_drag_flag: None,
            now_us: 0,
            crossings: Vec::new(),
        }
    }

    /// Allows crossing to the other machine with a button pressed.
    pub fn with_drag_across(mut self, allow: bool) -> Self {
        self.drag_across = allow;
        self
    }

    /// The agent detected (or no longer) a file drag on the source machine.
    pub fn set_file_drag(&mut self, active: bool) {
        self.file_drag = active;
    }

    /// Like [`Self::set_file_drag`], but governed by `flag`: the agent turns it on from
    /// another task without going through the hot path (here it is only read, relaxed).
    pub fn with_file_drag_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.file_drag_flag = Some(flag);
        self
    }

    /// Some mouse button is pressed.
    pub fn button_held(&self) -> bool {
        self.pressed.any_button()
    }

    fn file_drag_active(&self) -> bool {
        self.file_drag
            || self
                .file_drag_flag
                .as_ref()
                .is_some_and(|f| f.load(Ordering::Relaxed))
    }

    /// The cursor cannot cross now: locked by the shortcut or dragging.
    fn held_in_place(&self) -> bool {
        self.locked || (!self.drag_across && !self.file_drag_active() && self.pressed.any_button())
    }

    /// Keyboard shortcut (all the keys at once) that locks and unlocks the
    /// cursor on the machine where it is. The keys follow their normal path.
    pub fn with_lock_shortcut(mut self, keys: Vec<HidUsage>) -> Self {
        self.lock_shortcut = keys;
        self
    }

    /// Whether the cursor is locked: it does not cross to the other machine in either direction.
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    pub fn set_locked(&mut self, locked: bool) {
        self.locked = locked;
    }

    /// With the key just pressed, is the shortcut complete?
    fn shortcut_hit(&self, usage: HidUsage) -> bool {
        let canon = canonical_key(usage);
        !self.lock_shortcut.is_empty()
            && self
                .lock_shortcut
                .iter()
                .any(|k| canonical_key(*k) == canon)
            && self.lock_shortcut.iter().all(|k| {
                self.pressed.is_key_pressed(*k)
                    || alias_key(*k).is_some_and(|a| self.pressed.is_key_pressed(a))
            })
    }

    /// Keeps count of what is pressed and of the lock shortcut.
    fn record(&mut self, event: &InputEvent) {
        // Automatic repeat arrives as another `Pressed`: only the
        // first counts, otherwise holding the shortcut would toggle the lock nonstop.
        let change = self.pressed.record(event);
        if change == Change::Changed
            && let InputEvent::Key {
                usage,
                state: KeyState::Pressed,
            } = *event
            && self.shortcut_hit(usage)
        {
            self.locked = !self.locked;
            tracing::info!(locked = self.locked, "cursor lock shortcut");
        }
    }

    pub fn machine(&self) -> Machine {
        self.machine
    }

    /// Whether the focus is on the other machine.
    pub fn is_remote(&self) -> bool {
        matches!(self.focus, Focus::Remote { .. })
    }

    /// Position of the client's cursor, if the focus is remote.
    pub fn remote_cursor(&self) -> Option<Point> {
        match self.focus {
            Focus::Remote { pos } => Some(pos),
            Focus::Local => None,
        }
    }

    pub fn local_cursor(&self) -> Point {
        self.local
    }

    /// The crossings that happened since the last call, in order. Empty almost always: it
    /// costs nothing on the hot path.
    pub fn take_crossings(&mut self) -> Vec<Crossing> {
        if self.crossings.is_empty() {
            return Vec::new();
        }
        std::mem::take(&mut self.crossings)
    }

    /// The id of this machine's screen that contains `p` (the nearest if it falls in a
    /// gap); empty if its screens are not known.
    pub fn screen_id(&self, p: Point) -> String {
        self.layout
            .screen_at(self.machine, p)
            .map(|s| s.id.0.clone())
            .unwrap_or_default()
    }

    /// Records that the cursor crossed from `from` to the other machine through the edge facing it.
    fn crossed(&mut self, from: Machine) {
        // Without an edge (the arrangement changed in the meantime) there is no way it crossed.
        if let Some(edge) = self.layout.facing(from) {
            self.crossings.push(Crossing {
                from,
                to: from.other(),
                edge,
            });
        }
    }

    /// Processes a captured event. Movements are in local native
    /// units; a `MotionAbsolute` is the local cursor's real position, which the
    /// capture reports to correct tracking.
    pub fn handle(&mut self, event: InputEvent) -> Vec<ServerOutput> {
        self.handle_at(event, 0)
    }

    /// Like [`Self::handle`], with the capture timestamp ([`super::latency::now_us`])
    /// that travels in the messages to measure latency.
    pub fn handle_at(&mut self, event: InputEvent, t_us: u64) -> Vec<ServerOutput> {
        self.now_us = t_us;
        let mut out = Vec::new();
        match self.focus {
            Focus::Local => self.handle_local(event, &mut out),
            Focus::Remote { pos } => self.handle_remote(event, pos, &mut out),
        }
        out
    }

    /// Brings the state up to date with an event captured when there was no session (without a
    /// link, or between one session and the next). It only remembers what is pressed and
    /// the local cursor's real position: old movement and scroll are
    /// discarded, because applying them late would make it cross without the user asking.
    pub fn catch_up(&mut self, event: InputEvent) {
        match event {
            InputEvent::Key { .. } | InputEvent::Button { .. } => self.record(&event),
            InputEvent::MotionAbsolute { x, y } if !self.is_remote() => {
                self.local = Point::new(x, y);
            }
            InputEvent::MotionAbsolute { .. }
            | InputEvent::MotionRelative { .. }
            | InputEvent::Scroll(_)
            | InputEvent::Modifiers(_) => {}
        }
    }

    fn handle_local(&mut self, event: InputEvent, out: &mut Vec<ServerOutput>) {
        match event {
            InputEvent::Key { .. } | InputEvent::Button { .. } => self.record(&event),
            InputEvent::MotionAbsolute { x, y } => self.local = Point::new(x, y),
            InputEvent::MotionRelative { dx, dy } if self.held_in_place() => {
                self.local = self.layout.step_within(self.machine, self.local, dx, dy);
            }
            InputEvent::MotionRelative { dx, dy } => {
                match self.layout.step(self.machine, self.local, dx, dy) {
                    Motion::Local(p) => self.local = p,
                    Motion::Cross { to, pos } if to != self.machine => self.enter(pos, out),
                    // It should not happen: crossing "to itself" is staying.
                    Motion::Cross { pos, .. } => self.local = pos,
                }
            }
            InputEvent::Scroll(_) | InputEvent::Modifiers(_) => {}
        }
    }

    fn handle_remote(&mut self, event: InputEvent, pos: Point, out: &mut Vec<ServerOutput>) {
        match event {
            InputEvent::Key { usage, state } => {
                self.record(&event);
                // It is always forwarded: the client decides whether anything changes (a key
                // pressed before entering is not pressed over there).
                out.push(ServerOutput::Stream(StreamMsg::Key {
                    usage,
                    state,
                    t_us: self.now_us,
                }));
            }
            InputEvent::Button { button, state } => {
                self.record(&event);
                out.extend(self.flush().map(ServerOutput::Datagram));
                out.push(ServerOutput::Stream(StreamMsg::Button {
                    button,
                    state,
                    seq: self.seq,
                    pos,
                    t_us: self.now_us,
                }));
            }
            InputEvent::MotionRelative { dx, dy } => self.move_remote(pos, dx, dy, out),
            InputEvent::Scroll(s) => {
                if let Some(merged) = merge_scroll(self.pending.scroll, s) {
                    self.pending.scroll = Some(merged);
                } else {
                    // Wheel and pixels are not added together: the previous one goes out first.
                    out.extend(self.flush().map(ServerOutput::Datagram));
                    self.pending.scroll = Some(s);
                }
                self.pending.t_us.get_or_insert(self.now_us);
            }
            // The local cursor's real position does not change while it is frozen.
            InputEvent::MotionAbsolute { .. } | InputEvent::Modifiers(_) => {}
        }
    }

    fn move_remote(&mut self, pos: Point, dx: f64, dy: f64, out: &mut Vec<ServerOutput>) {
        let remote = self.machine.other();
        // Local native → logical → remote native: speed feels the same
        // on screens of different DPI.
        let (lx, ly) = match self.layout.screen_at(self.machine, self.local) {
            Some(s) => s.to_logical(dx, dy),
            None => (dx, dy),
        };
        let (rx, ry) = match self.layout.screen_at(remote, pos) {
            Some(s) => s.to_native(lx, ly),
            None => (lx, ly),
        };
        let motion = if self.held_in_place() {
            Motion::Local(self.layout.step_within(remote, pos, rx, ry))
        } else {
            self.layout.step(remote, pos, rx, ry)
        };
        match motion {
            Motion::Local(p) => {
                // The delta that travels is native to the client and is measured over what
                // really moved (against an edge without a crossing, nothing): added up, the deltas
                // take the client exactly to `p`.
                if p != pos {
                    self.pending.dx += p.x - pos.x;
                    self.pending.dy += p.y - pos.y;
                    self.pending.t_us.get_or_insert(self.now_us);
                }
                self.focus = Focus::Remote { pos: p };
            }
            Motion::Cross { to, pos: back } if to == self.machine => self.leave(back, out),
            Motion::Cross { pos: p, .. } => self.focus = Focus::Remote { pos: p },
        }
    }

    fn enter(&mut self, pos: Point, out: &mut Vec<ServerOutput>) {
        self.crossed(self.machine);
        self.settled = false;
        self.focus = Focus::Remote { pos };
        self.pending = Pending::default();
        self.since_resync = 0;
        self.synced = true;
        out.push(ServerOutput::Grab(true));
        out.push(ServerOutput::Stream(StreamMsg::Enter {
            seq: self.seq,
            pos,
            modifiers: self.pressed.modifiers(),
        }));
    }

    fn leave(&mut self, back: Point, out: &mut Vec<ServerOutput>) {
        self.crossed(self.machine.other());
        self.settled = false;
        self.focus = Focus::Local;
        // What was pending was for the client, which now releases everything.
        self.pending = Pending::default();
        self.local = back;
        out.push(ServerOutput::Stream(StreamMsg::Leave));
        out.push(ServerOutput::Grab(false));
        out.push(ServerOutput::Warp(back));
    }

    /// Datagram with what has accumulated, if there is anything. The driver calls it after
    /// processing each batch of events: what arrived together goes out together.
    pub fn flush(&mut self) -> Option<MotionDatagram> {
        let Focus::Remote { pos } = self.focus else {
            return None;
        };
        if self.pending.is_empty() {
            return None;
        }
        let p = std::mem::take(&mut self.pending);
        self.seq += 1;
        self.since_resync += 1;
        let resync = (self.since_resync >= self.resync_every).then(|| {
            self.since_resync = 0;
            pos
        });
        self.synced = false;
        Some(MotionDatagram {
            seq: self.seq,
            dx: p.dx,
            dy: p.dy,
            scroll: p.scroll,
            resync,
            t_us: p.t_us.unwrap_or(self.now_us),
        })
    }

    /// Whether there was movement since the last exact position the client received.
    pub fn needs_resync(&self) -> bool {
        self.is_remote() && !(self.synced && self.pending.is_empty())
    }

    /// Exact position over the reliable stream. The driver asks for it when the mouse
    /// has been still for a while: if datagrams were lost (even the periodic ones with
    /// the position), the client's cursor still ends up where it should.
    pub fn resync(&mut self) -> Vec<ServerOutput> {
        let Focus::Remote { pos } = self.focus else {
            return Vec::new();
        };
        if self.synced && self.pending.is_empty() {
            return Vec::new();
        }
        // What is pending goes out first, so the `seq` of the Sync covers it.
        let mut out: Vec<_> = self
            .flush()
            .map(ServerOutput::Datagram)
            .into_iter()
            .collect();
        self.synced = true;
        out.push(ServerOutput::Stream(StreamMsg::Sync { seq: self.seq, pos }));
        out
    }

    /// The geometry changed (mode or screens change). If the other machine no longer
    /// has visible screens, the cursor comes back. A local cursor left in a screen that is no
    /// longer in the geometry (its monitor went off, to standby or to the other machine, T-349)
    /// is taken to the nearest live screen.
    pub fn set_layout(&mut self, layout: Layout) -> Vec<ServerOutput> {
        self.layout = layout;
        let mut out = Vec::new();
        if let Some(s) = self.layout.screen_at(self.machine, self.local) {
            let inside = s.rect.clamp(self.local);
            if inside != self.local && !self.is_remote() {
                out.push(ServerOutput::Warp(inside));
            }
            self.local = inside;
        }
        // A settle made on a view that did not last (a role or link change): with a screen of
        // this machine visible again the cursor comes back instead of staying on the other one.
        if self.settled
            && self.is_remote()
            && let Some(first) = self.layout.screens(self.machine).first()
        {
            let r = first.rect;
            let back = if self.layout.screen_at(self.machine, self.local).is_some() {
                self.local
            } else {
                Point::new(
                    f64::from(r.x) + f64::from(r.width) / 2.0,
                    f64::from(r.y) + f64::from(r.height) / 2.0,
                )
            };
            tracing::info!("a screen of this machine is visible again: the cursor comes back");
            self.leave(back, &mut out);
            return out;
        }
        if let Focus::Remote { pos } = self.focus {
            match self.layout.screen_at(self.machine.other(), pos) {
                Some(s) => {
                    let clamped = s.rect.clamp(pos);
                    if clamped != pos {
                        self.focus = Focus::Remote { pos: clamped };
                        self.synced = false;
                    }
                }
                // Coming back to a machine with no visible screen would leave the cursor
                // where nobody sees it and, with no crossing edge, with no way out: it
                // stays on the other one until one of the two has a screen again.
                None if self.layout.screens(self.machine).is_empty() => {}
                None => {
                    let back = self.local;
                    self.leave(back, &mut out);
                }
            }
        }
        self.settle(&mut out);
        out
    }

    /// Starts a session: if this machine has nothing visible, the focus goes to the other one.
    pub fn start(&mut self) -> Vec<ServerOutput> {
        let mut out = Vec::new();
        self.settle(&mut out);
        out
    }

    /// Every monitor of this machine shows the other one (a scene): the keyboard and mouse
    /// can only be used over there, so the cursor goes there by itself, to the centre of its
    /// first screen. Without this, with no crossing edge (it needs screens on both sides) it
    /// would never reach it. Screens not known yet (starting up) do not count: only what the
    /// agent marks ([`Layout::shown_elsewhere`]).
    fn settle(&mut self, out: &mut Vec<ServerOutput>) {
        if self.is_remote()
            || !self.layout.screens(self.machine).is_empty()
            || !self.layout.shown_elsewhere(self.machine)
        {
            return;
        }
        let Some(target) = self.layout.screens(self.machine.other()).first() else {
            return;
        };
        let r = target.rect;
        let center = Point::new(
            f64::from(r.x) + f64::from(r.width) / 2.0,
            f64::from(r.y) + f64::from(r.height) / 2.0,
        );
        tracing::info!("no screen of this machine is visible: the cursor goes to the other one");
        self.enter(center, out);
        self.settled = true;
    }

    /// The capture refused to take local input (Secure Event Input: a password field): the
    /// cursor did not leave, so the session cannot believe the focus is on the other machine.
    /// Without this the Mac kept typing locally while the other one received everything too,
    /// and the cursor was stuck on a machine that was not following it. The focus comes back
    /// here and the client is told.
    pub fn grab_refused(&mut self) -> Vec<ServerOutput> {
        let mut out = Vec::new();
        if self.is_remote() {
            tracing::warn!("the capture refused the input: the cursor stays on this machine");
            let back = self.local;
            self.leave(back, &mut out);
        }
        out
    }

    /// The link dropped or changed: the focus returns to this machine without notifying the
    /// client (there is no way to); it releases everything on its own.
    pub fn link_lost(&mut self) -> Vec<ServerOutput> {
        if !self.is_remote() {
            return Vec::new();
        }
        self.focus = Focus::Local;
        self.settled = false;
        self.pending = Pending::default();
        vec![ServerOutput::Grab(false), ServerOutput::Warp(self.local)]
    }
}

/// macOS delivers PrintScreen, ScrollLock and Pause of a PC keyboard as F13, F14
/// and F15 ([`crate::event::keymap`]): for the shortcut they are the same key.
fn alias_key(usage: HidUsage) -> Option<HidUsage> {
    const PAIRS: [(u16, u16); 3] = [(0x46, 0x68), (0x47, 0x69), (0x48, 0x6A)];
    if usage.page != crate::event::PAGE_KEYBOARD {
        return None;
    }
    PAIRS.iter().find_map(|&(pc, mac)| {
        if usage.id == pc {
            Some(HidUsage::key(mac))
        } else if usage.id == mac {
            Some(HidUsage::key(pc))
        } else {
            None
        }
    })
}

fn canonical_key(usage: HidUsage) -> HidUsage {
    match alias_key(usage) {
        Some(a) if a.id < usage.id => a,
        _ => usage,
    }
}

/// Adds two scrolls of the same type; `None` if they are of different types.
fn merge_scroll(acc: Option<Scroll>, s: Scroll) -> Option<Scroll> {
    match (acc, s) {
        (None, s) => Some(s),
        (
            Some(Scroll::Wheel { dx_v120, dy_v120 }),
            Scroll::Wheel {
                dx_v120: x,
                dy_v120: y,
            },
        ) => Some(Scroll::Wheel {
            dx_v120: dx_v120.saturating_add(x),
            dy_v120: dy_v120.saturating_add(y),
        }),
        (Some(Scroll::Pixels { dx, dy }), Scroll::Pixels { dx: x, dy: y }) => {
            Some(Scroll::Pixels {
                dx: dx + x,
                dy: dy + y,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Button, HidUsage, KeyState, ModifierKey, Modifiers};
    use crate::session::testutil::{
        edge_pc, layout_mac_only, layout_mac_pc_hidpi, layout_none, layout_pc_mac,
        layout_pc_mac_small, layout_pc_only, layout_pc_two_mac, layout_pc_unknown,
    };

    fn session() -> ServerSession {
        ServerSession::new(Machine::B, layout_pc_mac(), Point::new(100.0, 100.0), 4)
    }

    fn rel(dx: f64, dy: f64) -> InputEvent {
        InputEvent::MotionRelative { dx, dy }
    }

    fn key(id: u16, state: KeyState) -> InputEvent {
        InputEvent::Key {
            usage: HidUsage::key(id),
            state,
        }
    }

    /// Takes the cursor to the edge and crosses; returns the crossing's outputs.
    fn cross(s: &mut ServerSession) -> Vec<ServerOutput> {
        s.handle(InputEvent::MotionAbsolute {
            x: edge_pc(),
            y: 720.0,
        });
        s.handle(rel(5.0, 0.0))
    }

    /// T-362: the capture refused to take the input after a crossing: the focus comes back
    /// and the client is told.
    #[test]
    fn a_refused_grab_returns_the_focus_and_tells_the_client() {
        let mut s = session();
        cross(&mut s);
        assert!(s.is_remote());
        let out = s.grab_refused();
        assert!(!s.is_remote());
        assert!(out.contains(&ServerOutput::Stream(StreamMsg::Leave)));
        assert!(out.contains(&ServerOutput::Grab(false)));
        // Without a remote focus there is nothing to undo.
        assert!(s.grab_refused().is_empty());
    }

    /// T-362: a machine with no visible screen whose input is refused (a lock screen) must
    /// not stay "on the other machine": the session returns to local.
    #[test]
    fn a_refused_settle_does_not_strand_the_cursor() {
        let mut s = ServerSession::new(Machine::B, layout_mac_only(), Point::new(9.0, 9.0), 4);
        s.start();
        assert!(s.is_remote());
        s.grab_refused();
        assert!(!s.is_remote());
    }

    /// T-362: a settle made on a transient view (a role or link change, the PC at 15:43) must
    /// not strand the cursor: when a screen of this machine is visible again, it comes back.
    #[test]
    fn a_settled_cursor_comes_back_when_a_own_screen_is_visible_again() {
        let mut s = ServerSession::new(Machine::B, layout_mac_only(), Point::new(9.0, 9.0), 4);
        s.start();
        assert!(s.is_remote());
        let out = s.set_layout(layout_pc_mac());
        assert!(!s.is_remote());
        assert!(out.contains(&ServerOutput::Stream(StreamMsg::Leave)));
        assert!(out.contains(&ServerOutput::Grab(false)));
    }

    /// A cursor that crossed by an edge is not taken back by a layout change.
    #[test]
    fn a_crossed_cursor_stays_when_the_layout_changes() {
        let mut s = session();
        cross(&mut s);
        s.set_layout(layout_pc_mac());
        assert!(s.is_remote());
    }

    #[test]
    fn local_sends_nothing() {
        let mut s = session();
        assert!(s.handle(rel(10.0, 5.0)).is_empty());
        assert_eq!(s.local_cursor(), Point::new(110.0, 105.0));
        assert!(s.handle(key(0x04, KeyState::Pressed)).is_empty());
        assert!(
            s.handle(InputEvent::Scroll(Scroll::Pixels { dx: 0.0, dy: 1.0 }))
                .is_empty()
        );
        assert_eq!(s.flush(), None);
        assert!(!s.is_remote());
    }

    #[test]
    fn crossing_enters_with_the_modifiers_held() {
        let mut s = session();
        s.handle(key(0xE1, KeyState::Pressed)); // left Shift
        s.handle(key(0x04, KeyState::Pressed)); // A: not a modifier
        let out = cross(&mut s);
        assert_eq!(
            out,
            vec![
                ServerOutput::Grab(true),
                ServerOutput::Stream(StreamMsg::Enter {
                    seq: 0,
                    pos: Point::new(3440.0, 540.0),
                    modifiers: Modifiers::from_iter([ModifierKey::ShiftLeft]),
                }),
            ]
        );
        assert!(s.is_remote());
        assert_eq!(s.remote_cursor(), Some(Point::new(3440.0, 540.0)));
    }

    #[test]
    fn movement_is_coalesced_and_resynchronized_every_n() {
        let mut s = session();
        cross(&mut s);
        for _ in 0..3 {
            assert!(s.handle(rel(2.0, 1.0)).is_empty());
        }
        let d = s.flush().unwrap();
        assert_eq!((d.seq, d.dx, d.dy, d.resync), (1, 6.0, 3.0, None));
        assert_eq!(s.flush(), None, "nothing pending");
        for i in 2..=4 {
            s.handle(rel(1.0, 0.0));
            let d = s.flush().unwrap();
            assert_eq!(d.seq, i);
            // The fourth datagram carries the absolute position.
            assert_eq!(d.resync.is_some(), i == 4);
        }
        assert_eq!(s.remote_cursor(), Some(Point::new(3449.0, 543.0)));
    }

    #[test]
    fn the_delta_travels_in_the_clients_native_units() {
        // PC client at 150 %, Mac server at 1.0.
        let mut s = ServerSession::new(
            Machine::A,
            layout_mac_pc_hidpi(),
            Point::new(3439.0, 720.0),
            0,
        );
        let out = s.handle(rel(1.0, 0.0));
        assert!(matches!(
            out[1],
            ServerOutput::Stream(StreamMsg::Enter { .. })
        ));
        let start = s.remote_cursor().unwrap();
        s.handle(rel(10.0, 0.0));
        let d = s.flush().unwrap();
        // 10 Mac points are 15 pixels on the PC's 150 % screen: the same
        // physical distance.
        assert_eq!((d.dx, d.dy), (15.0, 0.0));
        assert_eq!(d.resync, Some(Point::new(start.x + 15.0, start.y)));
    }

    #[test]
    fn still_resynchronizes_only_once() {
        let mut s = session();
        cross(&mut s);
        assert!(!s.needs_resync(), "the Enter already carries the position");
        assert!(s.resync().is_empty());
        s.handle(rel(3.0, 0.0));
        s.flush().unwrap();
        assert!(s.needs_resync());
        let pos = s.remote_cursor().unwrap();
        assert_eq!(
            s.resync(),
            vec![ServerOutput::Stream(StreamMsg::Sync { seq: 1, pos })]
        );
        assert!(s.resync().is_empty());
        // With pending movement, the datagram goes out first.
        s.handle(rel(1.0, 0.0));
        let out = s.resync();
        assert!(matches!(out[0], ServerOutput::Datagram(d) if d.seq == 2));
        assert!(matches!(
            out[1],
            ServerOutput::Stream(StreamMsg::Sync { seq: 2, .. })
        ));
        assert!(s.resync().is_empty());
        assert!(s.link_lost().len() == 2);
        assert!(
            s.resync().is_empty(),
            "without remote focus there is nothing to synchronize"
        );
    }

    #[test]
    fn the_button_carries_the_position_and_goes_out_after_the_movement() {
        let mut s = session();
        cross(&mut s);
        s.handle(rel(4.0, 0.0));
        let out = s.handle(InputEvent::Button {
            button: Button::Left,
            state: KeyState::Pressed,
        });
        let pos = s.remote_cursor().unwrap();
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], ServerOutput::Datagram(d) if d.seq == 1));
        assert_eq!(
            out[1],
            ServerOutput::Stream(StreamMsg::Button {
                button: Button::Left,
                state: KeyState::Pressed,
                seq: 1,
                pos,
                t_us: 0,
            })
        );
    }

    #[test]
    fn the_datagram_carries_the_stamp_of_the_oldest_event() {
        let mut s = session();
        cross(&mut s);
        s.handle_at(rel(1.0, 0.0), 1_000);
        s.handle_at(rel(1.0, 0.0), 1_700);
        s.handle_at(
            InputEvent::Scroll(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: 120,
            }),
            2_000,
        );
        assert_eq!(s.flush().unwrap().t_us, 1_000);
        s.handle_at(
            InputEvent::Scroll(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: 120,
            }),
            5_000,
        );
        s.handle_at(rel(1.0, 0.0), 6_000);
        assert_eq!(s.flush().unwrap().t_us, 5_000);
        // A movement against an edge without a crossing accumulates nothing.
        assert_eq!(s.flush(), None);
    }

    #[test]
    fn a_drag_does_not_cross_the_edge_unless_allowed() {
        let press = InputEvent::Button {
            button: Button::Left,
            state: KeyState::Pressed,
        };
        let release = InputEvent::Button {
            button: Button::Left,
            state: KeyState::Released,
        };
        // By default (like Deskflow) the drag stays on its machine.
        let mut s = session();
        s.handle(InputEvent::MotionAbsolute {
            x: edge_pc(),
            y: 720.0,
        });
        s.handle(press);
        assert!(s.handle(rel(50.0, 0.0)).is_empty());
        assert!(!s.is_remote());
        assert_eq!(s.local_cursor(), Point::new(edge_pc(), 720.0));
        // On release, it crosses.
        s.handle(release);
        assert!(matches!(
            s.handle(rel(5.0, 0.0))[0],
            ServerOutput::Grab(true)
        ));
        // On the other side it does not come back dragging either.
        s.handle(press);
        assert!(s.handle(rel(-500.0, 0.0)).is_empty());
        assert!(s.is_remote());
        s.handle(release);
        assert!(matches!(
            s.handle(rel(-5.0, 0.0))[0],
            ServerOutput::Stream(StreamMsg::Leave)
        ));
        // Configured, it crosses with the button pressed.
        let mut s = session().with_drag_across(true);
        s.handle(InputEvent::MotionAbsolute {
            x: edge_pc(),
            y: 720.0,
        });
        s.handle(press);
        assert!(matches!(
            s.handle(rel(5.0, 0.0))[0],
            ServerOutput::Grab(true)
        ));
    }

    #[test]
    fn a_file_drag_crosses_even_if_dragging_is_off() {
        let press = InputEvent::Button {
            button: Button::Left,
            state: KeyState::Pressed,
        };
        let mut s = session();
        s.handle(InputEvent::MotionAbsolute {
            x: edge_pc(),
            y: 720.0,
        });
        s.handle(press);
        assert!(s.handle(rel(50.0, 0.0)).is_empty());
        s.set_file_drag(true);
        assert!(matches!(
            s.handle(rel(5.0, 0.0))[0],
            ServerOutput::Grab(true)
        ));
    }

    #[test]
    fn the_external_file_drag_flag_allows_crossing_and_only_while_it_lasts() {
        let flag = Arc::new(AtomicBool::new(false));
        let press = InputEvent::Button {
            button: Button::Left,
            state: KeyState::Pressed,
        };
        let mut s = session().with_file_drag_flag(flag.clone());
        s.handle(InputEvent::MotionAbsolute {
            x: edge_pc(),
            y: 720.0,
        });
        s.handle(press);
        assert!(s.button_held());
        assert!(
            s.handle(rel(50.0, 0.0)).is_empty(),
            "without the flag it stays"
        );
        flag.store(true, Ordering::Relaxed);
        assert!(matches!(
            s.handle(rel(5.0, 0.0))[0],
            ServerOutput::Grab(true)
        ));
    }

    #[test]
    fn scroll_adds_the_same_type_and_separates_different_ones() {
        let mut s = session();
        cross(&mut s);
        let wheel = |dy| {
            InputEvent::Scroll(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: dy,
            })
        };
        assert!(s.handle(wheel(120)).is_empty());
        assert!(s.handle(wheel(60)).is_empty());
        let out = s.handle(InputEvent::Scroll(Scroll::Pixels { dx: 1.0, dy: 2.0 }));
        assert_eq!(out.len(), 1);
        let ServerOutput::Datagram(d) = out[0] else {
            panic!("a datagram was expected: {out:?}")
        };
        assert_eq!(
            d.scroll,
            Some(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: 180
            })
        );
        assert_eq!(
            s.flush().unwrap().scroll,
            Some(Scroll::Pixels { dx: 1.0, dy: 2.0 })
        );
    }

    #[test]
    fn coming_back_through_the_edge_leaves_and_returns_the_cursor() {
        let mut s = session();
        cross(&mut s);
        s.handle(rel(2.0, 0.0));
        let out = s.handle(rel(-10.0, 0.0));
        let back = s.local_cursor();
        assert_eq!(
            out,
            vec![
                ServerOutput::Stream(StreamMsg::Leave),
                ServerOutput::Grab(false),
                ServerOutput::Warp(back),
            ]
        );
        assert!(!s.is_remote());
        assert_eq!(s.flush(), None, "what is pending is discarded on leaving");
        assert!(back.x < 3440.0);
    }

    #[test]
    fn each_crossing_is_recorded_with_its_edge_and_collected_once() {
        let mut s = session();
        assert!(s.take_crossings().is_empty());
        cross(&mut s);
        let edge = s.layout.facing(Machine::B).unwrap();
        assert_eq!(
            s.take_crossings(),
            vec![Crossing {
                from: Machine::B,
                to: Machine::A,
                edge
            }]
        );
        assert!(s.take_crossings().is_empty(), "already collected");
        // Moving inside the other machine does not cross; coming back does, through the other's edge.
        s.handle(rel(2.0, 0.0));
        assert!(s.take_crossings().is_empty());
        s.handle(rel(-10.0, 0.0));
        let back = s.layout.facing(Machine::A).unwrap();
        assert_eq!(back, edge.opposite());
        assert_eq!(
            s.take_crossings(),
            vec![Crossing {
                from: Machine::A,
                to: Machine::B,
                edge: back
            }]
        );
    }

    #[test]
    fn a_local_movement_records_no_crossings() {
        let mut s = session();
        s.handle(rel(10.0, 5.0));
        assert!(s.take_crossings().is_empty());
    }

    #[test]
    fn the_cursors_screen_is_the_one_containing_it_or_the_nearest() {
        let s = session();
        assert_eq!(s.screen_id(Point::new(100.0, 100.0)), "m34");
        // Outside every screen: the nearest.
        assert_eq!(s.screen_id(Point::new(-50.0, 100.0)), "m34");
    }

    #[test]
    fn keys_are_forwarded_while_the_focus_is_remote() {
        let mut s = session();
        cross(&mut s);
        // With the capture stamp, to measure latency.
        assert_eq!(
            s.handle_at(key(0x05, KeyState::Pressed), 42),
            vec![ServerOutput::Stream(StreamMsg::Key {
                usage: HidUsage::key(0x05),
                state: KeyState::Pressed,
                t_us: 42,
            })]
        );
        assert!(
            s.handle(InputEvent::MotionAbsolute { x: 0.0, y: 0.0 })
                .is_empty()
        );
    }

    #[test]
    fn catching_up_neither_crosses_nor_moves_but_remembers_what_is_pressed() {
        let mut s = session();
        s.catch_up(key(0xE1, KeyState::Pressed));
        s.catch_up(InputEvent::MotionAbsolute {
            x: edge_pc(),
            y: 720.0,
        });
        // Against the edge: with `handle` it would cross.
        s.catch_up(rel(50.0, 0.0));
        s.catch_up(InputEvent::Scroll(Scroll::Pixels { dx: 0.0, dy: 3.0 }));
        assert!(!s.is_remote());
        assert_eq!(s.local_cursor(), Point::new(edge_pc(), 720.0));
        assert_eq!(s.flush(), None);
        // The Shift that is still held enters with the first real crossing.
        let out = s.handle(rel(5.0, 0.0));
        assert!(matches!(
            out[1],
            ServerOutput::Stream(StreamMsg::Enter { modifiers, .. })
                if modifiers == Modifiers::from_iter([ModifierKey::ShiftLeft])
        ));
        // With remote focus it neither moves nor sends anything either.
        let remote = s.remote_cursor();
        s.catch_up(rel(10.0, 0.0));
        s.catch_up(InputEvent::MotionAbsolute { x: 0.0, y: 0.0 });
        assert_eq!(s.remote_cursor(), remote);
        assert_eq!(s.local_cursor(), Point::new(edge_pc(), 720.0));
        assert_eq!(s.flush(), None);
    }

    #[test]
    fn link_drop_returns_the_focus_locally() {
        let mut s = session();
        assert!(s.link_lost().is_empty());
        cross(&mut s);
        let frozen = s.local_cursor();
        s.handle(rel(5.0, 0.0));
        assert_eq!(
            s.link_lost(),
            vec![ServerOutput::Grab(false), ServerOutput::Warp(frozen)]
        );
        assert!(!s.is_remote());
        assert_eq!(s.flush(), None);
    }

    /// T-349: the screen the local cursor is on leaves the geometry (its monitor went to standby):
    /// the cursor is taken to the nearest live screen, once.
    #[test]
    fn a_local_cursor_in_a_screen_that_left_is_moved_to_the_nearest_live_one() {
        let mut s = ServerSession::new(
            Machine::B,
            layout_pc_two_mac(),
            Point::new(-500.0, 300.0),
            4,
        );
        let out = s.set_layout(layout_pc_mac());
        assert_eq!(out, vec![ServerOutput::Warp(Point::new(0.0, 300.0))]);
        assert_eq!(s.local_cursor(), Point::new(0.0, 300.0));
        // Already inside: nothing more to do.
        assert!(s.set_layout(layout_pc_mac()).is_empty());
    }

    #[test]
    fn if_the_other_machine_stops_being_seen_the_cursor_comes_back() {
        let mut s = session();
        cross(&mut s);
        let out = s.set_layout(layout_pc_only());
        assert_eq!(out[0], ServerOutput::Stream(StreamMsg::Leave));
        assert_eq!(out[1], ServerOutput::Grab(false));
        assert!(!s.is_remote());
    }

    /// The bug: with every monitor showing the Mac, a moment without Mac screens (a scene
    /// change, macOS reconfiguring its displays) brought the cursor back to a PC with
    /// nothing visible and no crossing edge: the mouse "died" until the agent restarted.
    #[test]
    fn without_local_screens_the_focus_does_not_come_back_to_nothing() {
        let mut s = session();
        cross(&mut s);
        let mac_only = layout_mac_only();
        assert!(s.set_layout(mac_only.clone()).is_empty());
        assert!(s.is_remote());
        // The Mac's screens vanish for a moment: the focus stays there.
        assert!(s.set_layout(layout_none()).is_empty());
        assert!(s.is_remote());
        // They come back: the mouse keeps moving the Mac's cursor.
        assert!(s.set_layout(mac_only).is_empty());
        s.handle(rel(10.0, 0.0));
        assert!(s.flush().is_some());
        // The PC shows itself again: now it can come back through the edge.
        assert!(s.set_layout(layout_pc_mac()).is_empty());
        assert!(s.is_remote());
    }

    #[test]
    fn without_local_screens_the_cursor_goes_to_the_other_machine_by_itself() {
        let mut s = session();
        // Session start with the scene already applied.
        let mut s2 = ServerSession::new(Machine::B, layout_mac_only(), Point::new(9.0, 9.0), 4);
        let out = s2.start();
        assert_eq!(out[0], ServerOutput::Grab(true));
        assert!(matches!(
            out[1],
            ServerOutput::Stream(StreamMsg::Enter { pos, .. }) if pos == Point::new(4400.0, 540.0)
        ));
        assert!(s2.is_remote());
        assert!(s2.start().is_empty(), "only once");
        // The scene changes during the session.
        let out = s.set_layout(layout_mac_only());
        assert_eq!(out[0], ServerOutput::Grab(true));
        assert!(s.is_remote());
        // With neither machine visible there is nowhere to go.
        let mut s3 = ServerSession::new(Machine::B, layout_none(), Point::new(9.0, 9.0), 4);
        assert!(s3.start().is_empty());
        assert!(!s3.is_remote());
        // With its own screens it stays local.
        assert!(session().start().is_empty());
        // Screens not read yet are not a scene: it does not jump to the other machine.
        let mut s4 = ServerSession::new(Machine::B, layout_pc_unknown(), Point::new(9.0, 9.0), 4);
        assert!(s4.start().is_empty());
        assert!(s4.set_layout(layout_pc_unknown()).is_empty());
        assert!(!s4.is_remote());
    }

    #[test]
    fn new_geometry_with_the_cursor_outside_adjusts_it_and_resynchronizes() {
        let mut s = session();
        cross(&mut s);
        // Same screens: nothing to do.
        assert!(s.set_layout(layout_pc_mac()).is_empty());
        assert!(!s.needs_resync());
        s.handle(rel(100.0, 400.0));
        s.flush();
        s.resync();
        assert_eq!(s.remote_cursor(), Some(Point::new(3540.0, 940.0)));
        // The client's screen shrinks: the cursor stays inside.
        assert!(s.set_layout(layout_pc_mac_small()).is_empty());
        assert_eq!(s.remote_cursor(), Some(Point::new(3540.0, 599.0)));
        assert_eq!(
            s.resync(),
            vec![ServerOutput::Stream(StreamMsg::Sync {
                seq: 1,
                pos: Point::new(3540.0, 599.0)
            })]
        );
    }

    #[test]
    fn the_shortcut_locks_the_cursor_on_its_machine() {
        // Left Ctrl + ScrollLock.
        let mut s = session().with_lock_shortcut(vec![HidUsage::key(0xE0), HidUsage::key(0x47)]);
        s.handle(InputEvent::MotionAbsolute {
            x: edge_pc(),
            y: 720.0,
        });
        // ScrollLock alone is not enough.
        s.handle(key(0x47, KeyState::Pressed));
        s.handle(key(0x47, KeyState::Released));
        assert!(!s.is_locked());
        s.handle(key(0xE0, KeyState::Pressed));
        s.handle(key(0x47, KeyState::Pressed));
        assert!(s.is_locked());
        assert!(s.handle(rel(50.0, 0.0)).is_empty(), "locked does not cross");
        assert!(!s.is_remote());
        assert_eq!(s.local_cursor(), Point::new(edge_pc(), 720.0));
        // Repeating the shortcut unlocks and the cursor crosses again.
        s.handle(key(0x47, KeyState::Released));
        s.handle(key(0x47, KeyState::Pressed));
        s.handle(key(0x47, KeyState::Released));
        s.handle(key(0xE0, KeyState::Released));
        assert!(!s.is_locked());
        assert!(matches!(
            s.handle(rel(5.0, 0.0))[0],
            ServerOutput::Grab(true)
        ));
    }

    #[test]
    fn locked_on_the_other_machine_does_not_return() {
        let mut s = session().with_lock_shortcut(vec![HidUsage::key(0x47)]);
        cross(&mut s);
        // The shortcut also reaches the client: it is just another key.
        let out = s.handle(key(0x47, KeyState::Pressed));
        assert!(matches!(
            out[0],
            ServerOutput::Stream(StreamMsg::Key { .. })
        ));
        assert!(s.is_locked());
        assert!(s.handle(rel(-500.0, 0.0)).is_empty());
        assert!(s.is_remote(), "against the edge, without coming back");
        assert_eq!(s.remote_cursor().map(|p| p.x), Some(3440.0));
    }

    #[test]
    fn scroll_lock_of_a_pc_keyboard_on_the_mac_is_f14() {
        let mut s = ServerSession::new(Machine::A, layout_pc_mac(), Point::new(3500.0, 10.0), 4)
            .with_lock_shortcut(vec![HidUsage::key(0x47)]);
        s.handle(key(0x69, KeyState::Pressed));
        assert!(s.is_locked());
        assert_eq!(canonical_key(HidUsage::key(0x69)), HidUsage::key(0x47));
        assert_eq!(canonical_key(HidUsage::key(0x04)), HidUsage::key(0x04));
        assert_eq!(alias_key(HidUsage::consumer(0x47)), None);
    }

    #[test]
    fn automatic_repeat_does_not_toggle_the_lock() {
        // The captures forward the repeat as another `Pressed`.
        let mut s = session().with_lock_shortcut(vec![HidUsage::key(0x47)]);
        for _ in 0..3 {
            s.handle(key(0x47, KeyState::Pressed));
        }
        assert!(s.is_locked());
        s.handle(key(0x47, KeyState::Released));
        assert!(s.is_locked());
        // Also with remote focus and with a shortcut of several keys.
        let mut s = session().with_lock_shortcut(vec![HidUsage::key(0xE0), HidUsage::key(0x0F)]);
        cross(&mut s);
        s.handle(key(0xE0, KeyState::Pressed));
        for _ in 0..4 {
            s.handle(key(0x0F, KeyState::Pressed));
        }
        assert!(s.is_locked());
    }

    #[test]
    fn without_a_shortcut_it_never_locks() {
        let mut s = session();
        s.handle(key(0x47, KeyState::Pressed));
        assert!(!s.is_locked());
        s.set_locked(true);
        assert!(s.is_locked());
    }

    #[test]
    fn merge_scroll_saturates() {
        let big = Scroll::Wheel {
            dx_v120: i32::MAX,
            dy_v120: 0,
        };
        assert_eq!(merge_scroll(Some(big), big), Some(big));
    }
}
