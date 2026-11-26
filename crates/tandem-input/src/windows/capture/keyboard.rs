//! Keyboard: hook scancode → HID key, and AltGr's fake Ctrl.

use std::time::{Duration, Instant};

use super::KeyboardHook;
use super::consts::*;
use crate::event::keymap::{self, WinScancode};
use crate::event::{InputEvent, KeyState};

/// Scancode with which Windows marks the left Ctrl it invents when pressing
/// AltGr on layouts that have it (like the Spanish one).
const FAKE_CTRL_SCAN: u32 = 0x21D;
const SCAN_CONTROL: u32 = 0x1D;
const SCAN_ALT: u32 = 0x38;
/// Right Shift. The low-level hook reports the real key with `LLKHF_EXTENDED` (T-359), the same
/// flags as the fake one Windows adds around the navigation keys, so the flag does not tell
/// them apart: [`FakeShiftFilter`] does, by time.
const SCAN_RSHIFT: u32 = 0x36;

/// Keys that can arrive without a scancode (some media keyboards) or with one
/// that the table does not have: they are resolved by the virtual code.
fn scancode_from_vk(vk: u32) -> Option<WinScancode> {
    let code = match vk {
        // Ctrl+Pause arrives as VK_CANCEL with E0 46: it is the Pause key.
        VK_CANCEL => 0x0045,
        VK_VOLUME_MUTE => 0xE020,
        VK_VOLUME_DOWN => 0xE02E,
        VK_VOLUME_UP => 0xE030,
        VK_MEDIA_NEXT_TRACK => 0xE019,
        VK_MEDIA_PREV_TRACK => 0xE010,
        VK_MEDIA_STOP => 0xE024,
        VK_MEDIA_PLAY_PAUSE => 0xE022,
        _ => return None,
    };
    Some(WinScancode(code))
}

/// The physical key of a hook event. `None` for what is not a real key: the fake left Shift
/// (E0 2A) that Windows adds around the arrows with NumLock, or codes that are not in the
/// table. Right Shift is right Shift with or without the extended flag; its fake one is
/// removed earlier by [`FakeShiftFilter`].
pub fn key_event(k: &KeyboardHook) -> Option<InputEvent> {
    let extended = k.is_extended() && k.scan_code != SCAN_RSHIFT;
    let by_scan = u8::try_from(k.scan_code)
        .ok()
        .filter(|code| *code != 0)
        .and_then(|code| keymap::from_win(WinScancode::from_parts(code, extended)));
    let usage = by_scan.or_else(|| keymap::from_win(scancode_from_vk(k.vk)?))?;
    let state = if k.is_up() {
        KeyState::Released
    } else {
        KeyState::Pressed
    };
    Some(InputEvent::Key { usage, state })
}

fn is_left_ctrl(k: &KeyboardHook) -> bool {
    k.scan_code & 0xFF == SCAN_CONTROL && !k.is_extended()
}

fn is_right_alt(k: &KeyboardHook) -> bool {
    k.scan_code == SCAN_ALT && k.is_extended()
}

/// Removes the left Ctrl that Windows sends along with AltGr.
///
/// It is recognized by scancode `0x21D` or, if it does not carry it, because it arrives just before
/// right Alt with the same `time` and the same direction. To know that, each left
/// Ctrl is held until the next event or until the window passes.
#[derive(Debug)]
pub struct AltGrFilter {
    window: Duration,
    pending: Option<(KeyboardHook, Instant)>,
}

impl AltGrFilter {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            pending: None,
        }
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn push(&mut self, k: KeyboardHook, now: Instant, out: &mut impl FnMut(KeyboardHook)) {
        if let Some((held, _)) = self.pending.take() {
            if is_right_alt(&k) && k.time == held.time && k.is_up() == held.is_up() {
                out(k);
                return;
            }
            out(held);
        }
        if k.scan_code == FAKE_CTRL_SCAN {
            return;
        }
        if is_left_ctrl(&k) {
            self.pending = Some((k, now));
            return;
        }
        out(k);
    }

    pub fn flush_expired(&mut self, now: Instant, out: &mut impl FnMut(KeyboardHook)) {
        if let Some((_, since)) = self.pending
            && now.saturating_duration_since(since) >= self.window
        {
            self.flush_all(out);
        }
    }

    pub fn flush_all(&mut self, out: &mut impl FnMut(KeyboardHook)) {
        if let Some((held, _)) = self.pending.take() {
            out(held);
        }
    }
}

/// The extended navigation keys Windows wraps in fake Shifts with NumLock on: Insert, Delete,
/// Home, End, Page Up, Page Down and the arrows (E0 47..E0 53), and the keypad's `/` (E0 35).
fn is_navigation(k: &KeyboardHook) -> bool {
    k.is_extended() && matches!(k.scan_code, 0x47..=0x49 | 0x4B | 0x4D | 0x4F..=0x53 | 0x35)
}

fn is_right_shift(k: &KeyboardHook) -> bool {
    k.scan_code == SCAN_RSHIFT && k.is_extended()
}

/// Removes the fake right Shift (E0 B6 before, E0 36 after) that Windows puts around a
/// navigation key while the real right Shift is held with NumLock on. The real right Shift
/// carries the same flags, so it is told apart by time: the fake one comes within `window` of
/// the navigation key's `time`; a person does not press both that close. A right Shift is held
/// until the next event or until the window passes, like AltGr's Ctrl.
#[derive(Debug)]
pub struct FakeShiftFilter {
    window: Duration,
    pending: Option<(KeyboardHook, Instant)>,
    /// `time` of the last navigation key.
    last_navigation: Option<u32>,
}

impl FakeShiftFilter {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            pending: None,
            last_navigation: None,
        }
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Hook times are milliseconds that wrap around.
    fn close(&self, a: u32, b: u32) -> bool {
        let gap = a.wrapping_sub(b).min(b.wrapping_sub(a));
        u128::from(gap) <= self.window.as_millis()
    }

    pub fn push(&mut self, k: KeyboardHook, now: Instant, out: &mut impl FnMut(KeyboardHook)) {
        if let Some((held, _)) = self.pending.take()
            && !(is_navigation(&k) && self.close(k.time, held.time))
        {
            out(held);
        }
        if is_right_shift(&k) {
            if !self.last_navigation.is_some_and(|t| self.close(k.time, t)) {
                self.pending = Some((k, now));
            }
            return;
        }
        if is_navigation(&k) {
            self.last_navigation = Some(k.time);
        }
        out(k);
    }

    pub fn flush_expired(&mut self, now: Instant, out: &mut impl FnMut(KeyboardHook)) {
        if let Some((_, since)) = self.pending
            && now.saturating_duration_since(since) >= self.window
        {
            self.flush_all(out);
        }
    }

    pub fn flush_all(&mut self, out: &mut impl FnMut(KeyboardHook)) {
        if let Some((held, _)) = self.pending.take() {
            out(held);
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::super::tests::key;
    use super::*;
    use crate::event::HidUsage;

    fn usage(k: KeyboardHook) -> Option<HidUsage> {
        match key_event(&k)? {
            InputEvent::Key { usage, .. } => Some(usage),
            _ => None,
        }
    }

    #[test]
    fn hook_scancodes_to_hid() {
        // A, Enter, numeric keypad Enter (E0), arrows, Win.
        assert_eq!(usage(key(0x1E, 0, 0)), Some(HidUsage::key(0x04)));
        assert_eq!(usage(key(0x1C, 0, 0)), Some(HidUsage::key(0x28)));
        assert_eq!(
            usage(key(0x1C, LLKHF_EXTENDED, 0)),
            Some(HidUsage::key(0x58))
        );
        assert_eq!(
            usage(key(0x48, LLKHF_EXTENDED, 0)),
            Some(HidUsage::key(0x52))
        );
        assert_eq!(usage(key(0x48, 0, 0)), Some(HidUsage::key(0x60)));
        assert_eq!(
            usage(key(0x5B, LLKHF_EXTENDED, 0)),
            Some(HidUsage::key(0xE3))
        );
        // AltGr is right Alt; right Ctrl carries E0.
        assert_eq!(
            usage(key(0x38, LLKHF_EXTENDED, 0)),
            Some(HidUsage::key(0xE6))
        );
        assert_eq!(
            usage(key(0x1D, LLKHF_EXTENDED, 0)),
            Some(HidUsage::key(0xE4))
        );
        // The ISO `<` key.
        assert_eq!(usage(key(0x56, 0, 0)), Some(HidUsage::key(0x64)));
    }

    #[test]
    fn numlock_and_pause_share_a_scancode() {
        // The hook delivers 0x45 for both; NumLock with the extended mark.
        assert_eq!(
            usage(key(0x45, LLKHF_EXTENDED, 0)),
            Some(HidUsage::key(0x53))
        );
        assert_eq!(usage(key(0x45, 0, 0)), Some(HidUsage::key(0x48)));
        // Ctrl+Pause (Intl).
        let brk = KeyboardHook {
            vk: VK_CANCEL,
            ..key(0x46, LLKHF_EXTENDED, 0)
        };
        assert_eq!(usage(brk), Some(HidUsage::key(0x48)));
    }

    #[test]
    fn media_without_scancode_by_virtual_code() {
        let cases = [
            (VK_VOLUME_MUTE, HidUsage::key(0x7F)),
            (VK_VOLUME_UP, HidUsage::key(0x80)),
            (VK_VOLUME_DOWN, HidUsage::key(0x81)),
            (VK_MEDIA_PLAY_PAUSE, HidUsage::consumer(0xCD)),
            (VK_MEDIA_NEXT_TRACK, HidUsage::consumer(0xB5)),
            (VK_MEDIA_PREV_TRACK, HidUsage::consumer(0xB6)),
            (VK_MEDIA_STOP, HidUsage::consumer(0xB7)),
        ];
        for (vk, expected) in cases {
            let k = KeyboardHook { vk, ..key(0, 0, 0) };
            assert_eq!(usage(k), Some(expected), "vk {vk:#x}");
        }
        // The same volume with its real scancode gives the same.
        assert_eq!(
            usage(key(0x20, LLKHF_EXTENDED, 0)),
            Some(HidUsage::key(0x7F))
        );
    }

    #[test]
    fn fake_shift_and_unknown_codes_are_discarded() {
        assert_eq!(key_event(&key(0x2A, LLKHF_EXTENDED, 0)), None);
        assert_eq!(key_event(&key(0, 0, 0)), None);
        assert_eq!(key_event(&key(0x1_0000, 0, 0)), None);
    }

    #[test]
    fn right_shift_with_the_extended_flag_is_right_shift() {
        // The low-level hook reports the real right Shift with LLKHF_EXTENDED (T-359).
        assert_eq!(
            usage(key(0x36, LLKHF_EXTENDED, 0)),
            Some(HidUsage::key(0xE5))
        );
        assert_eq!(usage(key(0x36, 0, 0)), Some(HidUsage::key(0xE5)));
    }

    fn shifts(events: &[KeyboardHook]) -> Vec<KeyboardHook> {
        let now = Instant::now();
        let mut f = FakeShiftFilter::new(Duration::from_millis(10));
        let mut out = Vec::new();
        for k in events {
            f.push(*k, now, &mut |k| out.push(k));
        }
        f.flush_all(&mut |k| out.push(k));
        out
    }

    #[test]
    fn a_lone_right_shift_passes() {
        let down = key(SCAN_RSHIFT, LLKHF_EXTENDED, 100);
        let a = key(0x1E, 0, 160);
        let up = key(SCAN_RSHIFT, LLKHF_EXTENDED | LLKHF_UP, 220);
        assert_eq!(shifts(&[down, a, up]), vec![down, a, up]);
    }

    #[test]
    fn the_fake_right_shifts_around_an_arrow_are_dropped() {
        // Right Shift held, NumLock on, up arrow: Windows wraps the arrow in a fake E0 B6 /
        // E0 36 pair with the arrow's time. The real Shift stays held at the other end.
        let real = key(SCAN_RSHIFT, LLKHF_EXTENDED, 100);
        let fake_up = key(SCAN_RSHIFT, LLKHF_EXTENDED | LLKHF_UP, 300);
        let arrow = key(0x48, LLKHF_EXTENDED, 300);
        let arrow_up = key(0x48, LLKHF_EXTENDED | LLKHF_UP, 380);
        let fake_down = key(SCAN_RSHIFT, LLKHF_EXTENDED, 380);
        let real_up = key(SCAN_RSHIFT, LLKHF_EXTENDED | LLKHF_UP, 500);
        assert_eq!(
            shifts(&[real, fake_up, arrow, arrow_up, fake_down, real_up]),
            vec![real, arrow, arrow_up, real_up]
        );
    }

    #[test]
    fn a_held_right_shift_goes_out_when_the_window_expires() {
        let t0 = Instant::now();
        let mut f = FakeShiftFilter::new(Duration::from_millis(10));
        let mut out = Vec::new();
        let down = key(SCAN_RSHIFT, LLKHF_EXTENDED, 1);
        f.push(down, t0, &mut |k| out.push(k));
        assert!(out.is_empty() && f.has_pending());
        f.flush_expired(t0 + Duration::from_millis(9), &mut |k| out.push(k));
        assert!(out.is_empty());
        f.flush_expired(t0 + Duration::from_millis(10), &mut |k| out.push(k));
        assert_eq!(out, vec![down]);
    }

    #[test]
    fn the_state_comes_from_llkhf_up() {
        assert_eq!(
            key_event(&key(0x1E, LLKHF_UP, 0)),
            Some(InputEvent::Key {
                usage: HidUsage::key(0x04),
                state: KeyState::Released
            })
        );
    }

    fn filter(events: &[KeyboardHook]) -> Vec<KeyboardHook> {
        let now = Instant::now();
        let mut f = AltGrFilter::new(Duration::from_millis(10));
        let mut out = Vec::new();
        for k in events {
            f.push(*k, now, &mut |k| out.push(k));
        }
        f.flush_all(&mut |k| out.push(k));
        out
    }

    #[test]
    fn ctrl_with_0x21d_is_always_discarded() {
        let fake = key(FAKE_CTRL_SCAN, 0, 1);
        let ralt = key(SCAN_ALT, LLKHF_EXTENDED, 2);
        let fake_up = key(FAKE_CTRL_SCAN, LLKHF_UP, 3);
        assert_eq!(filter(&[fake, ralt, fake_up]), vec![ralt]);
    }

    #[test]
    fn held_real_ctrl_survives_altgr() {
        // Real Ctrl (t=1), AltGr with its fake Ctrl (t=9), release AltGr, release Ctrl.
        let ctrl = key(SCAN_CONTROL, 0, 1);
        let fake = key(SCAN_CONTROL, 0, 9);
        let ralt = key(SCAN_ALT, LLKHF_EXTENDED, 9);
        let fake_up = key(SCAN_CONTROL, LLKHF_UP, 12);
        let ralt_up = key(SCAN_ALT, LLKHF_EXTENDED | LLKHF_UP, 12);
        let ctrl_up = key(SCAN_CONTROL, LLKHF_UP, 20);
        assert_eq!(
            filter(&[ctrl, fake, ralt, fake_up, ralt_up, ctrl_up]),
            vec![ctrl, ralt, ralt_up, ctrl_up]
        );
    }

    #[test]
    fn ctrl_and_right_alt_at_different_times_are_real() {
        let ctrl = key(SCAN_CONTROL, 0, 1);
        let ralt = key(SCAN_ALT, LLKHF_EXTENDED, 2);
        assert_eq!(filter(&[ctrl, ralt]), vec![ctrl, ralt]);
        // Same time but opposite direction: it is not AltGr's pair either.
        let ctrl_up = key(SCAN_CONTROL, LLKHF_UP, 3);
        let ralt_down = key(SCAN_ALT, LLKHF_EXTENDED, 3);
        assert_eq!(filter(&[ctrl_up, ralt_down]), vec![ctrl_up, ralt_down]);
        // Right Ctrl (E0 1D) is not held.
        let rctrl = key(SCAN_CONTROL, LLKHF_EXTENDED, 4);
        assert_eq!(filter(&[rctrl]), vec![rctrl]);
    }

    #[test]
    fn the_held_ctrl_goes_out_when_the_window_expires() {
        let t0 = Instant::now();
        let mut f = AltGrFilter::new(Duration::from_millis(10));
        let mut out = Vec::new();
        let ctrl = key(SCAN_CONTROL, 0, 1);
        f.push(ctrl, t0, &mut |k| out.push(k));
        assert!(out.is_empty() && f.has_pending());
        f.flush_expired(t0 + Duration::from_millis(9), &mut |k| out.push(k));
        assert!(out.is_empty());
        f.flush_expired(t0 + Duration::from_millis(10), &mut |k| out.push(k));
        assert_eq!(out, vec![ctrl]);
        assert!(!f.has_pending());
    }

    fn arb_key() -> impl Strategy<Value = KeyboardHook> {
        (
            prop_oneof![
                Just(SCAN_CONTROL),
                Just(SCAN_ALT),
                Just(FAKE_CTRL_SCAN),
                0x01u32..0x60
            ],
            prop_oneof![
                Just(0),
                Just(LLKHF_EXTENDED),
                Just(LLKHF_UP),
                Just(LLKHF_EXTENDED | LLKHF_UP)
            ],
            0u32..4,
        )
            .prop_map(|(scan, flags, time)| key(scan, flags, time))
    }

    fn arb_typing() -> impl Strategy<Value = Vec<KeyboardHook>> {
        // Letters and the right Shift, with times that only go forward.
        prop::collection::vec(
            (
                prop_oneof![Just(SCAN_RSHIFT), 0x10u32..0x33],
                prop_oneof![
                    Just(LLKHF_EXTENDED),
                    Just(LLKHF_EXTENDED | LLKHF_UP),
                    Just(0),
                    Just(LLKHF_UP)
                ],
                0u32..200,
            ),
            0..40,
        )
        .prop_map(|v| {
            let mut t = 0u32;
            v.into_iter()
                .map(|(scan, flags, gap)| {
                    t = t.wrapping_add(gap);
                    key(scan, flags, t)
                })
                .collect()
        })
    }

    proptest! {
        #[test]
        fn without_navigation_keys_nothing_is_lost_or_reordered(events in arb_typing()) {
            prop_assert_eq!(shifts(&events), events);
        }
    }

    proptest! {
        /// The filter only removes left Ctrls (and everything `0x21D`): the rest comes out
        /// as is and in the same order.
        #[test]
        fn altgr_only_removes_the_left_ctrl(events in prop::collection::vec(arb_key(), 0..40)) {
            let out = filter(&events);
            let others = |v: &[KeyboardHook]| {
                v.iter()
                    .copied()
                    .filter(|k| !is_left_ctrl(k) && k.scan_code != FAKE_CTRL_SCAN)
                    .collect::<Vec<_>>()
            };
            prop_assert_eq!(others(&out), others(&events));
            // What comes out is a subsequence of what went in.
            let mut it = events.iter();
            for k in &out {
                prop_assert!(it.any(|e| e == k));
            }
            prop_assert!(out.iter().all(|k| k.scan_code != FAKE_CTRL_SCAN));
        }

        /// Without a right Alt nearby, no real Ctrl is lost.
        #[test]
        fn without_altgr_no_ctrl_is_lost(events in prop::collection::vec(arb_key(), 0..40)) {
            let events: Vec<_> = events
                .into_iter()
                .filter(|k| !is_right_alt(k) && k.scan_code != FAKE_CTRL_SCAN)
                .collect();
            prop_assert_eq!(filter(&events), events);
        }
    }
}
