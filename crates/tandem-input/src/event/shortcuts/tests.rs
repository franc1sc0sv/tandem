use std::collections::BTreeSet;

use proptest::prelude::*;

use super::*;
use crate::event::{Button, Scroll};

const NAMES: &[(&str, u16)] = &[
    ("ctrl", 0xE0),
    ("shift", 0xE1),
    ("alt", 0xE2),
    ("meta", 0xE3),
    ("rctrl", 0xE4),
    ("rshift", 0xE5),
    ("ralt", 0xE6),
    ("rmeta", 0xE7),
    ("a", 0x04),
    ("c", 0x06),
    ("d", 0x07),
    ("e", 0x08),
    ("l", 0x0F),
    ("q", 0x14),
    ("s", 0x16),
    ("3", 0x20),
    ("4", 0x21),
    ("esc", 0x29),
    ("bs", 0x2A),
    ("tab", 0x2B),
    ("space", 0x2C),
    ("f4", 0x3D),
    ("f11", 0x44),
    ("prtsc", 0x46),
    ("home", 0x4A),
    ("delete", 0x4C),
    ("end", 0x4D),
    ("right", 0x4F),
    ("left", 0x50),
    ("down", 0x51),
    ("up", 0x52),
    ("f2", 0x3B),
];

fn usage(name: &str) -> HidUsage {
    let id = NAMES.iter().find(|(n, _)| *n == name).expect(name).1;
    HidUsage::key(id)
}

fn name(u: HidUsage) -> &'static str {
    NAMES.iter().find(|(_, id)| *id == u.id).expect("name").0
}

fn translator(d: Direction) -> ShortcutTranslator {
    ShortcutTranslator::new(d, &RemapPolicy::default())
}

/// Parses "+ctrl +left -left -ctrl" and returns what the target receives, in the same form.
fn run_with(t: &mut ShortcutTranslator, script: &str) -> String {
    let mut out = Vec::new();
    for tok in script.split_whitespace() {
        let (sign, n) = tok.split_at(1);
        let state = if sign == "+" {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        for e in t.process(InputEvent::Key {
            usage: usage(n),
            state,
        }) {
            match e {
                InputEvent::Key { usage, state } => out.push(format!(
                    "{}{}",
                    if state == KeyState::Pressed { "+" } else { "-" },
                    name(usage)
                )),
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    out.join(" ")
}

fn run(d: Direction, script: &str) -> String {
    run_with(&mut translator(d), script)
}

/// What the target holds when `key` goes down with `mods` held: sorted modifiers + key.
/// It also checks that releasing everything leaves the target with nothing pressed.
fn chord(d: Direction, mods: &[&str], key: &str) -> String {
    let mut t = translator(d);
    let mut held: BTreeSet<HidUsage> = BTreeSet::new();
    let apply = |events: Vec<InputEvent>, held: &mut BTreeSet<HidUsage>| {
        for e in events {
            match e {
                InputEvent::Key {
                    usage,
                    state: KeyState::Pressed,
                } => {
                    held.insert(usage);
                }
                InputEvent::Key {
                    usage,
                    state: KeyState::Released,
                } => {
                    held.remove(&usage);
                }
                _ => {}
            }
        }
    };
    let key_event = |n: &str, state| InputEvent::Key {
        usage: usage(n),
        state,
    };
    for m in mods {
        apply(t.process(key_event(m, KeyState::Pressed)), &mut held);
    }
    apply(t.process(key_event(key, KeyState::Pressed)), &mut held);
    let mut names: Vec<(u16, &str)> = held.iter().map(|u| (u.id, name(*u))).collect();
    // Modifiers first, in HID order, then the key.
    names.sort_by_key(|(id, _)| if *id >= 0xE0 { *id } else { 0x100 + *id });
    let shown = names.iter().map(|(_, n)| *n).collect::<Vec<_>>().join("+");
    apply(t.process(key_event(key, KeyState::Released)), &mut held);
    for m in mods.iter().rev() {
        apply(t.process(key_event(m, KeyState::Released)), &mut held);
    }
    assert!(held.is_empty(), "stuck after {mods:?}+{key}: {held:?}");
    shown
}

use Direction::{MacToWin, WinToMac};

#[test]
fn win_to_mac_table() {
    // (PC chord) → (Mac chord at the moment the key goes down). On the Mac, `meta` is Cmd,
    // `alt` is Option and `ctrl` is Ctrl.
    let cases: &[(&[&str], &str, &str)] = &[
        (&["ctrl"], "left", "alt+left"),
        (&["ctrl"], "right", "alt+right"),
        (&["ctrl", "shift"], "left", "shift+alt+left"),
        (&["ctrl", "shift"], "right", "shift+alt+right"),
        (&["ctrl"], "bs", "alt+bs"),
        (&["ctrl"], "delete", "alt+delete"),
        (&[], "home", "meta+left"),
        (&[], "end", "meta+right"),
        (&["shift"], "home", "shift+meta+left"),
        (&["shift"], "end", "shift+meta+right"),
        (&["ctrl"], "home", "meta+up"),
        (&["ctrl"], "end", "meta+down"),
        (&["ctrl", "shift"], "home", "shift+meta+up"),
        (&["ctrl", "shift"], "end", "shift+meta+down"),
        (&["alt"], "tab", "meta+tab"),
        (&["alt", "shift"], "tab", "shift+meta+tab"),
        (&["alt"], "f4", "meta+q"),
        (&["meta"], "d", "f11"),
        (&["meta"], "l", "ctrl+meta+q"),
        (&["meta"], "e", "alt+meta+space"),
        (&["meta", "shift"], "s", "shift+meta+4"),
        (&[], "prtsc", "shift+meta+3"),
        (&["shift"], "prtsc", "shift+meta+3"),
        (&["ctrl", "shift"], "esc", "alt+meta+esc"),
        // The right-hand modifiers count like the left ones.
        (&["rctrl"], "left", "alt+left"),
        (&["rmeta"], "d", "f11"),
    ];
    for (mods, key, want) in cases {
        assert_eq!(chord(WinToMac, mods, key), *want, "{mods:?}+{key}");
    }
}

#[test]
fn mac_to_win_table() {
    let cases: &[(&[&str], &str, &str)] = &[
        (&["alt"], "left", "ctrl+left"),
        (&["alt"], "right", "ctrl+right"),
        (&["alt", "shift"], "left", "ctrl+shift+left"),
        (&["alt"], "bs", "ctrl+bs"),
        (&["alt"], "delete", "ctrl+delete"),
        (&["meta"], "left", "home"),
        (&["meta"], "right", "end"),
        (&["meta", "shift"], "left", "shift+home"),
        (&["meta", "shift"], "right", "shift+end"),
        (&["meta"], "up", "ctrl+home"),
        (&["meta"], "down", "ctrl+end"),
        (&["meta", "shift"], "up", "ctrl+shift+home"),
        (&["meta", "shift"], "down", "ctrl+shift+end"),
        (&["meta"], "tab", "alt+tab"),
        (&["meta", "shift"], "tab", "shift+alt+tab"),
        (&["meta"], "q", "alt+f4"),
        (&["meta"], "space", "meta"),
        (&["ctrl", "meta"], "q", "meta+l"),
        (&["meta", "shift"], "3", "prtsc"),
        (&["meta", "shift"], "4", "shift+meta+s"),
        (&["meta", "alt"], "esc", "ctrl+shift+esc"),
    ];
    for (mods, key, want) in cases {
        assert_eq!(chord(MacToWin, mods, key), *want, "{mods:?}+{key}");
    }
}

#[test]
fn unmapped_chords_keep_the_plain_remap() {
    // Ctrl+C on the PC is Cmd+C on the Mac, and the other way around.
    assert_eq!(chord(WinToMac, &["ctrl"], "c"), "meta+c");
    assert_eq!(chord(MacToWin, &["meta"], "c"), "ctrl+c");
    assert_eq!(chord(WinToMac, &["alt"], "left"), "alt+left");
    assert_eq!(chord(WinToMac, &["meta"], "c"), "ctrl+c");
    // F2 is not translated, nor is Cmd+Backspace.
    assert_eq!(chord(WinToMac, &[], "f2"), "f2");
    assert_eq!(chord(MacToWin, &["meta"], "bs"), "ctrl+bs");
    // More modifiers than the rule asks for: not translated.
    assert_eq!(chord(WinToMac, &["ctrl", "alt"], "left"), "alt+meta+left");
    // AltGr types characters: nothing is translated while it is held.
    assert_eq!(chord(WinToMac, &["ralt"], "home"), "ralt+home");
    assert_eq!(chord(WinToMac, &["ctrl", "ralt"], "left"), "meta+ralt+left");
}

#[test]
fn ctrl_left_sequence_in_order() {
    assert_eq!(
        run(WinToMac, "+ctrl +left -left -ctrl"),
        // The Cmd that Ctrl first became is taken back before the key goes down.
        "+meta -meta +alt +left -left -alt +meta -meta"
    );
}

#[test]
fn ctrl_stays_translated_for_the_next_key_after_the_chord() {
    assert_eq!(
        run(WinToMac, "+ctrl +left -left +c -c -ctrl"),
        "+meta -meta +alt +left -left -alt +meta +c -c -meta"
    );
}

#[test]
fn modifiers_can_be_released_before_the_key() {
    let mut t = translator(WinToMac);
    let out = run_with(&mut t, "+ctrl +left -ctrl");
    // Option stays for the key's life: the repeat is still Option+Left.
    assert_eq!(out, "+meta -meta +alt +left");
    assert_eq!(run_with(&mut t, "+left"), "+left");
    assert_eq!(run_with(&mut t, "-left"), "-left -alt");
}

#[test]
fn key_repeat_repeats_the_target_key_only() {
    let mut t = translator(WinToMac);
    run_with(&mut t, "+ctrl +left");
    assert_eq!(run_with(&mut t, "+left +left +left"), "+left +left +left");
    assert_eq!(run_with(&mut t, "-left -ctrl"), "-left -alt +meta -meta");
}

#[test]
fn alt_tab_keeps_cmd_while_alt_is_held() {
    let mut t = translator(WinToMac);
    let out = run_with(&mut t, "+alt +tab -tab +tab -tab +tab -tab");
    // Option goes down when Alt does and is swapped for Cmd once Tab arrives; Cmd then
    // stays through the released Tabs so the switcher does not close.
    assert_eq!(out, "+alt -alt +meta +tab -tab +tab -tab +tab -tab");
    assert_eq!(run_with(&mut t, "-alt"), "-meta");
}

#[test]
fn alt_shift_tab_goes_backwards() {
    let mut t = translator(WinToMac);
    let out = run_with(&mut t, "+alt +shift +tab -tab -shift -alt");
    assert_eq!(out, "+alt +shift -alt +meta +tab -tab -shift -meta");
}

#[test]
fn alt_released_before_tab_closes_the_switcher_after_the_key() {
    let mut t = translator(WinToMac);
    let out = run_with(&mut t, "+alt +tab -alt -tab");
    // Cmd outlives Alt for as long as the Tab that started the chord is held.
    assert_eq!(out, "+alt -alt +meta +tab -tab -meta");
}

#[test]
fn cmd_tab_holds_alt_toward_the_pc() {
    let mut t = translator(MacToWin);
    // Cmd goes to Ctrl first and is swapped for Alt once Tab arrives.
    assert_eq!(
        run_with(&mut t, "+meta +tab -tab +tab -tab"),
        "+ctrl -ctrl +alt +tab -tab +tab -tab"
    );
    assert_eq!(run_with(&mut t, "-meta"), "-alt");
}

#[test]
fn win_tap_opens_spotlight() {
    assert_eq!(
        run(WinToMac, "+meta -meta"),
        "+meta +space -space -meta",
        "Win alone is Cmd+Space"
    );
}

#[test]
fn win_held_with_a_key_is_not_a_tap() {
    // Win+C is Ctrl+C on the Mac: the Win press was held back and comes out with the key.
    assert_eq!(run(WinToMac, "+meta +c -c -meta"), "+ctrl +c -c -ctrl");
    // Win+Ctrl: another modifier cancels the tap.
    assert_eq!(
        run(WinToMac, "+meta +ctrl -ctrl -meta"),
        "+ctrl +meta -meta -ctrl"
    );
    // Win+D is show desktop and does not open Spotlight on release.
    assert_eq!(run(WinToMac, "+meta +d -d -meta"), "+f11 -f11 +ctrl -ctrl");
}

#[test]
fn win_tap_is_cancelled_by_a_click_or_scroll() {
    let mut t = translator(WinToMac);
    run_with(&mut t, "+meta");
    let out = t.process(InputEvent::Button {
        button: Button::Left,
        state: KeyState::Pressed,
    });
    assert_eq!(out.len(), 2, "{out:?}");
    assert_eq!(out[0], press(ModifierKey::ControlLeft.usage()));
    assert_eq!(run_with(&mut t, "-meta"), "-ctrl");
    // No tap on the other direction: Cmd alone does nothing special.
    assert_eq!(run(MacToWin, "+meta -meta"), "+ctrl -ctrl");
    let mut t = translator(WinToMac);
    run_with(&mut t, "+meta");
    let out = t.process(InputEvent::Scroll(Scroll::Wheel {
        dx_v120: 0,
        dy_v120: 120,
    }));
    assert_eq!(out.len(), 2);
}

#[test]
fn motion_does_not_cancel_a_tap() {
    let mut t = translator(WinToMac);
    run_with(&mut t, "+meta");
    let motion = InputEvent::MotionRelative { dx: 1.0, dy: 2.0 };
    assert_eq!(t.process(motion), vec![motion]);
    assert_eq!(run_with(&mut t, "-meta"), "+meta +space -space -meta");
}

#[test]
fn a_key_pressed_before_the_modifier_is_not_translated_on_repeat() {
    let mut t = translator(WinToMac);
    assert_eq!(run_with(&mut t, "+left"), "+left");
    // Ctrl arrives and the held Left repeats: it stays plain, so its release matches.
    assert_eq!(run_with(&mut t, "+ctrl +left"), "+meta +left");
    assert_eq!(run_with(&mut t, "-left -ctrl"), "-left -meta");
}

#[test]
fn exempt_keys_are_never_translated() {
    let mut t = translator(WinToMac).with_exempt(vec![usage("left")]);
    assert_eq!(
        run_with(&mut t, "+ctrl +left -left -ctrl"),
        "+meta +left -left -meta"
    );
}

#[test]
fn modifiers_event_syncs_with_the_translated_state() {
    let mut t = translator(WinToMac);
    let held: Modifiers = [ModifierKey::ControlLeft].into_iter().collect();
    let out = t.process(InputEvent::Modifiers(held));
    // Ctrl on the PC is Cmd on the Mac.
    assert_eq!(
        out,
        vec![InputEvent::Modifiers(
            [ModifierKey::MetaLeft].into_iter().collect()
        )]
    );
    assert_eq!(run_with(&mut t, "-ctrl"), "-meta");
}

#[test]
fn reset_forgets_everything() {
    let mut t = translator(WinToMac);
    run_with(&mut t, "+ctrl +left");
    t.reset();
    assert_eq!(run_with(&mut t, "+ctrl"), "+meta");
}

#[test]
fn non_key_events_pass_unchanged() {
    let mut t = translator(MacToWin);
    for e in [
        InputEvent::MotionAbsolute { x: 1.0, y: 2.0 },
        InputEvent::Scroll(Scroll::Pixels { dx: 0.0, dy: 3.0 }),
    ] {
        assert_eq!(t.process(e), vec![e]);
    }
}

#[test]
fn media_keys_travel_as_they_are() {
    // Consumer-page usages (volume, play/pause...) never match a rule.
    let mut t = translator(WinToMac);
    for id in [0xE2, 0xE9, 0xEA, 0xCD, 0xB5, 0xB6, 0x6F, 0x70] {
        let u = HidUsage::consumer(id);
        for state in [KeyState::Pressed, KeyState::Released] {
            let e = InputEvent::Key { usage: u, state };
            assert_eq!(t.process(e), vec![e]);
        }
    }
    // Also with a modifier held: Ctrl+Volume does not become Option+Volume.
    run_with(&mut t, "+ctrl");
    let e = InputEvent::Key {
        usage: HidUsage::consumer(0x50),
        state: KeyState::Pressed,
    };
    assert_eq!(t.process(e), vec![e]);
}

fn arb_event() -> impl Strategy<Value = (u16, bool)> {
    let ids: Vec<u16> = NAMES.iter().map(|(_, id)| *id).collect();
    (prop::sample::select(ids), any::<bool>())
}

fn simulate(d: Direction, events: &[(u16, bool)]) -> (BTreeSet<HidUsage>, Vec<InputEvent>) {
    let mut t = translator(d);
    let mut held: BTreeSet<HidUsage> = BTreeSet::new();
    let mut src_down: BTreeSet<u16> = BTreeSet::new();
    let mut all = Vec::new();
    let mut apply = |evs: Vec<InputEvent>, held: &mut BTreeSet<HidUsage>| {
        for e in evs {
            all.push(e);
            if let InputEvent::Key { usage, state } = e {
                if state == KeyState::Pressed {
                    held.insert(usage);
                } else {
                    held.remove(&usage);
                }
            }
        }
    };
    for &(id, down) in events {
        let state = if down {
            src_down.insert(id);
            KeyState::Pressed
        } else {
            src_down.remove(&id);
            KeyState::Released
        };
        apply(
            t.process(InputEvent::Key {
                usage: HidUsage::key(id),
                state,
            }),
            &mut held,
        );
    }
    // Release whatever is still down at the source, in the order it was left.
    for id in src_down {
        apply(
            t.process(InputEvent::Key {
                usage: HidUsage::key(id),
                state: KeyState::Released,
            }),
            &mut held,
        );
    }
    (held, all)
}

proptest! {
    /// Whatever the source does, once everything is released the target holds nothing.
    #[test]
    fn nothing_stays_pressed(
        events in prop::collection::vec(arb_event(), 0..60),
        win_to_mac in any::<bool>(),
    ) {
        let d = if win_to_mac { WinToMac } else { MacToWin };
        let (held, _) = simulate(d, &events);
        prop_assert!(held.is_empty(), "stuck: {held:?}");
    }

    /// Keys that are neither modifiers nor in any rule come out as they went in, and
    /// without modifiers held the stream is the identity.
    #[test]
    fn unmapped_keys_are_identity(
        keys in prop::collection::vec(prop::sample::select(vec![0x04u16, 0x06, 0x3B]), 0..30),
        win_to_mac in any::<bool>(),
    ) {
        let d = if win_to_mac { WinToMac } else { MacToWin };
        let mut t = translator(d);
        for id in keys {
            for state in [KeyState::Pressed, KeyState::Released] {
                let e = InputEvent::Key { usage: HidUsage::key(id), state };
                prop_assert_eq!(t.process(e), vec![e]);
            }
        }
    }

    /// The target never receives a press of something already down nor the release of
    /// something that is not (except key repeats of translated keys).
    #[test]
    fn modifier_events_are_balanced(
        events in prop::collection::vec(arb_event(), 0..60),
        win_to_mac in any::<bool>(),
    ) {
        let d = if win_to_mac { WinToMac } else { MacToWin };
        let (_, all) = simulate(d, &events);
        let mut down: BTreeSet<HidUsage> = BTreeSet::new();
        for e in all {
            if let InputEvent::Key { usage, state } = e && usage.modifier().is_some() {
                if state == KeyState::Pressed {
                    prop_assert!(down.insert(usage), "double press of {usage:?}");
                } else {
                    prop_assert!(down.remove(&usage), "release of {usage:?} not down");
                }
            }
        }
    }
}
