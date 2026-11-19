//! The helper's work for each request of the session agent (`tandemd --input-helper`).
//!
//! The Windows side (`windows::helper`) accepts the pipe's clients one after another and
//! hands each stream to [`Helper::serve`]; everything it decides is here, over an
//! [`InputSink`] and the [`Desktops`] of the tracker, so it is tested with doubles.

use std::io::{self, Read, Write};
use std::time::Instant;

use tandem_input::windows::emulate::{InputSink, WinInput};

use super::desktop::{DesktopTracker, Desktops};
use super::protocol::{PROTOCOL_VERSION, Reply, Request, read_frame, write_frame};

pub struct Helper<S: InputSink, D: Desktops> {
    sink: S,
    tracker: DesktopTracker<D>,
    /// Passes a Ctrl+Alt+Del request to the service.
    sas: Box<dyn FnMut() -> io::Result<()>>,
}

impl<S: InputSink, D: Desktops> Helper<S, D> {
    pub fn new(
        sink: S,
        tracker: DesktopTracker<D>,
        sas: Box<dyn FnMut() -> io::Result<()>>,
    ) -> Self {
        Helper { sink, tracker, sas }
    }

    pub fn tracker(&self) -> &DesktopTracker<D> {
        &self.tracker
    }

    pub fn handle(&mut self, request: Request, now: Instant) -> Reply {
        match request {
            Request::Hello { protocol } if protocol == PROTOCOL_VERSION => Reply::Ok,
            Request::Hello { .. } => Reply::Version {
                protocol: PROTOCOL_VERSION,
            },
            Request::Inject(wire) => {
                let inputs: Vec<WinInput> = wire.iter().map(WinInput::from).collect();
                Reply::from_send(self.inject(&inputs, now))
            }
            Request::Sas => match (self.sas)() {
                Ok(()) => Reply::Ok,
                Err(e) => Reply::Failed(e.to_string()),
            },
        }
    }

    fn inject(
        &mut self,
        inputs: &[WinInput],
        now: Instant,
    ) -> Result<(), tandem_input::windows::emulate::EmulateError> {
        if let Err(e) = self.tracker.before_send(now) {
            tracing::warn!(error = %e, "could not follow the input desktop");
        }
        let first = self.sink.send(inputs);
        if first.is_ok() {
            return first;
        }
        // A failure is what a desktop switch looks like: if the thread moved, once more.
        match self.tracker.after_failure(now) {
            Ok(true) => self.sink.send(inputs),
            Ok(false) => first,
            Err(e) => {
                tracing::warn!(error = %e, "could not follow the input desktop");
                first
            }
        }
    }

    /// Serves one client until it closes. The first request has to be the hello: a client
    /// that does not know the protocol gets nothing injected.
    pub fn serve(&mut self, stream: &mut (impl Read + Write)) -> io::Result<()> {
        let mut greeted = false;
        while let Some(request) = read_frame::<Request>(stream)? {
            let is_hello = matches!(request, Request::Hello { .. });
            let reply = if greeted || is_hello {
                self.handle(request, Instant::now())
            } else {
                Reply::Failed("the first request must be the hello".into())
            };
            greeted |= is_hello && reply == Reply::Ok;
            write_frame(stream, &reply)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use tandem_input::windows::TANDEM_EXTRA_INFO;
    use tandem_input::windows::emulate::{Desktop, EmulateError};

    use super::super::desktop::CHECK_INTERVAL;
    use super::super::protocol::WireInput;
    use super::*;

    /// The window station: which desktop has the input and which one the thread is on.
    #[derive(Default)]
    struct Station {
        input: String,
        attached: Option<String>,
        /// `SendInput` fails even on the right desktop (an elevated window, for instance).
        blocked: bool,
    }

    struct Desks(Rc<RefCell<Station>>);

    impl Desktops for Desks {
        fn input_name(&mut self) -> io::Result<String> {
            Ok(self.0.borrow().input.clone())
        }
        fn attach_input(&mut self) -> io::Result<String> {
            let mut st = self.0.borrow_mut();
            st.attached = Some(st.input.clone());
            Ok(st.input.clone())
        }
    }

    /// Injects only while the thread is on the desktop that has the input, like `SendInput`.
    struct DeskSink {
        station: Rc<RefCell<Station>>,
        sent: Rc<RefCell<Vec<WinInput>>>,
    }

    impl InputSink for DeskSink {
        fn send(&mut self, inputs: &[WinInput]) -> Result<(), EmulateError> {
            let st = self.station.borrow();
            if st.blocked || st.attached.as_deref() != Some(st.input.as_str()) {
                return Err(EmulateError::Blocked {
                    sent: 0,
                    expected: inputs.len(),
                });
            }
            self.sent.borrow_mut().extend_from_slice(inputs);
            Ok(())
        }
        fn desktop(&self) -> Result<Desktop, EmulateError> {
            Err(EmulateError::NoDesktop)
        }
        fn cursor_pos(&self) -> Option<(i32, i32)> {
            None
        }
    }

    fn mouse() -> WireInput {
        WireInput::Mouse {
            dx: 3,
            dy: 4,
            data: 0,
            flags: 1,
            extra_info: TANDEM_EXTRA_INFO as u64,
        }
    }

    struct Rig {
        helper: Helper<DeskSink, Desks>,
        station: Rc<RefCell<Station>>,
        sent: Rc<RefCell<Vec<WinInput>>>,
        sas: Rc<RefCell<usize>>,
    }

    fn rig() -> Rig {
        let station = Rc::new(RefCell::new(Station {
            input: "Default".into(),
            ..Default::default()
        }));
        let sent = Rc::new(RefCell::new(Vec::new()));
        let sas = Rc::new(RefCell::new(0));
        let counter = sas.clone();
        let helper = Helper::new(
            DeskSink {
                station: station.clone(),
                sent: sent.clone(),
            },
            DesktopTracker::new(Desks(station.clone()), CHECK_INTERVAL),
            Box::new(move || {
                *counter.borrow_mut() += 1;
                Ok(())
            }),
        );
        Rig {
            helper,
            station,
            sent,
            sas,
        }
    }

    #[test]
    fn hello_checks_the_version() {
        let mut r = rig();
        let t = Instant::now();
        let hello = |protocol| Request::Hello { protocol };
        assert_eq!(r.helper.handle(hello(PROTOCOL_VERSION), t), Reply::Ok);
        assert_eq!(
            r.helper.handle(hello(PROTOCOL_VERSION + 1), t),
            Reply::Version {
                protocol: PROTOCOL_VERSION
            }
        );
    }

    #[test]
    fn injects_on_the_input_desktop() {
        let mut r = rig();
        let t = Instant::now();
        assert_eq!(
            r.helper.handle(Request::Inject(vec![mouse()]), t),
            Reply::Ok
        );
        assert_eq!(r.sent.borrow().len(), 1);
        assert_eq!(r.helper.tracker().current(), Some("Default"));
    }

    #[test]
    fn a_uac_prompt_is_followed_and_the_batch_is_retried_once() {
        let mut r = rig();
        let t = Instant::now();
        r.helper.handle(Request::Inject(vec![mouse()]), t);
        // UAC moves the input to Winlogon right after a check: the first try fails, the
        // tracker moves the thread and the retry lands on the prompt.
        r.station.borrow_mut().input = "Winlogon".into();
        let reply = r.helper.handle(
            Request::Inject(vec![mouse()]),
            t + Duration::from_millis(10),
        );
        assert_eq!(reply, Reply::Ok);
        assert_eq!(r.helper.tracker().current(), Some("Winlogon"));
        assert_eq!(r.sent.borrow().len(), 2);
        // Back to the normal desktop when it closes, on the periodic check.
        r.station.borrow_mut().input = "Default".into();
        let reply = r.helper.handle(
            Request::Inject(vec![mouse()]),
            t + Duration::from_millis(200),
        );
        assert_eq!(reply, Reply::Ok);
        assert_eq!(r.helper.tracker().current(), Some("Default"));
    }

    #[test]
    fn a_failure_on_the_same_desktop_is_reported_without_a_retry() {
        let mut r = rig();
        let t = Instant::now();
        r.helper.handle(Request::Inject(vec![mouse()]), t);
        r.station.borrow_mut().blocked = true;
        assert_eq!(
            r.helper.handle(Request::Inject(vec![mouse()]), t),
            Reply::Blocked {
                sent: 0,
                expected: 1
            }
        );
        assert_eq!(r.sent.borrow().len(), 1);
    }

    #[test]
    fn sas_goes_to_the_service() {
        let mut r = rig();
        assert_eq!(r.helper.handle(Request::Sas, Instant::now()), Reply::Ok);
        assert_eq!(*r.sas.borrow(), 1);
        r.helper.sas = Box::new(|| Err(io::Error::other("the service is gone")));
        assert!(matches!(
            r.helper.handle(Request::Sas, Instant::now()),
            Reply::Failed(_)
        ));
    }

    /// A stream with the client's requests already written; the replies stay in `output`.
    struct Script {
        input: io::Cursor<Vec<u8>>,
        output: Vec<u8>,
    }
    impl Read for Script {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }
    impl Write for Script {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn script(requests: &[Request]) -> Script {
        let mut input = Vec::new();
        for r in requests {
            write_frame(&mut input, r).unwrap();
        }
        Script {
            input: io::Cursor::new(input),
            output: Vec::new(),
        }
    }

    fn replies(s: &Script) -> Vec<Reply> {
        let mut out = Vec::new();
        let mut r = s.output.as_slice();
        while let Some(reply) = read_frame(&mut r).unwrap() {
            out.push(reply);
        }
        out
    }

    #[test]
    fn serve_requires_the_hello_first() {
        let Rig {
            helper: mut h,
            sent,
            ..
        } = rig();
        let mut s = script(&[
            Request::Inject(vec![mouse()]),
            Request::Hello {
                protocol: PROTOCOL_VERSION,
            },
            Request::Inject(vec![mouse()]),
        ]);
        h.serve(&mut s).unwrap();
        let r = replies(&s);
        assert!(matches!(r[0], Reply::Failed(_)));
        assert_eq!(r[1..], [Reply::Ok, Reply::Ok]);
        assert_eq!(sent.borrow().len(), 1, "only after the hello");
    }

    #[test]
    fn serve_with_the_wrong_version_injects_nothing() {
        let Rig {
            helper: mut h,
            sent,
            ..
        } = rig();
        let mut s = script(&[
            Request::Hello {
                protocol: PROTOCOL_VERSION + 1,
            },
            Request::Inject(vec![mouse()]),
        ]);
        h.serve(&mut s).unwrap();
        assert!(sent.borrow().is_empty());
    }
}
