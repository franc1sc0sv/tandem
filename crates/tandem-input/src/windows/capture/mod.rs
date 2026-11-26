//! Keyboard and mouse capture on Windows (docs/research/input-engine.md).
//!
//! Two threads:
//!
//! - **Hook thread** (`cfg(windows)`): installs `WH_KEYBOARD_LL` and `WH_MOUSE_LL` and
//!   receives Raw Input (`RIDEV_INPUTSINK`) in a message-only window. Each
//!   callback copies the system structure, decides whether the event is swallowed
//!   ([`HookFilter`]) and queues it without blocking. Windows silently removes a hook
//!   that takes longer than `LowLevelHooksTimeout`, so nothing else is done there.
//! - **Process thread** ([`run_pipeline`], portable): translates into [`InputEvent`]
//!   ([`Translator`]: injected filter, AltGr's fake Ctrl, keys by
//!   scancode, Raw Input deltas) and runs the [`Watchdog`], which compares the
//!   hooks with Raw Input from real devices and reinstalls them if Windows
//!   removed them. It never injects anything.
//!
//! Contract with the session (T-026): while input is not swallowed, movement arrives as
//! `MotionRelative` from Raw Input (without acceleration, in physical pixels) and as
//! `MotionAbsolute` with the cursor's real position. While it is swallowed (the focus is
//! on the other machine), as `MotionRelative` measured by the hook: the point where
//! Windows would have taken the cursor minus where it is. That delta already carries
//! the pointer speed and "Enhance pointer precision" the user set, so the mouse feels
//! the same on the other machine as on this one; Raw Input's counts would not.
//! Coordinates are physical because the process declares itself Per-Monitor-V2 DPI aware.

mod keyboard;
mod mouse;
mod watchdog;

#[cfg(windows)]
mod sys;

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use crate::event::InputEvent;

pub use keyboard::{AltGrFilter, FakeShiftFilter, key_event};
pub use mouse::{RawMotion, anchor, mouse_event};
pub use watchdog::{HookKind, Watchdog, WatchdogAction, WatchdogConfig};

#[cfg(windows)]
pub use sys::{WindowsCapture, ensure_per_monitor_v2};

/// Win32 constants used by the portable logic. `sys` checks at compile time
/// that they match those of the `windows` crate.
pub mod consts {
    pub const LLKHF_EXTENDED: u32 = 0x01;
    pub const LLKHF_INJECTED: u32 = 0x10;
    pub const LLKHF_UP: u32 = 0x80;
    pub const LLMHF_INJECTED: u32 = 0x01;

    pub const WM_MOUSEMOVE: u32 = 0x0200;
    pub const WM_LBUTTONDOWN: u32 = 0x0201;
    pub const WM_LBUTTONUP: u32 = 0x0202;
    pub const WM_RBUTTONDOWN: u32 = 0x0204;
    pub const WM_RBUTTONUP: u32 = 0x0205;
    pub const WM_MBUTTONDOWN: u32 = 0x0207;
    pub const WM_MBUTTONUP: u32 = 0x0208;
    pub const WM_MOUSEWHEEL: u32 = 0x020A;
    pub const WM_XBUTTONDOWN: u32 = 0x020B;
    pub const WM_XBUTTONUP: u32 = 0x020C;
    pub const WM_MOUSEHWHEEL: u32 = 0x020E;

    pub const XBUTTON1: u16 = 0x0001;
    pub const XBUTTON2: u16 = 0x0002;

    pub const MOUSE_MOVE_ABSOLUTE: u16 = 0x01;
    pub const MOUSE_VIRTUAL_DESKTOP: u16 = 0x02;

    pub const VK_CANCEL: u32 = 0x03;
    pub const VK_VOLUME_MUTE: u32 = 0xAD;
    pub const VK_VOLUME_DOWN: u32 = 0xAE;
    pub const VK_VOLUME_UP: u32 = 0xAF;
    pub const VK_MEDIA_NEXT_TRACK: u32 = 0xB0;
    pub const VK_MEDIA_PREV_TRACK: u32 = 0xB1;
    pub const VK_MEDIA_STOP: u32 = 0xB2;
    pub const VK_MEDIA_PLAY_PAUSE: u32 = 0xB3;
}

/// Copy of `KBDLLHOOKSTRUCT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardHook {
    pub vk: u32,
    pub scan_code: u32,
    pub flags: u32,
    /// System milliseconds; AltGr's fake Ctrl carries the same as AltGr.
    pub time: u32,
    pub extra_info: usize,
}

impl KeyboardHook {
    pub fn injected(&self) -> bool {
        self.flags & consts::LLKHF_INJECTED != 0
    }

    pub fn is_up(&self) -> bool {
        self.flags & consts::LLKHF_UP != 0
    }

    pub fn is_extended(&self) -> bool {
        self.flags & consts::LLKHF_EXTENDED != 0
    }
}

/// Copy of `MSLLHOOKSTRUCT` plus the message (`wParam`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseHook {
    pub message: u32,
    /// Cursor position in physical pixels of the virtual desktop.
    pub x: i32,
    pub y: i32,
    pub mouse_data: u32,
    pub flags: u32,
    pub time: u32,
    pub extra_info: usize,
}

impl MouseHook {
    pub fn injected(&self) -> bool {
        self.flags & consts::LLMHF_INJECTED != 0
    }

    /// High word of `mouseData`: wheel delta or X button number.
    pub fn high_word(&self) -> u16 {
        (self.mouse_data >> 16) as u16
    }
}

/// What matters of a `RAWMOUSE`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawMouse {
    /// `hDevice`; 0 = injected with `SendInput`.
    pub device: usize,
    /// `usFlags`.
    pub flags: u16,
    pub last_x: i32,
    pub last_y: i32,
    /// Width and height in pixels of the area covered by the 0–65535 range of
    /// absolute movements (virtual desktop or primary screen).
    pub span: (f64, f64),
    /// Input was being swallowed when it arrived: the hook measures the movement.
    pub grabbed: bool,
}

/// What the hook thread queues.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HookEvent {
    Keyboard {
        hook: KeyboardHook,
        swallowed: bool,
    },
    Mouse {
        hook: MouseHook,
        swallowed: bool,
        /// Where the cursor really was when a swallowed movement arrived
        /// ([`HookFilter::origin`]).
        origin: Option<(i32, i32)>,
    },
    Raw(RawMouse),
    /// Keyboard Raw Input; only useful to the watchdog.
    RawKeyboard {
        /// `hDevice`; 0 = injected with `SendInput`.
        device: usize,
    },
}

impl HookEvent {
    /// Which hook this event proves is still alive.
    pub fn hook_kind(&self) -> Option<HookKind> {
        match self {
            HookEvent::Keyboard { .. } => Some(HookKind::Keyboard),
            HookEvent::Mouse { .. } => Some(HookKind::Mouse),
            HookEvent::Raw(_) | HookEvent::RawKeyboard { .. } => None,
        }
    }

    /// Which kind of real input (from a device, not injected) this Raw Input
    /// event proves. A mouse without movement does not count: it is enough that the
    /// watchdog does not act more than needed.
    pub fn real_activity(&self) -> Option<HookKind> {
        match self {
            HookEvent::Raw(r) if r.device != 0 && (r.last_x != 0 || r.last_y != 0) => {
                Some(HookKind::Mouse)
            }
            HookEvent::RawKeyboard { device } if *device != 0 => Some(HookKind::Keyboard),
            _ => None,
        }
    }
}

/// What the hook does with an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Follow the chain: the local system receives it.
    Pass,
    /// Swallow it: the focus is on the other machine.
    Swallow,
}

/// Hook decision, with memory of what the local system saw pressed.
///
/// If a key or a button was pressed with local input (for example,
/// dragging to the edge) and is released already with input swallowed, the
/// release passes anyway: otherwise it would stay stuck on Windows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookFilter {
    /// Bit per virtual code (0–255).
    keys: [u64; 4],
    /// Bit per button: left, right, middle, X1, X2.
    buttons: u8,
    /// The cursor's real position: the last point of an event that passed, or where the
    /// capture put it. A swallowed event does not move it.
    cursor: Option<(i32, i32)>,
}

impl HookFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn keyboard(&mut self, grab: bool, k: &KeyboardHook) -> Verdict {
        if k.injected() {
            return Verdict::Pass;
        }
        let vk = (k.vk & 0xFF) as usize;
        let (word, bit) = (vk / 64, 1u64 << (vk % 64));
        let local = self.keys[word] & bit != 0;
        if k.is_up() {
            self.keys[word] &= !bit;
            if local || !grab {
                return Verdict::Pass;
            }
            return Verdict::Swallow;
        }
        if grab {
            Verdict::Swallow
        } else {
            self.keys[word] |= bit;
            Verdict::Pass
        }
    }

    /// Where the cursor is, measured from an event already decided: one that passes
    /// takes it to its point; a swallowed movement returns where it is, from which the hook
    /// measures the (accelerated) movement Windows would have made.
    pub fn origin(&mut self, verdict: Verdict, m: &MouseHook) -> Option<(i32, i32)> {
        match verdict {
            Verdict::Pass => {
                self.cursor = Some((m.x, m.y));
                None
            }
            Verdict::Swallow if m.message == consts::WM_MOUSEMOVE => self.cursor,
            Verdict::Swallow => None,
        }
    }

    /// The capture moved the cursor itself (`SetCursorPos`).
    pub fn set_cursor(&mut self, pos: (i32, i32)) {
        self.cursor = Some(pos);
    }

    pub fn mouse(&mut self, grab: bool, m: &MouseHook) -> Verdict {
        if m.injected() {
            return Verdict::Pass;
        }
        let Some((index, down)) = mouse::button_bit(m) else {
            return if grab {
                Verdict::Swallow
            } else {
                Verdict::Pass
            };
        };
        let bit = 1u8 << index;
        let local = self.buttons & bit != 0;
        if !down {
            self.buttons &= !bit;
            return if local || !grab {
                Verdict::Pass
            } else {
                Verdict::Swallow
            };
        }
        if grab {
            Verdict::Swallow
        } else {
            self.buttons |= bit;
            Verdict::Pass
        }
    }
}

/// The only thing each hook callback does: decide with the [`HookFilter`] and queue
/// for the process thread. No locks, I/O or waits (`send` on an unbounded channel
/// does not block): Windows removes the hook if it takes long. If the process thread
/// already ended, there is nobody to notify. Returns `true` if the event has to be swallowed.
pub fn hook_step(
    filter: &mut HookFilter,
    grab: bool,
    tx: &Sender<HookEvent>,
    decide: impl FnOnce(&mut HookFilter, bool) -> (Verdict, HookEvent),
) -> bool {
    let (verdict, event) = decide(filter, grab);
    let _ = tx.send(event);
    verdict != Verdict::Pass
}

/// Translates what the hook thread queues into Tandem events.
#[derive(Debug)]
pub struct Translator {
    shift: FakeShiftFilter,
    altgr: AltGrFilter,
    raw: RawMotion,
}

impl Translator {
    /// `altgr_window`: how long a left Ctrl is held waiting to see whether it is
    /// AltGr's fake one.
    pub fn new(altgr_window: Duration) -> Self {
        Self {
            shift: FakeShiftFilter::new(altgr_window),
            altgr: AltGrFilter::new(altgr_window),
            raw: RawMotion::new(),
        }
    }

    pub fn push(&mut self, event: HookEvent, now: Instant, out: &mut impl FnMut(InputEvent)) {
        let Self { shift, altgr, raw } = self;
        // Fake Shifts go first, then AltGr's Ctrl: each filter hands what it keeps to the next.
        let mut emit = |k: KeyboardHook| {
            altgr.push(k, now, &mut |k| emit_key(k, out));
        };
        match event {
            HookEvent::Keyboard { hook, .. } => {
                if !hook.injected() {
                    shift.push(hook, now, &mut emit);
                }
            }
            HookEvent::Mouse {
                hook,
                swallowed,
                origin,
            } => {
                if hook.injected() {
                    return;
                }
                if let Some(ev) = mouse_event(&hook, swallowed, origin) {
                    // The held keys happened earlier: they go first.
                    shift.flush_all(&mut emit);
                    altgr.flush_all(&mut |k| emit_key(k, out));
                    out(ev);
                }
            }
            HookEvent::Raw(r) => {
                // Always pushed, so an absolute device keeps its series; with input
                // swallowed the hook already measures the movement, with acceleration.
                if let Some((dx, dy)) = raw.push(&r)
                    && !r.grabbed
                {
                    shift.flush_all(&mut emit);
                    altgr.flush_all(&mut |k| emit_key(k, out));
                    out(InputEvent::MotionRelative { dx, dy });
                }
            }
            HookEvent::RawKeyboard { .. } => {}
        }
    }

    /// Releases the held keys whose window has passed.
    pub fn flush_expired(&mut self, now: Instant, out: &mut impl FnMut(InputEvent)) {
        let Self { shift, altgr, .. } = self;
        shift.flush_expired(now, &mut |k| altgr.push(k, now, &mut |k| emit_key(k, out)));
        altgr.flush_expired(now, &mut |k| emit_key(k, out));
    }

    pub fn flush_all(&mut self, out: &mut impl FnMut(InputEvent)) {
        let Self { shift, altgr, .. } = self;
        let now = Instant::now();
        shift.flush_all(&mut |k| altgr.push(k, now, &mut |k| emit_key(k, out)));
        altgr.flush_all(&mut |k| emit_key(k, out));
    }

    pub fn has_pending(&self) -> bool {
        self.shift.has_pending() || self.altgr.has_pending()
    }
}

fn emit_key(k: KeyboardHook, out: &mut impl FnMut(InputEvent)) {
    if let Some(ev) = key_event(&k) {
        out(ev);
    }
}

/// Errors on starting the capture.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("{op}: {message} (0x{code:08X})")]
    Os {
        op: &'static str,
        code: u32,
        message: String,
    },
    #[error("the process could not declare itself Per-Monitor-V2 DPI aware")]
    DpiAwareness,
    #[error("the capture thread could not be created: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("the hook thread ended before starting")]
    ThreadDied,
}

/// Capture parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureConfig {
    /// Window to recognize AltGr's fake Ctrl by time. The two events
    /// arrive back to back on the same thread, so very little is enough.
    pub altgr_window: Duration,
    /// How often the process thread wakes up without events (watchdog).
    pub tick: Duration,
    pub watchdog: WatchdogConfig,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            altgr_window: Duration::from_millis(10),
            tick: Duration::from_millis(100),
            watchdog: WatchdogConfig::default(),
        }
    }
}

/// What the process thread asks of the system; [`sys`] implements it with a
/// message to the hook thread, and tests with a double.
pub trait HookControl {
    /// Removes and installs the two hooks again.
    fn reinstall(&mut self);
}

/// Process thread loop: translates what arrives over `rx` and delivers it to `out`.
/// It ends when the hook thread drops its end of the channel.
pub fn run_pipeline(
    rx: &Receiver<HookEvent>,
    control: &mut impl HookControl,
    out: &mut impl FnMut(InputEvent),
    config: &CaptureConfig,
) {
    let mut translator = Translator::new(config.altgr_window);
    let mut watchdog = Watchdog::new(config.watchdog);
    loop {
        let wait = if translator.has_pending() {
            config.altgr_window
        } else {
            config.tick
        };
        match rx.recv_timeout(wait) {
            Ok(event) => {
                let now = Instant::now();
                if let Some(kind) = event.hook_kind() {
                    watchdog.hook_activity(kind, now);
                }
                if let Some(kind) = event.real_activity() {
                    watchdog.real_activity(kind, now);
                }
                translator.push(event, now, out);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                translator.flush_all(out);
                return;
            }
        }
        let now = Instant::now();
        translator.flush_expired(now, out);
        if watchdog.tick(now) == WatchdogAction::Reinstall {
            control.reinstall();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::consts::*;
    use super::*;
    use crate::event::{Button, HidUsage, KeyState, Scroll};
    use crate::windows::TANDEM_EXTRA_INFO;

    pub(super) fn key(scan: u32, flags: u32, time: u32) -> KeyboardHook {
        KeyboardHook {
            vk: 0,
            scan_code: scan,
            flags,
            time,
            extra_info: 0,
        }
    }

    fn mouse(message: u32, mouse_data: u32) -> MouseHook {
        MouseHook {
            message,
            x: 10,
            y: 20,
            mouse_data,
            flags: 0,
            time: 0,
            extra_info: 0,
        }
    }

    fn kb(hook: KeyboardHook) -> HookEvent {
        HookEvent::Keyboard {
            hook,
            swallowed: false,
        }
    }

    #[derive(Default)]
    struct FakeControl {
        reinstalls: u32,
        at: Vec<Instant>,
    }

    impl HookControl for FakeControl {
        fn reinstall(&mut self) {
            self.reinstalls += 1;
            self.at.push(Instant::now());
        }
    }

    /// T-093: the hook callback never waits, not even with the process thread stopped
    /// (the channel grows and nobody reads it). It is measured with a real clock: each step takes
    /// microseconds, far from the `LowLevelHooksTimeout` with which Windows removes the hook.
    #[test]
    fn the_hook_step_does_not_block_even_if_nobody_reads() {
        let (tx, _rx) = mpsc::channel();
        let mut filter = HookFilter::new();
        let mut times = Vec::with_capacity(20_000);
        for i in 0..20_000u32 {
            let hook = key(0x1E, if i % 2 == 0 { 0 } else { LLKHF_UP }, i);
            let t = Instant::now();
            hook_step(&mut filter, i % 3 == 0, &tx, |f, grab| {
                let verdict = f.keyboard(grab, &hook);
                (
                    verdict,
                    HookEvent::Keyboard {
                        hook,
                        swallowed: verdict == Verdict::Swallow,
                    },
                )
            });
            times.push(t.elapsed());
        }
        times.sort_unstable();
        let p99 = times[times.len() * 99 / 100];
        let max = times[times.len() - 1];
        assert!(p99 < Duration::from_micros(200), "p99 {p99:?}");
        // Generous for shared runners; Windows gives at least 200 ms.
        assert!(max < Duration::from_millis(50), "max {max:?}");
    }

    /// T-093: a hook that Windows silently removes (Raw Input still sees real input
    /// and the hook no longer does) is reinstalled in time, with a real clock; while the hook
    /// lived, never. The deadline is one to two `hook_timeout`s: during the first, a real
    /// input may still be that of the last event the hook saw (the two do not
    /// arrive in a fixed order); the second is the deadline itself.
    #[test]
    fn the_watchdog_reinstalls_a_lost_hook_in_time() {
        let (tx, rx) = mpsc::channel();
        let timeout = Duration::from_millis(100);
        let tick = Duration::from_millis(5);
        let feeder = std::thread::spawn(move || {
            let start = Instant::now();
            // 150 ms with the hook alive: every real key reaches both.
            while start.elapsed() < Duration::from_millis(150) {
                tx.send(kb(key(0x1E, 0, 0))).unwrap();
                tx.send(HookEvent::RawKeyboard { device: 3 }).unwrap();
                std::thread::sleep(Duration::from_millis(5));
            }
            // The hook is lost: only Raw Input.
            let lost = Instant::now();
            while lost.elapsed() < Duration::from_millis(400) {
                tx.send(HookEvent::RawKeyboard { device: 3 }).unwrap();
                std::thread::sleep(Duration::from_millis(5));
            }
            lost
        });
        let config = CaptureConfig {
            tick,
            watchdog: WatchdogConfig {
                hook_timeout: timeout,
            },
            ..CaptureConfig::default()
        };
        let mut control = FakeControl::default();
        run_pipeline(&rx, &mut control, &mut |_| {}, &config);
        let lost = feeder.join().unwrap();
        let first = *control.at.first().expect("it was not reinstalled");
        assert!(first >= lost, "reinstalled with the hook alive");
        let took = first - lost;
        // With margin for a shared runner's scheduler.
        assert!(took >= timeout - tick, "too early: {took:?}");
        assert!(
            took <= 2 * timeout + 4 * tick + Duration::from_millis(80),
            "late: {took:?}"
        );
    }

    fn run(events: Vec<HookEvent>) -> Vec<InputEvent> {
        let (tx, rx) = mpsc::channel();
        for e in events {
            tx.send(e).unwrap();
        }
        drop(tx);
        let mut out = Vec::new();
        run_pipeline(
            &rx,
            &mut FakeControl::default(),
            &mut |e| out.push(e),
            &CaptureConfig::default(),
        );
        out
    }

    #[test]
    fn the_filter_swallows_only_with_grab_and_never_what_is_injected() {
        let mut f = HookFilter::new();
        let a = KeyboardHook {
            vk: 0x41,
            ..key(0x1E, 0, 0)
        };
        assert_eq!(f.keyboard(false, &a), Verdict::Pass);
        assert_eq!(
            f.keyboard(
                false,
                &KeyboardHook {
                    flags: LLKHF_UP,
                    ..a
                }
            ),
            Verdict::Pass
        );
        assert_eq!(f.keyboard(true, &a), Verdict::Swallow);
        assert_eq!(
            f.keyboard(
                true,
                &KeyboardHook {
                    flags: LLKHF_UP,
                    ..a
                }
            ),
            Verdict::Swallow
        );
        let injected = KeyboardHook {
            flags: LLKHF_INJECTED,
            extra_info: TANDEM_EXTRA_INFO,
            ..a
        };
        assert_eq!(f.keyboard(true, &injected), Verdict::Pass);

        let mv = mouse(WM_MOUSEMOVE, 0);
        assert_eq!(f.mouse(false, &mv), Verdict::Pass);
        assert_eq!(f.mouse(true, &mv), Verdict::Swallow);
        let injected = MouseHook {
            flags: LLMHF_INJECTED,
            ..mv
        };
        assert_eq!(f.mouse(true, &injected), Verdict::Pass);
    }

    #[test]
    fn releasing_what_was_pressed_locally_passes_even_with_grab() {
        let mut f = HookFilter::new();
        let ctrl = KeyboardHook {
            vk: 0xA2,
            ..key(0x1D, 0, 0)
        };
        let up = KeyboardHook {
            flags: LLKHF_UP,
            ..ctrl
        };
        assert_eq!(f.keyboard(false, &ctrl), Verdict::Pass);
        // Repeat with the focus already remote: it is swallowed, but the key is still local.
        assert_eq!(f.keyboard(true, &ctrl), Verdict::Swallow);
        assert_eq!(f.keyboard(true, &up), Verdict::Pass);
        // Already released: the next full round is swallowed.
        assert_eq!(f.keyboard(true, &ctrl), Verdict::Swallow);
        assert_eq!(f.keyboard(true, &up), Verdict::Swallow);

        let down = mouse(WM_LBUTTONDOWN, 0);
        let release = mouse(WM_LBUTTONUP, 0);
        assert_eq!(f.mouse(false, &down), Verdict::Pass);
        assert_eq!(f.mouse(true, &release), Verdict::Pass);
        assert_eq!(f.mouse(true, &down), Verdict::Swallow);
        assert_eq!(f.mouse(true, &release), Verdict::Swallow);
        // X1 and X2 are told apart by the high word.
        let x1 = mouse(WM_XBUTTONDOWN, u32::from(XBUTTON1) << 16);
        let x2_up = mouse(WM_XBUTTONUP, u32::from(XBUTTON2) << 16);
        assert_eq!(f.mouse(false, &x1), Verdict::Pass);
        assert_eq!(f.mouse(true, &x2_up), Verdict::Swallow);
    }

    #[test]
    fn pipeline_translates_in_order_and_discards_injected() {
        let a = key(0x1E, 0, 1);
        let out = run(vec![
            kb(a),
            kb(KeyboardHook {
                flags: LLKHF_INJECTED,
                ..key(0x30, 0, 2)
            }),
            HookEvent::Mouse {
                hook: mouse(WM_LBUTTONDOWN, 0),
                swallowed: true,
                origin: None,
            },
            HookEvent::Raw(RawMouse {
                device: 7,
                flags: 0,
                last_x: 3,
                last_y: -2,
                span: (0.0, 0.0),
                grabbed: false,
            }),
            HookEvent::Raw(RawMouse {
                device: 0,
                flags: 0,
                last_x: 50,
                last_y: 50,
                span: (0.0, 0.0),
                grabbed: false,
            }),
            HookEvent::RawKeyboard { device: 7 },
            HookEvent::Mouse {
                hook: mouse(WM_MOUSEWHEEL, (120u32) << 16),
                swallowed: true,
                origin: None,
            },
            kb(KeyboardHook {
                flags: LLKHF_UP,
                ..a
            }),
        ]);
        assert_eq!(
            out,
            vec![
                InputEvent::Key {
                    usage: HidUsage::key(0x04),
                    state: KeyState::Pressed
                },
                InputEvent::Button {
                    button: Button::Left,
                    state: KeyState::Pressed
                },
                InputEvent::MotionRelative { dx: 3.0, dy: -2.0 },
                InputEvent::Scroll(Scroll::Wheel {
                    dx_v120: 0,
                    dy_v120: 120
                }),
                InputEvent::Key {
                    usage: HidUsage::key(0x04),
                    state: KeyState::Released
                },
            ]
        );
    }

    #[test]
    fn pipeline_passes_the_extended_right_shift() {
        // T-359: the hook reports the real right Shift with LLKHF_EXTENDED; it reaches the
        // other machine as right Shift instead of being dropped as a fake one.
        let mut t = Translator::new(Duration::from_millis(10));
        let mut out = Vec::new();
        let now = Instant::now();
        for (scan, flags, time) in [
            (0x36, LLKHF_EXTENDED, 1),
            (0x1E, 0, 80),
            (0x36, LLKHF_EXTENDED | LLKHF_UP, 150),
        ] {
            t.push(
                HookEvent::Keyboard {
                    hook: key(scan, flags, time),
                    swallowed: true,
                },
                now,
                &mut |e| out.push(e),
            );
        }
        t.flush_all(&mut |e| out.push(e));
        let rshift = HidUsage::key(0xE5);
        assert_eq!(
            out,
            vec![
                InputEvent::Key {
                    usage: rshift,
                    state: KeyState::Pressed
                },
                InputEvent::Key {
                    usage: HidUsage::key(0x04),
                    state: KeyState::Pressed
                },
                InputEvent::Key {
                    usage: rshift,
                    state: KeyState::Released
                },
            ]
        );
    }

    #[test]
    fn pipeline_removes_altgrs_fake_ctrl() {
        let ralt = key(0x38, LLKHF_EXTENDED, 50);
        let lctrl = key(0x1D, 0, 50);
        let out = run(vec![
            kb(lctrl),
            kb(ralt),
            kb(KeyboardHook {
                flags: LLKHF_UP,
                ..lctrl
            }),
            kb(KeyboardHook {
                flags: LLKHF_EXTENDED | LLKHF_UP,
                ..ralt
            }),
        ]);
        let alt_right = HidUsage::key(0xE6);
        assert_eq!(
            out,
            vec![
                InputEvent::Key {
                    usage: alt_right,
                    state: KeyState::Pressed
                },
                InputEvent::Key {
                    usage: alt_right,
                    state: KeyState::Released
                },
            ]
        );
    }

    #[test]
    fn pipeline_releases_the_held_ctrl_on_closing_and_before_other_events() {
        let lctrl = key(0x1D, 0, 5);
        let ctrl = HidUsage::key(0xE0);
        // Only Ctrl and the channel is cut: it is not lost.
        assert_eq!(
            run(vec![kb(lctrl)]),
            vec![InputEvent::Key {
                usage: ctrl,
                state: KeyState::Pressed
            }]
        );
        // Ctrl + click: the Ctrl goes before the click.
        let out = run(vec![
            kb(lctrl),
            HookEvent::Mouse {
                hook: mouse(WM_LBUTTONDOWN, 0),
                swallowed: false,
                origin: None,
            },
        ]);
        assert_eq!(
            out,
            vec![
                InputEvent::Key {
                    usage: ctrl,
                    state: KeyState::Pressed
                },
                InputEvent::Button {
                    button: Button::Left,
                    state: KeyState::Pressed
                },
            ]
        );
    }

    #[test]
    fn the_filter_knows_where_the_cursor_really_is() {
        let mut f = HookFilter::new();
        let at = |x, y| MouseHook {
            x,
            y,
            ..mouse(WM_MOUSEMOVE, 0)
        };
        assert_eq!(f.origin(Verdict::Swallow, &at(5, 5)), None, "not known yet");
        assert_eq!(f.origin(Verdict::Pass, &at(100, 200)), None);
        // Swallowed: the cursor stays where the last one that passed left it.
        assert_eq!(f.origin(Verdict::Swallow, &at(110, 190)), Some((100, 200)));
        assert_eq!(f.origin(Verdict::Swallow, &at(130, 170)), Some((100, 200)));
        // A swallowed click does not measure anything.
        assert_eq!(f.origin(Verdict::Swallow, &mouse(WM_LBUTTONDOWN, 0)), None);
        f.set_cursor((1720, 720));
        assert_eq!(
            f.origin(Verdict::Swallow, &at(1725, 720)),
            Some((1720, 720))
        );
    }

    /// The bug: with the focus on the Mac the movement went in Raw Input counts, without
    /// the PC's pointer speed or acceleration, so the mouse felt different over there.
    #[test]
    fn with_input_swallowed_the_movement_is_the_accelerated_one_of_the_hook() {
        let moved = |x, y, swallowed, origin| HookEvent::Mouse {
            hook: MouseHook {
                x,
                y,
                ..mouse(WM_MOUSEMOVE, 0)
            },
            swallowed,
            origin,
        };
        let raw = |grabbed| {
            HookEvent::Raw(RawMouse {
                device: 7,
                flags: 0,
                last_x: 2,
                last_y: 0,
                span: (0.0, 0.0),
                grabbed,
            })
        };
        let out = run(vec![
            // Local: Raw's delta and the hook's position, as before.
            raw(false),
            moved(102, 50, false, None),
            // Swallowed: Raw does not count; the hook's point, accelerated by
            // Windows (2 counts became 9 pixels), minus where the cursor stayed.
            raw(true),
            moved(111, 50, true, Some((102, 50))),
        ]);
        assert_eq!(
            out,
            vec![
                InputEvent::MotionRelative { dx: 2.0, dy: 0.0 },
                InputEvent::MotionAbsolute { x: 102.0, y: 50.0 },
                InputEvent::MotionRelative { dx: 9.0, dy: 0.0 },
            ]
        );
    }

    fn raw_mouse(device: usize, dx: i32) -> HookEvent {
        HookEvent::Raw(RawMouse {
            device,
            flags: 0,
            last_x: dx,
            last_y: 0,
            span: (0.0, 0.0),
            grabbed: false,
        })
    }

    #[test]
    fn only_real_raw_input_counts_as_real_input() {
        assert_eq!(raw_mouse(1, 1).hook_kind(), None);
        assert_eq!(raw_mouse(1, 1).real_activity(), Some(HookKind::Mouse));
        // Injected or without movement: it does not count.
        assert_eq!(raw_mouse(0, 1).real_activity(), None);
        assert_eq!(raw_mouse(1, 0).real_activity(), None);
        let kb_raw = HookEvent::RawKeyboard { device: 4 };
        assert_eq!(kb_raw.hook_kind(), None);
        assert_eq!(kb_raw.real_activity(), Some(HookKind::Keyboard));
        assert_eq!(HookEvent::RawKeyboard { device: 0 }.real_activity(), None);
        assert_eq!(kb(key(0x1E, 0, 0)).hook_kind(), Some(HookKind::Keyboard));
        assert_eq!(kb(key(0x1E, 0, 0)).real_activity(), None);
    }

    /// Runs the pipeline with short deadlines while another thread sends `step()` every
    /// 5 ms for about 300 ms. Returns how many times it reinstalled.
    fn reinstalls_with(step: fn() -> Vec<HookEvent>) -> u32 {
        let (tx, rx) = mpsc::channel();
        let feeder = std::thread::spawn(move || {
            for _ in 0..60 {
                for e in step() {
                    tx.send(e).unwrap();
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let config = CaptureConfig {
            tick: Duration::from_millis(5),
            watchdog: WatchdogConfig {
                hook_timeout: Duration::from_millis(100),
            },
            ..CaptureConfig::default()
        };
        let mut control = FakeControl::default();
        run_pipeline(&rx, &mut control, &mut |_| {}, &config);
        feeder.join().unwrap();
        control.reinstalls
    }

    #[test]
    fn pipeline_reinstalls_if_the_real_keyboard_does_not_reach_the_hook() {
        assert!(reinstalls_with(|| vec![HookEvent::RawKeyboard { device: 3 }]) >= 1);
        assert!(reinstalls_with(|| vec![raw_mouse(3, 2)]) >= 1);
    }

    #[test]
    fn pipeline_does_not_reinstall_with_live_hooks_or_injected_input() {
        // Injected: Raw Input without hDevice and nothing in the hooks.
        assert_eq!(
            reinstalls_with(|| vec![HookEvent::RawKeyboard { device: 0 }, raw_mouse(0, 2)]),
            0
        );
        // Real and seen by the hooks.
        assert_eq!(
            reinstalls_with(|| vec![
                kb(key(0x1E, 0, 0)),
                HookEvent::RawKeyboard { device: 3 },
                raw_mouse(3, 2),
                HookEvent::Mouse {
                    hook: mouse(WM_MOUSEMOVE, 0),
                    swallowed: false,
                    origin: None,
                },
            ]),
            0
        );
    }

    #[test]
    fn idle_pipeline_touches_nothing() {
        assert_eq!(reinstalls_with(Vec::new), 0);
    }
}
