//! The session agent's `SendInput`: through the helper when it is up, locally when not.
//!
//! [`RoutedSink`] is the [`InputSink`] the agent's emulator writes to on Windows. The
//! emulator keeps its state (cursor position, pressed keys, screens) and only the final
//! `INPUT` records cross to the helper, which injects them on whatever desktop has the input.
//! Going through the helper also on the normal desktop is deliberate: as SYSTEM it is not
//! stopped by UIPI over elevated windows, which the agent's own `SendInput` is.
//!
//! When the helper is missing (service not installed, stopped, another session, another
//! version) it injects locally as before and tries to reconnect every
//! [`RECONNECT_EVERY`], so a missing service costs nothing per event.
//!
//! Ctrl+Alt+Del cannot be injected: Windows ignores it unless it comes from `SendSAS`.
//! [`SasChord`] spots it in the records and the sink asks the helper for a secure attention
//! sequence instead of sending the Delete.

use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use tandem_input::windows::emulate::{
    Desktop, EmulateError, InputSink, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
    WinInput,
};

use super::SecureStatus;
use super::protocol::{PROTOCOL_VERSION, Reply, Request, WireInput, read_frame, write_frame};

/// Time between connection attempts while the helper is missing.
pub const RECONNECT_EVERY: Duration = Duration::from_secs(2);

/// Why the helper could not be reached.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("the service is not installed")]
    NotInstalled,
    #[error("the helper is not running in this session")]
    NotRunning,
    #[error("access to the helper denied (the agent is not elevated)")]
    NoAccess,
    /// The pipe exists but its server is not SYSTEM: someone else squats the name.
    #[error("the pipe's server is not the Tandem helper: {0}")]
    Untrusted(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl ConnectError {
    fn status(&self) -> SecureStatus {
        match self {
            ConnectError::NotInstalled => SecureStatus::NotInstalled,
            ConnectError::NoAccess => SecureStatus::NoAccess,
            ConnectError::NotRunning | ConnectError::Untrusted(_) | ConnectError::Io(_) => {
                SecureStatus::NotRunning
            }
        }
    }
}

/// One request and its reply.
pub trait HelperLink: Send {
    fn call(&mut self, request: &Request) -> io::Result<Reply>;
}

/// Opens links to this session's helper.
pub trait HelperConnector: Send {
    fn connect(&mut self) -> Result<Box<dyn HelperLink>, ConnectError>;
}

/// A [`HelperLink`] over any byte stream (the pipe on Windows, memory in the tests).
pub struct FramedLink<S: Read + Write + Send>(pub S);

impl<S: Read + Write + Send> HelperLink for FramedLink<S> {
    fn call(&mut self, request: &Request) -> io::Result<Reply> {
        write_frame(&mut self.0, request)?;
        read_frame(&mut self.0)?.ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))
    }
}

const SC_CTRL: u16 = 0x1D;
const SC_ALT: u16 = 0x38;
/// Delete (`E0 53`) and the keypad's Del (`53`): both make the chord.
const SC_DELETE: u16 = 0x53;

/// Spots Ctrl+Alt+Delete in the records that go out.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SasChord {
    /// Left and right of each (`[left, right]`).
    ctrl: [bool; 2],
    alt: [bool; 2],
    /// The Delete whose press became a SAS: its release is dropped too.
    swallowed: Option<bool>,
}

/// What [`SasChord::split`] leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    pub inputs: Vec<WinInput>,
    pub sas: bool,
}

impl SasChord {
    /// Updates the modifiers with `inputs` and takes out the Delete that completes the
    /// chord (and its release). The rest goes out in the same order.
    pub fn split(&mut self, inputs: &[WinInput]) -> Split {
        let mut out = Vec::with_capacity(inputs.len());
        let mut sas = false;
        for input in inputs {
            let WinInput::Keyboard { scan, flags, .. } = *input else {
                out.push(*input);
                continue;
            };
            if flags & KEYEVENTF_SCANCODE == 0 {
                out.push(*input);
                continue;
            }
            let down = flags & KEYEVENTF_KEYUP == 0;
            let side = usize::from(flags & KEYEVENTF_EXTENDEDKEY != 0);
            match scan {
                SC_CTRL => self.ctrl[side] = down,
                SC_ALT => self.alt[side] = down,
                SC_DELETE => {
                    let extended = side == 1;
                    if down && self.ctrl.contains(&true) && self.alt.contains(&true) {
                        self.swallowed = Some(extended);
                        sas = true;
                        continue;
                    }
                    if !down && self.swallowed == Some(extended) {
                        self.swallowed = None;
                        continue;
                    }
                }
                _ => {}
            }
            out.push(*input);
        }
        Split { inputs: out, sas }
    }
}

pub struct RoutedSink<L: InputSink> {
    local: L,
    connector: Box<dyn HelperConnector>,
    link: Option<Box<dyn HelperLink>>,
    next_attempt: Option<Instant>,
    chord: SasChord,
    status: SecureStatus,
    /// Where status changes go: the process-wide value the module reads.
    report: fn(SecureStatus),
}

impl<L: InputSink> RoutedSink<L> {
    pub fn new(local: L, connector: Box<dyn HelperConnector>) -> Self {
        RoutedSink {
            local,
            connector,
            link: None,
            next_attempt: None,
            chord: SasChord::default(),
            status: SecureStatus::Unknown,
            report: super::report_status,
        }
    }

    /// With another destination for the status (tests: not the process-wide one).
    pub fn with_reporter(mut self, report: fn(SecureStatus)) -> Self {
        self.report = report;
        self
    }

    pub fn status(&self) -> SecureStatus {
        self.status
    }

    pub fn local(&self) -> &L {
        &self.local
    }

    fn set_status(&mut self, status: SecureStatus) {
        if self.status != status {
            self.status = status;
            (self.report)(status);
        }
    }

    /// The link to the helper, connecting if it is time to try.
    fn ensure_link(&mut self, now: Instant) {
        if self.link.is_some() || self.next_attempt.is_some_and(|t| now < t) {
            return;
        }
        self.next_attempt = Some(now + RECONNECT_EVERY);
        let mut link = match self.connector.connect() {
            Ok(link) => link,
            Err(e) => {
                tracing::debug!(error = %e, "secure desktop helper not reachable");
                self.set_status(e.status());
                return;
            }
        };
        match link.call(&Request::Hello {
            protocol: PROTOCOL_VERSION,
        }) {
            Ok(Reply::Ok) => {
                self.link = Some(link);
                self.set_status(SecureStatus::Connected);
            }
            Ok(Reply::Version { protocol }) => {
                tracing::warn!(
                    helper = protocol,
                    agent = PROTOCOL_VERSION,
                    "the secure desktop service is from another version"
                );
                self.set_status(SecureStatus::Outdated);
            }
            Ok(other) => {
                tracing::warn!(?other, "unexpected reply to the helper hello");
                self.set_status(SecureStatus::NotRunning);
            }
            Err(e) => {
                tracing::debug!(error = %e, "helper hello failed");
                self.set_status(SecureStatus::NotRunning);
            }
        }
    }

    /// Sends `request` to the helper. `None` if there is no link or it broke (then it is
    /// dropped and a reconnection is due).
    fn call(&mut self, request: &Request) -> Option<Reply> {
        let link = self.link.as_mut()?;
        match link.call(request) {
            Ok(reply) => Some(reply),
            Err(e) => {
                tracing::info!(error = %e, "lost the secure desktop helper; injecting locally");
                self.link = None;
                self.next_attempt = None;
                self.set_status(SecureStatus::NotRunning);
                None
            }
        }
    }

    /// [`InputSink::send`] at the instant `now`.
    pub fn send_at(&mut self, inputs: &[WinInput], now: Instant) -> Result<(), EmulateError> {
        self.ensure_link(now);
        let split = if self.link.is_some() {
            self.chord.split(inputs)
        } else {
            // Without a helper there is nobody to send the SAS: the keys go out as they are,
            // but the modifiers are still followed for when it comes back.
            self.chord.split(inputs);
            Split {
                inputs: inputs.to_vec(),
                sas: false,
            }
        };
        let mut result = Ok(());
        if !split.inputs.is_empty() {
            let wire = split.inputs.iter().map(WireInput::from).collect();
            result = match self.call(&Request::Inject(wire)) {
                Some(reply) => reply_to_result(reply, split.inputs.len()),
                None => self.local.send(&split.inputs),
            };
        }
        if split.sas {
            match self.call(&Request::Sas) {
                Some(Reply::Ok) => tracing::info!("Ctrl+Alt+Del sent"),
                Some(other) => tracing::warn!(?other, "the helper refused Ctrl+Alt+Del"),
                None => tracing::warn!("Ctrl+Alt+Del lost: the helper went away"),
            }
        }
        result
    }
}

fn reply_to_result(reply: Reply, expected: usize) -> Result<(), EmulateError> {
    match reply {
        Reply::Ok => Ok(()),
        Reply::Blocked { sent, expected } => Err(EmulateError::Blocked {
            sent: sent as usize,
            expected: expected as usize,
        }),
        Reply::Failed(message) => Err(EmulateError::Os {
            op: "helper SendInput",
            code: 0,
            message,
        }),
        Reply::Version { .. } => Err(EmulateError::Blocked { sent: 0, expected }),
    }
}

impl<L: InputSink> InputSink for RoutedSink<L> {
    fn send(&mut self, inputs: &[WinInput]) -> Result<(), EmulateError> {
        self.send_at(inputs, Instant::now())
    }

    fn desktop(&self) -> Result<Desktop, EmulateError> {
        self.local.desktop()
    }

    fn cursor_pos(&self) -> Option<(i32, i32)> {
        self.local.cursor_pos()
    }

    // The cursor is drawn by the session's desktop, whoever injects: MouseKeys is a
    // system setting, set from here.
    fn cursor_hidden(&self) -> bool {
        self.local.cursor_hidden()
    }

    fn force_cursor(&mut self, on: bool) -> Result<(), EmulateError> {
        self.local.force_cursor(on)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tandem_input::windows::TANDEM_EXTRA_INFO;

    use super::*;

    fn one_screen() -> Desktop {
        let r = tandem_input::Rect::new(0, 0, 1920, 1080);
        Desktop {
            virtual_rect: r,
            monitors: vec![r],
        }
    }

    /// What reached each side, shared with the test.
    #[derive(Debug, Default)]
    struct Seen {
        local: Vec<WinInput>,
        forced: Vec<bool>,
        helper: Vec<Request>,
        connects: usize,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        Up,
        Missing,
        Denied,
        OtherVersion,
        /// Accepts the hello and then breaks.
        Breaks,
    }

    struct Fake {
        seen: Arc<Mutex<Seen>>,
        mode: Arc<Mutex<Mode>>,
    }

    struct FakeLink {
        seen: Arc<Mutex<Seen>>,
        mode: Arc<Mutex<Mode>>,
    }

    impl HelperLink for FakeLink {
        fn call(&mut self, request: &Request) -> io::Result<Reply> {
            let mode = *self.mode.lock().unwrap();
            if let Request::Hello { .. } = request {
                return Ok(match mode {
                    Mode::OtherVersion => Reply::Version { protocol: 99 },
                    _ => Reply::Ok,
                });
            }
            if mode == Mode::Breaks {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            self.seen.lock().unwrap().helper.push(request.clone());
            Ok(Reply::Ok)
        }
    }

    impl HelperConnector for Fake {
        fn connect(&mut self) -> Result<Box<dyn HelperLink>, ConnectError> {
            self.seen.lock().unwrap().connects += 1;
            match *self.mode.lock().unwrap() {
                Mode::Missing => Err(ConnectError::NotRunning),
                Mode::Denied => Err(ConnectError::NoAccess),
                _ => Ok(Box::new(FakeLink {
                    seen: self.seen.clone(),
                    mode: self.mode.clone(),
                })),
            }
        }
    }

    struct Local(Arc<Mutex<Seen>>);

    impl InputSink for Local {
        fn send(&mut self, inputs: &[WinInput]) -> Result<(), EmulateError> {
            self.0.lock().unwrap().local.extend_from_slice(inputs);
            Ok(())
        }
        fn desktop(&self) -> Result<Desktop, EmulateError> {
            Ok(one_screen())
        }
        fn cursor_pos(&self) -> Option<(i32, i32)> {
            Some((7, 8))
        }
        fn cursor_hidden(&self) -> bool {
            true
        }
        fn force_cursor(&mut self, on: bool) -> Result<(), EmulateError> {
            self.0.lock().unwrap().forced.push(on);
            Ok(())
        }
    }

    /// Without this, the wrapper swallowed the call and the PC's cursor stayed invisible.
    #[test]
    fn drawing_the_cursor_reaches_the_local_sink() {
        let (mut s, seen, _) = sink(Mode::Missing);
        assert!(s.cursor_hidden());
        s.force_cursor(true).unwrap();
        s.force_cursor(false).unwrap();
        assert_eq!(seen.lock().unwrap().forced, vec![true, false]);
    }

    fn sink(mode: Mode) -> (RoutedSink<Local>, Arc<Mutex<Seen>>, Arc<Mutex<Mode>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mode = Arc::new(Mutex::new(mode));
        let fake = Fake {
            seen: seen.clone(),
            mode: mode.clone(),
        };
        (
            RoutedSink::new(Local(seen.clone()), Box::new(fake)).with_reporter(|_| {}),
            seen,
            mode,
        )
    }

    fn key(scan: u16, extended: bool, down: bool) -> WinInput {
        let mut flags = KEYEVENTF_SCANCODE;
        if extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if !down {
            flags |= KEYEVENTF_KEYUP;
        }
        WinInput::Keyboard {
            vk: 0,
            scan,
            flags,
            extra_info: TANDEM_EXTRA_INFO,
        }
    }

    fn mouse() -> WinInput {
        WinInput::Mouse {
            dx: 1,
            dy: 2,
            data: 0,
            flags: 1,
            extra_info: TANDEM_EXTRA_INFO,
        }
    }

    fn injected(seen: &Seen) -> Vec<WinInput> {
        seen.helper
            .iter()
            .filter_map(|r| match r {
                Request::Inject(v) => Some(v.iter().map(WinInput::from).collect::<Vec<_>>()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn goes_through_the_helper_when_it_is_up() {
        let t = Instant::now();
        let (mut s, seen, _) = sink(Mode::Up);
        s.send_at(&[mouse()], t).unwrap();
        let seen = seen.lock().unwrap();
        assert!(seen.local.is_empty());
        assert_eq!(injected(&seen), [mouse()]);
        assert_eq!(s.status(), SecureStatus::Connected);
    }

    #[test]
    fn falls_back_to_local_and_retries_only_every_few_seconds() {
        let t = Instant::now();
        let (mut s, seen, mode) = sink(Mode::Missing);
        for i in 0..10 {
            s.send_at(&[mouse()], t + Duration::from_millis(i * 10))
                .unwrap();
        }
        assert_eq!(seen.lock().unwrap().local.len(), 10);
        assert_eq!(seen.lock().unwrap().connects, 1);
        assert_eq!(s.status(), SecureStatus::NotRunning);
        // The service comes up: the next attempt connects.
        *mode.lock().unwrap() = Mode::Up;
        s.send_at(&[mouse()], t + RECONNECT_EVERY).unwrap();
        assert_eq!(injected(&seen.lock().unwrap()).len(), 1);
        assert_eq!(s.status(), SecureStatus::Connected);
    }

    #[test]
    fn a_broken_link_falls_back_without_losing_the_batch() {
        let t = Instant::now();
        let (mut s, seen, mode) = sink(Mode::Up);
        s.send_at(&[mouse()], t).unwrap();
        *mode.lock().unwrap() = Mode::Breaks;
        s.send_at(&[key(0x1E, false, true)], t).unwrap();
        assert_eq!(seen.lock().unwrap().local, [key(0x1E, false, true)]);
        assert_eq!(s.status(), SecureStatus::NotRunning);
    }

    #[test]
    fn an_unelevated_agent_or_another_version_reports_why() {
        let t = Instant::now();
        let (mut s, seen, _) = sink(Mode::Denied);
        s.send_at(&[mouse()], t).unwrap();
        assert_eq!(s.status(), SecureStatus::NoAccess);
        assert_eq!(seen.lock().unwrap().local.len(), 1);
        let (mut s, seen, _) = sink(Mode::OtherVersion);
        s.send_at(&[mouse()], t).unwrap();
        assert_eq!(s.status(), SecureStatus::Outdated);
        assert_eq!(seen.lock().unwrap().local.len(), 1, "it injects locally");
    }

    #[test]
    fn ctrl_alt_delete_becomes_a_sas_request() {
        let t = Instant::now();
        let (mut s, seen, _) = sink(Mode::Up);
        for i in [
            key(SC_CTRL, false, true),
            key(SC_ALT, false, true),
            key(SC_DELETE, true, true),
            key(SC_DELETE, true, false),
            key(SC_ALT, false, false),
            key(SC_CTRL, false, false),
        ] {
            s.send_at(&[i], t).unwrap();
        }
        let seen = seen.lock().unwrap();
        assert!(seen.helper.contains(&Request::Sas));
        let keys = injected(&seen);
        assert!(
            !keys.iter().any(|k| matches!(
                k,
                WinInput::Keyboard {
                    scan: SC_DELETE,
                    ..
                }
            )),
            "the Delete never reaches SendInput: {keys:?}"
        );
        assert_eq!(keys.len(), 4, "the modifiers go out and are released");
    }

    #[test]
    fn delete_alone_or_with_one_modifier_is_a_normal_key() {
        let mut chord = SasChord::default();
        let plain = [key(SC_DELETE, true, true), key(SC_DELETE, true, false)];
        assert_eq!(
            chord.split(&plain),
            Split {
                inputs: plain.to_vec(),
                sas: false
            }
        );
        let ctrl_only = [
            key(SC_CTRL, true, true),
            key(SC_DELETE, true, true),
            key(SC_DELETE, true, false),
            key(SC_CTRL, true, false),
        ];
        assert!(!chord.split(&ctrl_only).sas);
        // Right Ctrl + AltGr + keypad Del also counts.
        let split = chord.split(&[
            key(SC_CTRL, true, true),
            key(SC_ALT, true, true),
            key(SC_DELETE, false, true),
        ]);
        assert!(split.sas);
        assert_eq!(split.inputs.len(), 2);
    }

    #[test]
    fn without_a_helper_ctrl_alt_delete_goes_out_as_keys() {
        let t = Instant::now();
        let (mut s, seen, _) = sink(Mode::Missing);
        s.send_at(
            &[
                key(SC_CTRL, false, true),
                key(SC_ALT, false, true),
                key(SC_DELETE, true, true),
            ],
            t,
        )
        .unwrap();
        assert_eq!(seen.lock().unwrap().local.len(), 3);
    }

    #[test]
    fn screens_and_cursor_are_read_locally() {
        let (s, _, _) = sink(Mode::Up);
        assert_eq!(s.desktop().unwrap(), one_screen());
        assert_eq!(s.cursor_pos(), Some((7, 8)));
    }

    #[test]
    fn framed_link_talks_over_a_byte_stream() {
        // The "helper" answers Ok to whatever: the reply is already in the stream.
        struct Duplex {
            input: io::Cursor<Vec<u8>>,
            output: Vec<u8>,
        }
        impl Read for Duplex {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.input.read(buf)
            }
        }
        impl Write for Duplex {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.output.write(buf)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut replies = Vec::new();
        write_frame(&mut replies, &Reply::Ok).unwrap();
        let mut link = FramedLink(Duplex {
            input: io::Cursor::new(replies),
            output: Vec::new(),
        });
        assert_eq!(link.call(&Request::Sas).unwrap(), Reply::Ok);
        let sent: Request = read_frame(&mut link.0.output.as_slice()).unwrap().unwrap();
        assert_eq!(sent, Request::Sas);
        // No more replies: the helper closed.
        assert!(link.call(&Request::Sas).is_err());
    }

    proptest::proptest! {
        /// Whatever the keys, nothing but Deletes is ever dropped, and a SAS only appears
        /// when Ctrl and Alt are held.
        #[test]
        fn only_deletes_are_dropped(
            keys in proptest::collection::vec((0u8..4, proptest::bool::ANY, proptest::bool::ANY), 0..40)
        ) {
            let mut chord = SasChord::default();
            let inputs: Vec<WinInput> = keys
                .iter()
                .map(|&(k, ext, down)| {
                    let scan = [SC_CTRL, SC_ALT, SC_DELETE, 0x1E][usize::from(k)];
                    key(scan, ext, down)
                })
                .collect();
            let split = chord.split(&inputs);
            let kept_other = split.inputs.iter().filter(|i| !matches!(i, WinInput::Keyboard { scan: SC_DELETE, .. })).count();
            let all_other = inputs.iter().filter(|i| !matches!(i, WinInput::Keyboard { scan: SC_DELETE, .. })).count();
            proptest::prop_assert_eq!(kept_other, all_other);
            let delete_down = inputs.iter().any(|i| matches!(i, WinInput::Keyboard { scan: SC_DELETE, flags, .. } if flags & KEYEVENTF_KEYUP == 0));
            proptest::prop_assert!(!split.sas || delete_down);
        }
    }
}
