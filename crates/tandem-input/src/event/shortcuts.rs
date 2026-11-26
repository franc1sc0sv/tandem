//! Translation of the source system's shortcuts into the target's equivalent.
//!
//! The modifier remap ([`super::remap`]) only swaps keys: Ctrl+Left on the PC arrives as
//! Cmd+Left on the Mac (line start) when the user meant "word to the left". This module
//! rewrites whole chords by intent (Ctrl+Left → Option+Left, Home → Cmd+Left, Alt+Tab →
//! Cmd+Tab, ...). It is pure and portable: events in, events out; the server applies it
//! where it applied the remap, so the client still injects what it receives as is.
//!
//! # Model
//!
//! The translator follows the source's physical state (`src_mods`) and decides, after every
//! event, which modifiers the target must have down (`desired`): the remap of the source's
//! modifiers, minus the classes a translation consumes, plus the ones it adds. It emits only
//! the difference against what it already emitted (`out_mods`). A translation lives exactly
//! as long as its key is held (`active`), so releasing the key restores the plain remap, and
//! releasing everything at the source leaves nothing pressed at the target.
//!
//! Rules are written in the source's terms (before the remap) and match on the *classes*
//! Ctrl/Alt/Meta held (either side) plus an optional Shift requirement. The right Alt
//! (AltGr / right Option) types characters, so while it is held nothing is translated.
//!
//! Not translated, by design: Cmd+Backspace (Mac→Win would be "Shift+Home, Backspace", a
//! two-step sequence this model does not have) and F2.

use super::{
    HidUsage, InputEvent, KeyState, ModifierKey, ModifierRemap, Modifiers, PAGE_KEYBOARD,
    RemapPolicy, remap::Direction,
};

/// Set of modifier classes (side-agnostic): Ctrl, Alt (left only), Meta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Classes(u8);

impl Classes {
    const NONE: Classes = Classes(0);
    const CTRL: Classes = Classes(1);
    const ALT: Classes = Classes(2);
    const META: Classes = Classes(4);

    const fn or(self, other: Classes) -> Classes {
        Classes(self.0 | other.0)
    }

    const fn has(self, other: Classes) -> bool {
        self.0 & other.0 != 0
    }

    /// The class of a modifier key; empty for Shift and for the right Alt.
    fn of(key: ModifierKey) -> Classes {
        match key {
            ModifierKey::ControlLeft | ModifierKey::ControlRight => Classes::CTRL,
            ModifierKey::AltLeft => Classes::ALT,
            ModifierKey::MetaLeft | ModifierKey::MetaRight => Classes::META,
            ModifierKey::ShiftLeft | ModifierKey::ShiftRight | ModifierKey::AltRight => {
                Classes::NONE
            }
        }
    }

    /// The keys that represent the class at the target (left side).
    fn keys(self) -> impl Iterator<Item = ModifierKey> {
        [
            (Classes::CTRL, ModifierKey::ControlLeft),
            (Classes::ALT, ModifierKey::AltLeft),
            (Classes::META, ModifierKey::MetaLeft),
        ]
        .into_iter()
        .filter(move |(c, _)| self.has(*c))
        .map(|(_, k)| k)
    }
}

/// One translation: `req` classes (+ `shift`) and `key` at the source become `out_add`
/// classes (+ `out_shift`) and `out_key` at the target.
#[derive(Debug, Clone, Copy)]
struct Rule {
    req: Classes,
    /// `Some(b)`: Shift must be (not) held at the source. `None`: either, and it is kept.
    shift: Option<bool>,
    key: u16,
    /// Classes pressed at the target, in the target's terms.
    out_add: Classes,
    /// `Some(b)`: force Shift (not) held at the target. `None`: the source's Shift is kept.
    out_shift: Option<bool>,
    /// Keyboard-page usage pressed at the target; `NO_KEY` if the chord is only modifiers
    /// (Cmd+Space becomes a Win press, held as long as Space is).
    out_key: u16,
    /// While the consumed class stays held at the source, the added one stays held at the
    /// target (the Alt+Tab switcher closes when Alt goes up).
    hold: bool,
}

const fn rule(req: Classes, shift: Option<bool>, key: u16, out_add: Classes, out_key: u16) -> Rule {
    Rule {
        req,
        shift,
        key,
        out_add,
        out_shift: None,
        out_key,
        hold: false,
    }
}

impl Rule {
    const fn out_shift(mut self, shift: bool) -> Rule {
        self.out_shift = Some(shift);
        self
    }

    const fn hold(mut self) -> Rule {
        self.hold = true;
        self
    }
}

const NO: Classes = Classes::NONE;
const C: Classes = Classes::CTRL;
const A: Classes = Classes::ALT;
const M: Classes = Classes::META;

// HID usages (keyboard page).
const KEY_Q: u16 = 0x14;
const KEY_S: u16 = 0x16;
const KEY_D: u16 = 0x07;
const KEY_E: u16 = 0x08;
const KEY_L: u16 = 0x0F;
const KEY_3: u16 = 0x20;
const KEY_4: u16 = 0x21;
const ESC: u16 = 0x29;
const BACKSPACE: u16 = 0x2A;
const TAB: u16 = 0x2B;
const SPACE: u16 = 0x2C;
const PRINT_SCREEN: u16 = 0x46;
const HOME: u16 = 0x4A;
const DELETE: u16 = 0x4C;
const END: u16 = 0x4D;
const RIGHT: u16 = 0x4F;
const LEFT: u16 = 0x50;
const DOWN: u16 = 0x51;
const UP: u16 = 0x52;
const F4: u16 = 0x3D;
const F11: u16 = 0x44;
/// The target gets no key of its own, only `out_add`.
const NO_KEY: u16 = 0;

/// Windows → Mac, keyed on the PC's physical modifiers.
const WIN_TO_MAC: &[Rule] = &[
    // Word jump / word delete: Ctrl → Option.
    rule(C, None, LEFT, A, LEFT),
    rule(C, None, RIGHT, A, RIGHT),
    rule(C, None, BACKSPACE, A, BACKSPACE),
    rule(C, None, DELETE, A, DELETE),
    // Line / document start and end.
    rule(NO, None, HOME, M, LEFT),
    rule(NO, None, END, M, RIGHT),
    rule(C, None, HOME, M, UP),
    rule(C, None, END, M, DOWN),
    // Switcher and quit.
    rule(A, None, TAB, M, TAB).hold(),
    rule(A, Some(false), F4, M, KEY_Q),
    // The Win key's own shortcuts.
    rule(M, Some(false), KEY_D, NO, F11),
    rule(M, Some(false), KEY_L, C.or(M), KEY_Q),
    rule(M, Some(false), KEY_E, M.or(A), SPACE),
    rule(M, Some(true), KEY_S, M, KEY_4),
    // Screenshot and task manager.
    rule(NO, None, PRINT_SCREEN, M, KEY_3).out_shift(true),
    rule(C, Some(true), ESC, M.or(A), ESC).out_shift(false),
];

/// Mac → Windows, keyed on the Mac's physical modifiers (Cmd is Meta, Option is Alt).
const MAC_TO_WIN: &[Rule] = &[
    rule(A, None, LEFT, C, LEFT),
    rule(A, None, RIGHT, C, RIGHT),
    rule(A, None, BACKSPACE, C, BACKSPACE),
    rule(A, None, DELETE, C, DELETE),
    rule(M, None, LEFT, NO, HOME),
    rule(M, None, RIGHT, NO, END),
    rule(M, None, UP, C, HOME),
    rule(M, None, DOWN, C, END),
    rule(M, None, TAB, A, TAB).hold(),
    rule(M, Some(false), KEY_Q, A, F4),
    // Spotlight → Start: the Win key as the "key" of the chord.
    rule(M, Some(false), SPACE, M, NO_KEY),
    rule(C.or(M), Some(false), KEY_Q, M, KEY_L),
    rule(M, Some(true), KEY_3, NO, PRINT_SCREEN).out_shift(false),
    rule(M, Some(true), KEY_4, M, KEY_S),
    rule(M.or(A), Some(false), ESC, C, ESC).out_shift(true),
];

/// A modifier tapped alone becomes a chord (Win alone → Cmd+Space).
#[derive(Debug, Clone, Copy)]
struct TapRule {
    class: Classes,
    out_add: Classes,
    out_key: u16,
}

const WIN_TAP: TapRule = TapRule {
    class: Classes::META,
    out_add: Classes::META,
    out_key: SPACE,
};

/// A translation in progress: its source key is still held.
#[derive(Debug, Clone, Copy)]
struct Active {
    src: HidUsage,
    out_key: Option<HidUsage>,
    suppress: Classes,
    add: Classes,
    shift: Option<bool>,
}

/// The translator of one direction. Its state is the held translations and the modifiers it
/// has emitted; it lives as long as the capture (a rebuilt engine starts from nothing).
#[derive(Debug, Clone)]
pub struct ShortcutTranslator {
    rules: &'static [Rule],
    tap: Option<TapRule>,
    remap: ModifierRemap,
    /// Keys the translator never touches: the Tandem's own shortcuts match the physical
    /// chord, so their keys keep going through the plain remap.
    exempt: Vec<HidUsage>,
    src_mods: Modifiers,
    out_mods: Modifiers,
    active: Vec<Active>,
    /// Keys pressed and passed on as they are, to release them the same way.
    plain: Vec<HidUsage>,
    /// (consumed class, added class) of an `Alt+Tab`-style hold.
    sticky: Option<(Classes, Classes)>,
    /// A modifier pressed alone that may still turn out to be a tap: not emitted yet.
    pending: Option<ModifierKey>,
}

impl ShortcutTranslator {
    pub fn new(direction: Direction, policy: &RemapPolicy) -> Self {
        let (rules, tap) = match direction {
            Direction::WinToMac => (WIN_TO_MAC, Some(WIN_TAP)),
            Direction::MacToWin => (MAC_TO_WIN, None),
        };
        Self {
            rules,
            tap,
            remap: *policy.for_direction(direction),
            exempt: Vec::new(),
            src_mods: Modifiers::EMPTY,
            out_mods: Modifiers::EMPTY,
            active: Vec::new(),
            plain: Vec::new(),
            sticky: None,
            pending: None,
        }
    }

    /// Keys (source terms) that are never translated.
    pub fn with_exempt(mut self, keys: Vec<HidUsage>) -> Self {
        self.exempt = keys;
        self
    }

    /// Forgets everything held, for when the target was already released from outside (the
    /// session does it on losing focus).
    pub fn reset(&mut self) {
        self.src_mods = Modifiers::EMPTY;
        self.out_mods = Modifiers::EMPTY;
        self.active.clear();
        self.plain.clear();
        self.sticky = None;
        self.pending = None;
    }

    /// What the target must receive for one source event.
    pub fn process(&mut self, event: InputEvent) -> Vec<InputEvent> {
        let mut out = Vec::new();
        match event {
            InputEvent::Key { usage, state } => match (usage.modifier(), state) {
                (Some(m), KeyState::Pressed) => self.modifier_down(m, &mut out),
                (Some(m), KeyState::Released) => self.modifier_up(m, &mut out),
                (None, KeyState::Pressed) => self.key_down(usage, &mut out),
                (None, KeyState::Released) => self.key_up(usage, &mut out),
            },
            InputEvent::Modifiers(m) => {
                self.pending = None;
                self.src_mods = m;
                let desired = self.desired();
                self.out_mods = desired;
                out.push(InputEvent::Modifiers(desired));
            }
            InputEvent::Button { .. } | InputEvent::Scroll(_) => {
                // A click with the key held is a chord, not a tap.
                self.cancel_pending(&mut out);
                out.push(self.remap.apply(event));
            }
            other => out.push(other),
        }
        out
    }

    fn summary(&self) -> Option<(Classes, bool)> {
        if self.src_mods.contains(ModifierKey::AltRight) {
            return None;
        }
        let classes = self
            .src_mods
            .iter()
            .fold(Classes::NONE, |acc, k| acc.or(Classes::of(k)));
        let shift = self.src_mods.contains(ModifierKey::ShiftLeft)
            || self.src_mods.contains(ModifierKey::ShiftRight);
        Some((classes, shift))
    }

    fn find_rule(&self, usage: HidUsage) -> Option<Rule> {
        if usage.page != PAGE_KEYBOARD || self.exempt.contains(&usage) {
            return None;
        }
        let (classes, shift) = self.summary()?;
        self.rules
            .iter()
            .find(|r| r.key == usage.id && r.req == classes && r.shift.is_none_or(|s| s == shift))
            .copied()
    }

    /// The modifiers the target should have down right now.
    fn desired(&mut self) -> Modifiers {
        let mut src = self.src_mods;
        if let Some(p) = self.pending {
            src.remove(p);
        }
        let mut suppress = Classes::NONE;
        let mut add = Classes::NONE;
        let mut shift = None;
        for a in &self.active {
            suppress = suppress.or(a.suppress);
            add = add.or(a.add);
            shift = a.shift.or(shift);
        }
        if let Some((from, to)) = self.sticky {
            if src.iter().any(|k| Classes::of(k).has(from)) {
                suppress = suppress.or(from);
                add = add.or(to);
            } else {
                self.sticky = None;
            }
        }
        let mut kept: Modifiers = src
            .iter()
            .filter(|k| !Classes::of(*k).has(suppress))
            .collect();
        match shift {
            Some(false) => {
                kept.remove(ModifierKey::ShiftLeft);
                kept.remove(ModifierKey::ShiftRight);
            }
            Some(true)
                if !kept.contains(ModifierKey::ShiftLeft)
                    && !kept.contains(ModifierKey::ShiftRight) =>
            {
                kept.insert(ModifierKey::ShiftLeft);
            }
            _ => {}
        }
        let mut out = self.remap.map_modifiers(kept);
        for k in add.keys() {
            out.insert(k);
        }
        out
    }

    /// Emits the modifier changes needed to reach `desired()`: releases first, then presses.
    fn sync(&mut self, out: &mut Vec<InputEvent>) {
        let desired = self.desired();
        for k in self.out_mods.iter().filter(|k| !desired.contains(*k)) {
            out.push(release(k.usage()));
        }
        for k in desired.iter().filter(|k| !self.out_mods.contains(*k)) {
            out.push(press(k.usage()));
        }
        self.out_mods = desired;
    }

    fn cancel_pending(&mut self, out: &mut Vec<InputEvent>) {
        if self.pending.take().is_some() {
            self.sync(out);
        }
    }

    fn modifier_down(&mut self, m: ModifierKey, out: &mut Vec<InputEvent>) {
        let alone = self.src_mods.is_empty() && self.active.is_empty() && self.plain.is_empty();
        let tap = self.tap.is_some_and(|t| Classes::of(m).has(t.class));
        self.src_mods.insert(m);
        if alone && tap {
            self.pending = Some(m);
            return;
        }
        self.pending = None;
        self.sync(out);
    }

    fn modifier_up(&mut self, m: ModifierKey, out: &mut Vec<InputEvent>) {
        self.src_mods.remove(m);
        if self.pending == Some(m) {
            self.pending = None;
            if let Some(t) = self.tap {
                // Net zero on the target's modifier state: the whole chord in one go.
                let mods: Vec<_> = t.out_add.keys().collect();
                let key = HidUsage::key(t.out_key);
                for k in &mods {
                    out.push(press(k.usage()));
                }
                out.push(press(key));
                out.push(release(key));
                for k in mods.iter().rev() {
                    out.push(release(k.usage()));
                }
            }
            return;
        }
        self.sync(out);
    }

    fn key_down(&mut self, usage: HidUsage, out: &mut Vec<InputEvent>) {
        // Repeat of something already translated or passed on.
        if let Some(a) = self.active.iter().find(|a| a.src == usage) {
            if let Some(k) = a.out_key {
                out.push(press(k));
            }
            return;
        }
        if self.plain.contains(&usage) {
            self.sync(out);
            out.push(press(usage));
            return;
        }
        let found = self.find_rule(usage);
        self.pending = None;
        match found {
            Some(r) => {
                let out_key = (r.out_key != NO_KEY).then(|| HidUsage::key(r.out_key));
                self.active.push(Active {
                    src: usage,
                    out_key,
                    suppress: r.req,
                    add: r.out_add,
                    shift: r.out_shift,
                });
                if r.hold {
                    self.sticky = Some((r.req, r.out_add));
                }
                self.sync(out);
                if let Some(k) = out_key {
                    out.push(press(k));
                }
            }
            None => {
                self.plain.push(usage);
                self.sync(out);
                out.push(press(usage));
            }
        }
    }

    fn key_up(&mut self, usage: HidUsage, out: &mut Vec<InputEvent>) {
        if let Some(i) = self.active.iter().position(|a| a.src == usage) {
            let a = self.active.remove(i);
            if let Some(k) = a.out_key {
                out.push(release(k));
            }
            self.sync(out);
            return;
        }
        self.plain.retain(|k| *k != usage);
        out.push(release(usage));
    }
}

fn press(usage: HidUsage) -> InputEvent {
    InputEvent::Key {
        usage,
        state: KeyState::Pressed,
    }
}

fn release(usage: HidUsage) -> InputEvent {
    InputEvent::Key {
        usage,
        state: KeyState::Released,
    }
}

#[cfg(test)]
mod tests;
