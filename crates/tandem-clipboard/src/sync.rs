//! Synchronization state machine, without I/O: receives what happens (the
//! local clipboard changed, a message arrived, focus changed) and returns what to
//! do ([`Action`]). It is the same on both agents.
//!
//! **Echo suppression.** The hash of what the local clipboard holds is remembered
//! (`current`), whether because it was seen changing or because what arrived
//! from the other side was written. A local change with that same hash is not announced: it is our
//! own write coming back. And an offer from the other side with the hash we
//! already have is not written.

use std::time::{SystemTime, UNIX_EPOCH};

use tandem_proto::MAX_FRAME_LEN;

use crate::content::{ClipContent, ContentHash};
use crate::msg::ClipMsg;

/// Transfer limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncConfig {
    /// Up to this (encoded) size the content goes inside the announcement.
    pub inline_max: usize,
    /// Size of each `Chunk`. Small chunks let other messages through in
    /// between.
    pub chunk_len: usize,
    /// The largest that is sent or accepted: a cap on the memory the other side
    /// can ask for.
    pub max_len: usize,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            inline_max: 64 * 1024,
            chunk_len: 256 * 1024,
            // A 5K RGBA capture is ~60 MB.
            max_len: 128 * 1024 * 1024,
        }
    }
}

impl SyncConfig {
    /// What goes in a frame has to fit in it, with room for the
    /// message header.
    fn clamped(self) -> Self {
        let frame_room = MAX_FRAME_LEN - 1024;
        Self {
            inline_max: self.inline_max.min(frame_room),
            chunk_len: self.chunk_len.clamp(1, frame_room),
            max_len: self.max_len,
        }
    }
}

/// What the state machine asks to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send to the other agent.
    Send(ClipMsg),
    /// Write to the local clipboard what arrived from the other side.
    Write(ClipContent),
}

/// Counters, for diagnostics and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// Local copy announcements sent.
    pub offers_sent: u64,
    /// Announcements received from the other side.
    pub offers_received: u64,
    /// Contents from the other side written to the local clipboard.
    pub written: u64,
    /// Offers or transfers discarded as invalid (hash, size).
    pub rejected: u64,
}

/// The local copy the other side can request, already encoded.
#[derive(Debug, Clone)]
struct Local {
    id: u64,
    hash: ContentHash,
    bytes: Vec<u8>,
}

/// A copy from the other side that was announced and not yet written.
#[derive(Debug, Clone)]
struct Remote {
    id: u64,
    hash: ContentHash,
    len: usize,
    requested: bool,
    buf: Vec<u8>,
}

/// Transfer of the local copy in progress.
#[derive(Debug, Clone)]
struct Outgoing {
    id: u64,
    pos: usize,
}

#[derive(Debug, Clone)]
pub struct ClipSync {
    cfg: SyncConfig,
    focused: bool,
    /// Hash of what the local clipboard holds, seen or written by us.
    current: Option<ContentHash>,
    /// The current copy between the two sides, own or foreign: `(id, hash)` of the
    /// last one that won. Only a greater offer is accepted.
    winner: Option<(u64, ContentHash)>,
    /// Wall clock in microseconds; floor of the ids of new copies.
    clock: fn() -> u64,
    local: Option<Local>,
    remote: Option<Remote>,
    outgoing: Option<Outgoing>,
    stats: SyncStats,
}

impl ClipSync {
    /// `focused`: whether this machine now has the keyboard and mouse.
    pub fn new(cfg: SyncConfig, focused: bool) -> Self {
        Self::with_clock(cfg, focused, wall_micros)
    }

    pub(crate) fn with_clock(cfg: SyncConfig, focused: bool, clock: fn() -> u64) -> Self {
        Self {
            cfg: cfg.clamped(),
            focused,
            current: None,
            winner: None,
            clock,
            local: None,
            remote: None,
            outgoing: None,
            stats: SyncStats::default(),
        }
    }

    pub fn stats(&self) -> SyncStats {
        self.stats
    }

    pub fn focused(&self) -> bool {
        self.focused
    }

    /// There is a copy from the other side announced that has not been written yet.
    pub fn has_pending(&self) -> bool {
        self.remote.is_some()
    }

    /// There are chunks of the local copy left to send ([`Self::next_chunk`]).
    pub fn is_sending(&self) -> bool {
        self.outgoing.is_some()
    }

    /// Starts a session (new link) with what the local clipboard holds
    /// now. If it changed while there was no link, it is announced; if not, the
    /// current local copy is announced again, in case its announcement was lost with the previous
    /// link (the other side ignores what it already has). What was pending from the other side
    /// is kept (its owner still has it), but the half-done transfer
    /// is discarded and requested again.
    pub fn start(&mut self, local: Option<ClipContent>) -> Vec<Action> {
        self.begin(Some(local))
    }

    /// Like [`Self::start`], when the local clipboard could not be read: it
    /// continues with the last thing known about it.
    pub fn resume(&mut self) -> Vec<Action> {
        self.begin(None)
    }

    fn begin(&mut self, local: Option<Option<ClipContent>>) -> Vec<Action> {
        self.outgoing = None;
        if let Some(r) = &mut self.remote {
            r.requested = false;
            r.buf = Vec::new();
        }
        let mut out = local.map_or_else(Vec::new, |l| self.local_changed(l));
        if out.is_empty()
            && let Some(offer) = self.offer_again()
        {
            out.push(Action::Send(offer));
        }
        out.extend(self.request_if_needed());
        out
    }

    fn offer_again(&mut self) -> Option<ClipMsg> {
        let local = self
            .local
            .as_ref()
            .filter(|l| Some(l.hash) == self.current && Some((l.id, l.hash)) == self.winner)?;
        let content = if local.bytes.len() <= self.cfg.inline_max {
            Some(tandem_proto::decode::<ClipContent>(&local.bytes).ok()?)
        } else {
            None
        };
        self.stats.offers_sent += 1;
        Some(ClipMsg::Offer {
            id: local.id,
            hash: local.hash,
            len: local.bytes.len() as u64,
            content,
        })
    }

    /// The local clipboard changed; `content` is what it holds now (`None` if
    /// it is empty or only has formats that are not synchronized, like files).
    pub fn local_changed(&mut self, content: Option<ClipContent>) -> Vec<Action> {
        let content = content.and_then(ClipContent::normalized);
        let hash = content.as_ref().map(ClipContent::hash);
        if hash == self.current {
            // Our own write, or a change that changed nothing.
            return Vec::new();
        }
        self.current = hash;
        // What was just copied is newer than whatever there was from the other side.
        self.remote = None;
        self.outgoing = None;
        self.local = None;
        let (Some(content), Some(hash)) = (content, hash) else {
            return Vec::new();
        };
        let bytes = match tandem_proto::encode(&content) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("clipboard: could not encode: {e}");
                return Vec::new();
            }
        };
        if bytes.len() > self.cfg.max_len {
            tracing::warn!(
                len = bytes.len(),
                "clipboard: the copy exceeds the maximum, it is not synchronized"
            );
            return Vec::new();
        }
        // Lamport clock with a floor at the wall clock: a copy made after
        // seeing the other side's is always greater, and a restarted agent (which
        // forgot everything) does not end up below what the other remembers.
        let id = self
            .winner
            .map_or(0, |(id, _)| id.saturating_add(1))
            .max((self.clock)());
        self.winner = Some((id, hash));
        let inline = (bytes.len() <= self.cfg.inline_max).then_some(content);
        let offer = ClipMsg::Offer {
            id,
            hash,
            len: bytes.len() as u64,
            content: inline,
        };
        self.local = Some(Local { id, hash, bytes });
        self.stats.offers_sent += 1;
        vec![Action::Send(offer)]
    }

    /// Keyboard and mouse focus changed. On arriving at this machine what is pending
    /// is requested: it is when the user can paste.
    pub fn set_focus(&mut self, focused: bool) -> Vec<Action> {
        self.focused = focused;
        self.request_if_needed()
    }

    pub fn on_message(&mut self, msg: ClipMsg) -> Vec<Action> {
        match msg {
            ClipMsg::Offer {
                id,
                hash,
                len,
                content,
            } => self.on_offer(id, hash, len, content),
            ClipMsg::Request { id } => self.on_request(id),
            ClipMsg::Chunk { id, data, last } => self.on_chunk(id, data, last),
            ClipMsg::Unavailable { id } => {
                if self.remote.as_ref().is_some_and(|r| r.id == id) {
                    self.remote = None;
                }
                Vec::new()
            }
        }
    }

    /// The next chunk of the transfer in progress.
    pub fn next_chunk(&mut self) -> Option<ClipMsg> {
        let out = self.outgoing.as_mut()?;
        let Some(local) = self.local.as_ref().filter(|l| l.id == out.id) else {
            self.outgoing = None;
            return None;
        };
        let end = (out.pos + self.cfg.chunk_len).min(local.bytes.len());
        let data = local.bytes[out.pos..end].to_vec();
        out.pos = end;
        let last = end == local.bytes.len();
        let id = out.id;
        if last {
            self.outgoing = None;
        }
        Some(ClipMsg::Chunk { id, data, last })
    }

    fn on_offer(
        &mut self,
        id: u64,
        hash: ContentHash,
        len: u64,
        content: Option<ClipContent>,
    ) -> Vec<Action> {
        self.stats.offers_received += 1;
        if Some((id, hash)) <= self.winner {
            // Older than the current one: a copy we already saw (re-announced after
            // a link change) or that crossed with ours and lost. The
            // other side compares the same way and keeps the same one.
            return Vec::new();
        }
        self.winner = Some((id, hash));
        // What was offered from here will no longer be requested.
        self.local = None;
        self.outgoing = None;
        self.remote = None;
        if Some(hash) == self.current {
            // We already have it: the same copy on both sides.
            return Vec::new();
        }
        if let Some(content) = content {
            return match content.normalized() {
                Some(c) if c.hash() == hash => self.write(c),
                _ => self.reject("offer with content that does not match its hash"),
            };
        }
        let Ok(len) = usize::try_from(len) else {
            return self.reject("offer too large");
        };
        if len > self.cfg.max_len {
            return self.reject("offer too large");
        }
        self.remote = Some(Remote {
            id,
            hash,
            len,
            requested: false,
            buf: Vec::new(),
        });
        self.request_if_needed()
    }

    fn on_request(&mut self, id: u64) -> Vec<Action> {
        if self.local.as_ref().is_some_and(|l| l.id == id) {
            self.outgoing = Some(Outgoing { id, pos: 0 });
            Vec::new()
        } else {
            vec![Action::Send(ClipMsg::Unavailable { id })]
        }
    }

    fn on_chunk(&mut self, id: u64, data: Vec<u8>, last: bool) -> Vec<Action> {
        let Some(remote) = self.remote.as_mut().filter(|r| r.id == id && r.requested) else {
            // From a transfer that is no longer of interest.
            return Vec::new();
        };
        if remote.buf.len() + data.len() > remote.len {
            self.remote = None;
            return self.reject("the transfer exceeds the announced size");
        }
        if remote.buf.is_empty() {
            remote.buf.reserve_exact(remote.len);
        }
        remote.buf.extend_from_slice(&data);
        if !last {
            return Vec::new();
        }
        let Some(remote) = self.remote.take() else {
            return Vec::new();
        };
        if remote.buf.len() != remote.len {
            return self.reject("the transfer ended before the announced size");
        }
        match tandem_proto::decode::<ClipContent>(&remote.buf)
            .ok()
            .and_then(ClipContent::normalized)
        {
            Some(c) if c.hash() == remote.hash => self.write(c),
            _ => self.reject("the transfer does not match its hash"),
        }
    }

    fn write(&mut self, content: ClipContent) -> Vec<Action> {
        self.current = Some(content.hash());
        self.stats.written += 1;
        vec![Action::Write(content)]
    }

    fn reject(&mut self, why: &str) -> Vec<Action> {
        tracing::warn!("portapapeles: {why}");
        self.stats.rejected += 1;
        Vec::new()
    }

    fn request_if_needed(&mut self) -> Vec<Action> {
        match &mut self.remote {
            Some(r) if self.focused && !r.requested => {
                r.requested = true;
                vec![Action::Send(ClipMsg::Request { id: r.id })]
            }
            _ => Vec::new(),
        }
    }
}

fn wall_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests;
