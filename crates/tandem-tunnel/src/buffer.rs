//! Circular output buffer with absolute per-byte position.
//!
//! Each byte of a flow has its number (0, 1, 2…) since the flow was born. The buffer
//! keeps the last `capacity`: whoever reads asks "from byte N" and receives exactly what
//! follows, or the notice that a part was already discarded. Both sides use it: the destination
//! for what processes produce and the origin for what it already received (its mirror).

use std::collections::VecDeque;

/// What each flow keeps by default (DESIGN §8: 8 MiB per process).
pub const DEFAULT_CAPACITY: usize = 8 * 1024 * 1024;

/// What there is from a position onward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// `bytes` starts at `offset`.
    Data { offset: u64, bytes: Vec<u8> },
    /// `from..to` was already discarded; what comes next starts at `to`.
    Lost { from: u64, to: u64 },
    /// There is nothing yet from there.
    Pending,
}

#[derive(Debug, Clone)]
pub struct OutputBuffer {
    capacity: usize,
    /// Position of the first stored byte.
    start: u64,
    data: VecDeque<u8>,
    /// Discarded bytes (for lack of room or because of a gap the other side reported).
    dropped: u64,
}

impl OutputBuffer {
    /// `capacity` 0 is taken as 1: a buffer that stores nothing is useless for resuming.
    pub fn new(capacity: usize) -> Self {
        OutputBuffer {
            capacity: capacity.max(1),
            start: 0,
            data: VecDeque::new(),
            dropped: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Position of the first byte that can still be read.
    pub fn start(&self) -> u64 {
        self.start
    }

    /// Position of the next byte (the total written, counting what was discarded).
    pub fn end(&self) -> u64 {
        self.start + self.data.len() as u64
    }

    /// How many bytes were discarded without anyone reading them here.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Appends at the end; if it does not fit, discards the oldest.
    pub fn push(&mut self, bytes: &[u8]) {
        // What does not fit even with the buffer empty is discarded without copying it.
        let skip = bytes.len().saturating_sub(self.capacity);
        if skip > 0 {
            self.discard_front(self.data.len());
            self.start += skip as u64;
            self.dropped += skip as u64;
        }
        let bytes = &bytes[skip..];
        let over = (self.data.len() + bytes.len()).saturating_sub(self.capacity);
        self.discard_front(over);
        self.data.extend(bytes);
    }

    fn discard_front(&mut self, n: usize) {
        self.data.drain(..n);
        self.start += n as u64;
        self.dropped += n as u64;
    }

    /// Up to `max` bytes from `from`.
    pub fn read(&self, from: u64, max: usize) -> Read {
        if from < self.start {
            return Read::Lost {
                from,
                to: self.start,
            };
        }
        if from >= self.end() || max == 0 {
            return Read::Pending;
        }
        // `from - start` < `data.len()`, which is a `usize`.
        let skip = usize::try_from(from - self.start).unwrap_or(usize::MAX);
        let n = max.min(self.data.len() - skip);
        // Two slice copies instead of one push per byte: this is on the path of every
        // output message.
        let (a, b) = self.data.as_slices();
        let mut bytes = Vec::with_capacity(n);
        if skip < a.len() {
            let take = n.min(a.len() - skip);
            bytes.extend_from_slice(&a[skip..skip + take]);
            bytes.extend_from_slice(&b[..n - take]);
        } else {
            let at = skip - a.len();
            bytes.extend_from_slice(&b[at..at + n]);
        }
        Read::Data {
            offset: from,
            bytes,
        }
    }

    /// What arrives from the other side at `offset` (the origin's mirror): keeps only what
    /// it did not have yet, so a resend does not duplicate anything. Returns how many new
    /// bytes it added. If there is a gap before `offset`, it fails without touching anything: whoever calls
    /// has to ask from [`Self::end`] or accept the gap with [`Self::skip_to`].
    pub fn accept(&mut self, offset: u64, bytes: &[u8]) -> Result<usize, Gap> {
        let end = self.end();
        if offset > end {
            return Err(Gap { at: end, offset });
        }
        let have = usize::try_from(end - offset).unwrap_or(usize::MAX);
        let new = bytes.get(have..).unwrap_or_default();
        self.push(new);
        Ok(new.len())
    }

    /// Jumps to `to` giving up what is missing: what is stored is released (the buffer is
    /// contiguous) and is counted in [`Self::dropped`].
    pub fn skip_to(&mut self, to: u64) {
        let end = self.end();
        if to <= end {
            return;
        }
        // What is stored is released but not counted: it was already received, not lost.
        self.dropped += to - end;
        self.data.clear();
        self.start = to;
    }
}

/// Bytes arrived that start after the end of what is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("gap in the flow: held up to byte {at} and byte {offset} arrived")]
pub struct Gap {
    pub at: u64,
    pub offset: u64,
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn keeps_the_latest_and_reports_what_was_lost() {
        let mut b = OutputBuffer::new(4);
        b.push(b"abcdef");
        assert_eq!((b.start(), b.end(), b.dropped()), (2, 6, 2));
        assert_eq!(b.read(0, 10), Read::Lost { from: 0, to: 2 });
        assert_eq!(
            b.read(3, 10),
            Read::Data {
                offset: 3,
                bytes: b"def".to_vec()
            }
        );
        assert_eq!(b.read(6, 10), Read::Pending);
    }

    #[test]
    fn accept_does_not_duplicate_what_was_already_there() {
        let mut b = OutputBuffer::new(64);
        assert_eq!(b.accept(0, b"abcd"), Ok(4));
        assert_eq!(b.accept(2, b"cd world"), Ok(6));
        assert_eq!(b.accept(0, b"ab"), Ok(0));
        assert_eq!(b.accept(20, b"x"), Err(Gap { at: 10, offset: 20 }));
        assert_eq!(
            b.read(0, 64),
            Read::Data {
                offset: 0,
                bytes: b"abcd world".to_vec()
            }
        );
    }

    #[test]
    fn skipping_a_gap_counts_only_what_is_missing() {
        let mut b = OutputBuffer::new(64);
        b.push(b"abc");
        b.skip_to(10);
        assert_eq!((b.start(), b.end(), b.dropped()), (10, 10, 7));
        assert_eq!(b.read(3, 1), Read::Lost { from: 3, to: 10 });
        assert_eq!(b.accept(10, b"z"), Ok(1));
    }

    proptest! {
        /// What can be read is always exactly the tail of what was written, never more than
        /// the capacity, and what was discarded plus what is stored is the total.
        #[test]
        fn what_is_stored_is_the_tail_of_what_was_written(
            cap in 1usize..64,
            writes in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..40), 0..20),
        ) {
            let mut b = OutputBuffer::new(cap);
            let mut all = Vec::new();
            for w in &writes {
                b.push(w);
                all.extend_from_slice(w);
            }
            prop_assert_eq!(b.end(), all.len() as u64);
            prop_assert!(b.end() - b.start() <= cap as u64);
            prop_assert_eq!(b.dropped(), b.start());
            let tail = &all[usize::try_from(b.start()).unwrap()..];
            match b.read(b.start(), usize::MAX) {
                Read::Data { bytes, .. } => prop_assert_eq!(bytes.as_slice(), tail),
                Read::Pending => prop_assert!(tail.is_empty()),
                Read::Lost { .. } => prop_assert!(false, "from start there is never a loss"),
            }
        }

        /// Any window read gives exactly those bytes, also across the ring's wrap.
        #[test]
        fn any_window_reads_the_written_bytes(
            cap in 1usize..64,
            writes in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..40), 0..20),
            from in 0usize..400,
            max in 0usize..80,
        ) {
            let mut b = OutputBuffer::new(cap);
            let mut all = Vec::new();
            for w in &writes {
                b.push(w);
                all.extend_from_slice(w);
            }
            let from = from as u64;
            match b.read(from, max) {
                Read::Data { offset, bytes } => {
                    prop_assert_eq!(offset, from);
                    let at = usize::try_from(from).unwrap();
                    let n = max.min(all.len() - at);
                    prop_assert_eq!(bytes.as_slice(), &all[at..at + n]);
                }
                Read::Lost { to, .. } => prop_assert_eq!(to, b.start()),
                Read::Pending => prop_assert!(from >= b.end() || max == 0),
            }
        }

        /// Receiving overlapping pieces (resends after a cut) gives the original.
        #[test]
        fn overlapping_resends_give_the_original(
            data in prop::collection::vec(any::<u8>(), 0..300),
            cuts in prop::collection::vec((0usize..300, 0usize..300), 0..30),
        ) {
            let mut b = OutputBuffer::new(1024);
            for (a, len) in cuts {
                let from = a.min(usize::try_from(b.end()).unwrap()).min(data.len());
                let to = (from + len).min(data.len());
                b.accept(from as u64, &data[from..to]).unwrap();
            }
            let at = usize::try_from(b.end()).unwrap();
            b.accept(at as u64, &data[at..]).unwrap();
            prop_assert_eq!(b.read(0, usize::MAX), if data.is_empty() {
                Read::Pending
            } else {
                Read::Data { offset: 0, bytes: data.clone() }
            });
        }
    }
}
