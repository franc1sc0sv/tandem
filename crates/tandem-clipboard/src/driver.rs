//! Driver: connects [`ClipSync`] with the system clipboard and with a reliable
//! `tandem-link` stream.
//!
//! Each `run` lasts as long as a link does. The stream is opened and accepted by the
//! agent's wiring (it is a clipboard-only stream, separate from the
//! input one, so a large image does not delay the keys); the driver splits it
//! into its two halves to keep reading while a transfer is being sent.
//! The synchronization state survives between sessions.

use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;

use tandem_link::transport::{ControlSender, ControlStream, TransportError};
use tokio::sync::{mpsc, watch};
use tokio::time::{self, MissedTickBehavior};

use crate::clipboard::Clipboard;
use crate::content::ClipContent;
use crate::msg::ClipMsg;
use crate::sync::{Action, ClipSync, SyncConfig, SyncStats};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipConfig {
    /// How often the system change counter is checked.
    pub poll: Duration,
    pub sync: SyncConfig,
}

impl Default for ClipConfig {
    fn default() -> Self {
        Self {
            // Copying and switching machines to paste takes longer than this.
            poll: Duration::from_millis(250),
            sync: SyncConfig::default(),
        }
    }
}

/// Why a session ended.
#[derive(Debug, thiserror::Error)]
pub enum SyncEnd {
    #[error("stopped: the link changed")]
    Stopped,
    #[error("enlace: {0}")]
    Link(#[from] TransportError),
}

/// Progressive wait when another app has the clipboard open and it cannot be read
/// (on Windows, "being held by another party"): without it, each polling turn retries
/// and complains in the log again (T-087).
///
/// Each consecutive failure waits twice as long as the previous (starting from `base`, up to
/// [`ReadBackoff::MAX`]) and only the first of the streak is reported in the log.
#[derive(Debug, Default)]
pub struct ReadBackoff {
    failures: u32,
    retry_at: Option<Instant>,
}

impl ReadBackoff {
    /// The most that is waited between two attempts.
    pub const MAX: Duration = Duration::from_secs(5);

    /// Whether it is time to try again.
    pub fn ready(&self, now: Instant) -> bool {
        self.retry_at.is_none_or(|at| now >= at)
    }

    /// The read failed. `true` if it is the first of the streak: the only one reported.
    pub fn failed(&mut self, now: Instant, base: Duration) -> bool {
        let first = self.failures == 0;
        let shift = self.failures.min(16);
        self.failures = self.failures.saturating_add(1);
        let wait = base
            .checked_mul(1u32 << shift)
            .map_or(Self::MAX, |d| d.min(Self::MAX));
        self.retry_at = Some(now + wait);
        first
    }

    /// The read succeeded. `Some(n)` if it closed a streak of `n` failures.
    pub fn succeeded(&mut self) -> Option<u32> {
        self.retry_at = None;
        match std::mem::take(&mut self.failures) {
            0 => None,
            n => Some(n),
        }
    }
}

pub struct ClipDriver<C> {
    sync: ClipSync,
    clipboard: C,
    /// Last seen value of the system counter.
    last_count: Option<u64>,
    focus: Option<watch::Receiver<bool>>,
    stats: watch::Sender<SyncStats>,
    cfg: ClipConfig,
}

impl<C: Clipboard> ClipDriver<C> {
    /// `focused`: whether this machine now has the keyboard and mouse.
    pub fn new(clipboard: C, cfg: ClipConfig, focused: bool) -> Self {
        Self {
            sync: ClipSync::new(cfg.sync, focused),
            clipboard,
            last_count: None,
            focus: None,
            stats: watch::Sender::new(SyncStats::default()),
            cfg,
        }
    }

    /// Live focus: `true` while the keyboard and mouse are on this
    /// machine. It is when the other side's large copies are requested.
    pub fn with_focus(mut self, mut focus: watch::Receiver<bool>) -> Self {
        let now = *focus.borrow_and_update();
        // Without a session there is nothing to send.
        let _ = self.sync.set_focus(now);
        self.focus = Some(focus);
        self
    }

    /// Synchronization counters, up to date while it runs.
    pub fn stats(&self) -> watch::Receiver<SyncStats> {
        self.stats.subscribe()
    }

    pub fn sync(&self) -> &ClipSync {
        &self.sync
    }

    pub fn clipboard(&self) -> &C {
        &self.clipboard
    }

    pub async fn run(&mut self, stream: ControlStream, stop: impl Future<Output = ()>) -> SyncEnd {
        let end = self.serve(stream, stop).await;
        self.stats.send_replace(self.sync.stats());
        tracing::info!("clipboard session ended: {end}");
        end
    }

    async fn serve(&mut self, stream: ControlStream, stop: impl Future<Output = ()>) -> SyncEnd {
        let (mut tx, mut rx) = stream.split();
        let (inbox_tx, mut inbox) = mpsc::unbounded_channel();
        // Reading is separate: while an image is being sent and flow control
        // slows the writing, what arrives from the other side keeps being read.
        let reader = async move {
            loop {
                match rx.recv::<ClipMsg>().await {
                    Ok(Some(msg)) => {
                        if inbox_tx.send(msg).is_err() {
                            return SyncEnd::Stopped;
                        }
                    }
                    Ok(None) => return TransportError::ControlClosed.into(),
                    Err(e) => return e.into(),
                }
            }
        };

        let Self {
            sync,
            clipboard,
            last_count,
            focus,
            stats,
            cfg,
        } = self;
        let mut backoff = ReadBackoff::default();
        let main = async {
            if let Some(f) = focus.as_mut() {
                let _ = sync.set_focus(*f.borrow_and_update());
            }
            let acts = match read_local(clipboard, last_count, &mut backoff, cfg.poll) {
                Some(local) => sync.start(local),
                None => sync.resume(),
            };
            apply(clipboard, last_count, &mut tx, acts).await?;
            stats.send_replace(sync.stats());

            let mut poll = time::interval(cfg.poll);
            poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    biased;
                    msg = inbox.recv() => {
                        // If the reader ended, the other arm of the select says so.
                        let Some(msg) = msg else { std::future::pending::<()>().await; continue };
                        let acts = sync.on_message(msg);
                        apply(clipboard, last_count, &mut tx, acts).await?;
                        stats.send_replace(sync.stats());
                    }
                    focused = next_focus(focus) => match focused {
                        Some(f) => {
                            let acts = sync.set_focus(f);
                            apply(clipboard, last_count, &mut tx, acts).await?;
                        stats.send_replace(sync.stats());
                        }
                        None => *focus = None,
                    },
                    _ = poll.tick() => {
                        if backoff.ready(Instant::now())
                            && changed(clipboard, last_count)
                            && let Some(local) = read_local(clipboard, last_count, &mut backoff, cfg.poll)
                        {
                            let acts = sync.local_changed(local);
                            apply(clipboard, last_count, &mut tx, acts).await?;
                        stats.send_replace(sync.stats());
                        }
                    }
                    () = std::future::ready(()), if sync.is_sending() => {
                        // One chunk per turn: between chunks the rest is attended to.
                        if let Some(chunk) = sync.next_chunk() {
                            tx.send(&chunk).await?;
                        }
                    }
                }
            }
        };
        let main = async {
            let r: Result<std::convert::Infallible, TransportError> = main.await;
            match r {
                Err(e) => SyncEnd::Link(e),
                Ok(never) => match never {},
            }
        };

        tokio::pin!(stop);
        tokio::select! {
            biased;
            () = &mut stop => SyncEnd::Stopped,
            end = reader => end,
            end = main => end,
        }
    }
}

async fn next_focus(focus: &mut Option<watch::Receiver<bool>>) -> Option<bool> {
    match focus {
        Some(rx) => match rx.changed().await {
            Ok(()) => Some(*rx.borrow_and_update()),
            Err(_) => None,
        },
        None => std::future::pending().await,
    }
}

/// Whether the system counter moved since the last time.
fn changed<C: Clipboard>(clipboard: &mut C, last: &Option<u64>) -> bool {
    match clipboard.change_count() {
        Ok(n) => Some(n) != *last,
        Err(e) => {
            tracing::debug!("portapapeles: contador: {e}");
            false
        }
    }
}

/// Reads the content and notes the counter that corresponds to it. The counter
/// is taken before reading: if it changes in the meantime, the next turn sees it.
/// `None` if it could not be read (another app has it open): it is retried on the
/// next turn.
fn read_local<C: Clipboard>(
    clipboard: &mut C,
    last: &mut Option<u64>,
    backoff: &mut ReadBackoff,
    base: Duration,
) -> Option<Option<ClipContent>> {
    match clipboard.change_count() {
        Ok(n) => *last = Some(n),
        Err(e) => tracing::debug!("portapapeles: contador: {e}"),
    }
    match clipboard.read() {
        Ok(c) => {
            if let Some(n) = backoff.succeeded() {
                tracing::info!("clipboard: could be read again after {n} attempts");
            }
            Some(c)
        }
        Err(e) => {
            // One notice per streak: the rest, to debug, with growing wait.
            if backoff.failed(Instant::now(), base) {
                tracing::warn!("clipboard: could not be read ({e}); retrying with backoff");
            } else {
                tracing::debug!("clipboard: still cannot be read: {e}");
            }
            *last = None;
            None
        }
    }
}

async fn apply<C: Clipboard>(
    clipboard: &mut C,
    last: &mut Option<u64>,
    tx: &mut ControlSender,
    actions: Vec<Action>,
) -> Result<(), TransportError> {
    for action in actions {
        match action {
            Action::Send(msg) => tx.send(&msg).await?,
            Action::Write(content) => {
                if let Err(e) = clipboard.write(&content) {
                    tracing::error!("clipboard: could not write: {e}");
                }
                // Our own write is not a user change: it is not even read.
                // If the system converted it and something else touches it again, the
                // hash stops it anyway in `ClipSync`. A user copy right
                // between the write and this read would be lost, but it is
                // microseconds and the user is not copying on this machine
                // while pasting what came from the other one.
                match clipboard.change_count() {
                    Ok(n) => *last = Some(n),
                    Err(e) => tracing::debug!("portapapeles: contador: {e}"),
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: Duration = Duration::from_millis(250);

    #[test]
    fn waits_double_each_time_up_to_the_cap() {
        let t0 = Instant::now();
        let mut b = ReadBackoff::default();
        assert!(b.ready(t0));
        let mut waits = Vec::new();
        for _ in 0..8 {
            b.failed(t0, BASE);
            // The first instant at which it is due: searches in 50 ms intervals.
            let w = (0..200u64)
                .map(|i| Duration::from_millis(i * 50))
                .find(|d| b.ready(t0 + *d))
                .unwrap();
            waits.push(w.as_millis());
        }
        assert_eq!(waits, [250, 500, 1000, 2000, 4000, 5000, 5000, 5000]);
    }

    #[test]
    fn only_the_first_failure_of_a_streak_is_reported() {
        let t0 = Instant::now();
        let mut b = ReadBackoff::default();
        assert!(b.failed(t0, BASE));
        assert!(!b.failed(t0, BASE));
        assert!(!b.failed(t0, BASE));
        assert_eq!(b.succeeded(), Some(3));
        // A new streak notifies once again.
        assert!(b.failed(t0, BASE));
        assert_eq!(b.succeeded(), Some(1));
        assert_eq!(b.succeeded(), None);
        assert!(b.ready(t0));
    }

    #[test]
    fn a_huge_streak_never_overflows() {
        let t0 = Instant::now();
        let mut b = ReadBackoff::default();
        for _ in 0..200 {
            b.failed(t0, Duration::from_secs(3600));
        }
        assert!(!b.ready(t0 + Duration::from_secs(4)));
        assert!(b.ready(t0 + ReadBackoff::MAX));
    }
}
