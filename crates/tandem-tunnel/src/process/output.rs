//! A process's output (`output`) built from its two flows, the same on the destination
//! (own flows) and on the origin (its mirror).

use tandem_proto::{FlowId, OutputCursor, ProcessOutput, TunnelProcess};

use crate::buffer::Read;

/// The most an `output` returns per flow: two flows plus the rest fit in one IPC
/// frame (1 MiB).
pub const OUTPUT_MAX: usize = 400 * 1024;

/// What a flow gives on one read.
pub(crate) struct FlowRead {
    pub read: Read,
    /// Position of the next byte to be written.
    pub end: u64,
    /// The flow receives no more.
    pub ended: bool,
}

struct Part {
    bytes: Vec<u8>,
    from: u64,
    next: u64,
    /// Everything was read and there will be no more.
    finished: bool,
}

fn part(
    flow: FlowId,
    from: u64,
    max: usize,
    read: &mut impl FnMut(FlowId, u64, usize) -> Option<FlowRead>,
) -> Part {
    let nothing = |at| Part {
        bytes: Vec::new(),
        from: at,
        next: at,
        finished: false,
    };
    // If what was asked for was already discarded, it continues from what remains: once, because the second
    // read already starts at the oldest there is.
    let mut at = from;
    for _ in 0..2 {
        let Some(r) = read(flow, at, max) else {
            return nothing(at);
        };
        match r.read {
            Read::Data { offset, bytes } => {
                let next = offset + bytes.len() as u64;
                return Part {
                    bytes,
                    from: offset,
                    next,
                    finished: r.ended && next >= r.end,
                };
            }
            Read::Lost { to, .. } => at = to,
            Read::Pending => {
                return Part {
                    finished: r.ended && at >= r.end,
                    ..nothing(at)
                };
            }
        }
    }
    nothing(at)
}

/// `read(flow, from, max)` gives the flow's state, or `None` if it is not yet known.
pub(crate) fn collect(
    process: TunnelProcess,
    from: OutputCursor,
    max: usize,
    mut read: impl FnMut(FlowId, u64, usize) -> Option<FlowRead>,
) -> ProcessOutput {
    let max = max.min(OUTPUT_MAX);
    let out = part(process.stdout, from.stdout, max, &mut read);
    let err = part(process.stderr, from.stderr, max, &mut read);
    ProcessOutput {
        done: !process.is_running() && out.finished && err.finished,
        process,
        from: OutputCursor {
            stdout: out.from,
            stderr: err.from,
        },
        next: OutputCursor {
            stdout: out.next,
            stderr: err.next,
        },
        stdout: out.bytes,
        stderr: err.bytes,
    }
}

#[cfg(test)]
mod tests {
    use tandem_proto::{ProcessId, ProcessState, TunnelId};

    use super::*;
    use crate::buffer::OutputBuffer;

    fn process(state: ProcessState) -> TunnelProcess {
        TunnelProcess {
            id: ProcessId(1),
            tunnel: TunnelId(1),
            command: "x".into(),
            shell: None,
            cwd: None,
            pid: None,
            stdout: FlowId(1),
            stderr: FlowId(2),
            started: 0,
            duration: None,
            state,
        }
    }

    fn flows(
        out: &OutputBuffer,
        err: &OutputBuffer,
        ended: bool,
    ) -> impl FnMut(FlowId, u64, usize) -> Option<FlowRead> {
        let (out, err) = (out.clone(), err.clone());
        move |f, from, max| {
            let b = if f == FlowId(1) { &out } else { &err };
            Some(FlowRead {
                read: b.read(from, max),
                end: b.end(),
                ended,
            })
        }
    }

    #[test]
    fn reads_each_flow_from_its_position_and_marks_the_end() {
        let (mut out, mut err) = (OutputBuffer::new(100), OutputBuffer::new(100));
        out.push(b"ahoy\n");
        err.push(b"bad\n");
        let p = process(ProcessState::Exited { code: Some(0) });
        let o = collect(
            p.clone(),
            OutputCursor::default(),
            100,
            flows(&out, &err, true),
        );
        assert_eq!(
            (o.stdout.as_slice(), o.stderr.as_slice()),
            (&b"ahoy\n"[..], &b"bad\n"[..])
        );
        assert_eq!(
            o.next,
            OutputCursor {
                stdout: 5,
                stderr: 4
            }
        );
        assert!(o.done);
        // From where the previous read ended there is nothing more, and it stays ended.
        let o2 = collect(p, o.next, 100, flows(&out, &err, true));
        assert!(o2.stdout.is_empty() && o2.stderr.is_empty() && o2.done);
        assert_eq!(o2.from, o.next);
    }

    #[test]
    fn running_or_with_pending_output_is_not_ended() {
        let (mut out, err) = (OutputBuffer::new(100), OutputBuffer::new(100));
        out.push(b"abc");
        let o = collect(
            process(ProcessState::Running),
            OutputCursor::default(),
            2,
            flows(&out, &err, false),
        );
        // `max` limits each flow.
        assert_eq!(o.stdout, b"ab");
        assert!(!o.done);
        // The process ended but there is output left to read: not yet.
        let o = collect(
            process(ProcessState::Killed),
            OutputCursor::default(),
            2,
            flows(&out, &err, true),
        );
        assert!(!o.done);
    }

    #[test]
    fn what_was_discarded_is_skipped_and_reported_in_from() {
        let (mut out, err) = (OutputBuffer::new(4), OutputBuffer::new(4));
        out.push(b"0123456789");
        let o = collect(
            process(ProcessState::Running),
            OutputCursor::default(),
            100,
            flows(&out, &err, false),
        );
        assert_eq!(o.stdout, b"6789");
        assert_eq!((o.from.stdout, o.next.stdout), (6, 10));
    }

    #[test]
    fn an_unknown_flow_gives_nothing() {
        let o = collect(
            process(ProcessState::Running),
            OutputCursor {
                stdout: 7,
                stderr: 3,
            },
            100,
            |_, _, _| None,
        );
        assert_eq!(
            (o.from, o.next),
            (
                OutputCursor {
                    stdout: 7,
                    stderr: 3
                },
                OutputCursor {
                    stdout: 7,
                    stderr: 3
                }
            )
        );
        assert!(!o.done);
    }
}
