//! Each machine's screens as the input engine sees them, how they travel to the other
//! agent ([`ScreenRect`]) and the arrangement that comes from them and the desk (DESIGN §7).
//!
//! Per machine its `[[screens]]` declared in the desk apply; if undeclared, the ones it
//! reads itself (the bounds injection uses). A machine that is off has no screens:
//! the cursor does not cross to it. A monitor that does not show this machine (it shows the
//! other, or it is off, in standby or on an unplugged input) ceases to be a screen of this one
//! (DESIGN §7, T-111, T-349): its screen comes from the desk's `[[cables]]`.

use std::collections::BTreeMap;

use tandem_core::{Config, DisplayKey, Machine, MonitorLink, ScreenSpec, State};
use tandem_input::displays::Display;
use tandem_input::geometry::{Arrangement, GeometryError, Layout, LayoutOptions, Rect, Screen};
use tandem_proto::ScreenRect;

pub fn to_wire(s: &Screen) -> ScreenRect {
    ScreenRect {
        id: s.id.0.clone(),
        x: s.rect.x,
        y: s.rect.y,
        width: s.rect.width,
        height: s.rect.height,
        // Absurd scales do not arrive: `Layout::compute` rejects them first.
        scale_milli: (s.scale * 1000.0).round().clamp(1.0, f64::from(u32::MAX)) as u32,
    }
}

pub fn from_wire(r: &ScreenRect) -> Screen {
    Screen::new(
        r.id.as_str().into(),
        Rect::new(r.x, r.y, r.width, r.height),
        f64::from(r.scale_milli) / 1000.0,
    )
}

/// A screen declared in the desk.
pub fn from_spec(s: &ScreenSpec) -> Screen {
    Screen::new(
        s.id.as_str().into(),
        Rect::new(s.x, s.y, s.width, s.height),
        s.scale.as_f64(),
    )
}

/// The ones a machine reads, with an id per screen (`screen-1`, `screen-2`...; saved `[[cables]]`
/// refer to them, and `Desk::migrate_legacy_ids` renames the old `pantalla-N`). The id only has to be unique on its machine, so it
/// does not depend on the side.
pub fn detected(bounds: &[Rect]) -> Vec<Screen> {
    bounds
        .iter()
        .filter(|r| r.width > 0 && r.height > 0)
        .enumerate()
        .map(|(i, r)| Screen::new(format!("screen-{}", i + 1).as_str().into(), *r, 1.0))
        .collect()
}

/// The EDID identity of each screen [`detected`] names, under the same id (T-340): `displays`
/// come in the order of the bounds, each with the identity the system gives, if it gives one.
pub fn detected_keys(displays: &[Display]) -> Vec<(String, DisplayKey)> {
    displays
        .iter()
        .filter(|(r, _)| r.width > 0 && r.height > 0)
        .enumerate()
        .filter_map(|(i, (_, key))| Some((format!("screen-{}", i + 1), (*key)?)))
        .collect()
}

/// The derived link of each monitor, by id ([`crate::monitors::MonitorHub::links`]).
pub type Links = BTreeMap<String, MonitorLink>;

/// The screens of `machine` whose monitor does not show it ([`MonitorLink::live_for`]):
/// according to `links` and the screen of each `[[cables]]`. A monitor without a declared screen
/// cannot be removed.
pub fn hidden(desk: &Config, machine: Machine, links: &Links) -> Vec<String> {
    desk.cables
        .iter()
        .filter(|c| links.get(&c.monitor).is_some_and(|l| !l.live_for(machine)))
        .filter_map(|c| c.end(machine).screen.clone())
        .collect()
}

/// The arrangement: this machine's screens (`me`) and the other's, declared in
/// `desk` or, if not, the ones read (`mine` here, `theirs` the ones the other agent sent), without the
/// ones that show the other machine ([`hidden`]), and the desk's side and alignment (D18).
/// Without a declared arrangement, A to the left of B with the whole edge (the usual default
/// value).
pub fn arrangement(
    desk: &Config,
    state: &State,
    me: Machine,
    mine: &[Screen],
    theirs: &[Screen],
    links: &Links,
) -> Arrangement {
    let of = |machine: Machine| -> Vec<Screen> {
        if !state.is_on(machine) {
            return Vec::new();
        }
        let mut screens: Vec<Screen> = desk.screens_of(machine).map(from_spec).collect();
        if screens.is_empty() {
            screens = if machine == me {
                mine.to_vec()
            } else {
                theirs.to_vec()
            };
        }
        let hidden = hidden(desk, machine, links);
        screens.retain(|s| !hidden.contains(&s.id.0));
        screens
    };
    Arrangement {
        a: of(Machine::A),
        b: of(Machine::B),
        a_side: desk.arrangement.map(|a| a.a_side).unwrap_or_default(),
        align: desk.arrangement.map(|a| a.align),
    }
}

/// The arrangement's geometry.
pub fn layout(
    desk: &Config,
    state: &State,
    me: Machine,
    (mine, theirs): (&[Screen], &[Screen]),
    links: &Links,
    opts: LayoutOptions,
) -> Result<Layout, GeometryError> {
    let mut layout = Layout::compute(&arrangement(desk, state, me, mine, theirs, links), opts)?;
    for machine in [Machine::A, Machine::B] {
        let own = if machine == me { mine } else { theirs };
        if shown_elsewhere(desk, state, machine, own, links) {
            layout = layout.with_shown_elsewhere(machine);
        }
    }
    Ok(layout)
}

/// `machine` is on and has screens, and every one of them shows the other machine (or is
/// off): a scene, not screens still unknown. Then the keyboard and mouse go over there.
pub fn shown_elsewhere(
    desk: &Config,
    state: &State,
    machine: Machine,
    read: &[Screen],
    links: &Links,
) -> bool {
    if !state.is_on(machine) {
        return false;
    }
    let declared: Vec<Screen> = desk.screens_of(machine).map(from_spec).collect();
    let screens = if declared.is_empty() { read } else { &declared };
    let hidden = hidden(desk, machine, links);
    !screens.is_empty() && screens.iter().all(|s| hidden.contains(&s.id.0))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tandem_core::{Align, LinkState, Powered, Scale, Side};

    use super::*;

    fn both_on() -> State {
        State {
            on: Powered::both(),
            scene: None,
        }
    }

    fn mon(id: &str, x: i32, w: u32) -> Screen {
        Screen::new(id.into(), Rect::new(x, 0, w, 1440), 1.0)
    }

    #[test]
    fn detected_screens_get_one_id_each() {
        let got = detected(&[
            Rect::new(-3440, 0, 3440, 1440),
            Rect::new(0, 0, 0, 10),
            Rect::new(0, 0, 1512, 982),
        ]);
        let ids: Vec<_> = got.iter().map(|s| s.id.0.as_str()).collect();
        assert_eq!(ids, ["screen-1", "screen-2"]);
        assert_eq!(got[0].rect, Rect::new(-3440, 0, 3440, 1440));
        assert_eq!(got[1].scale, 1.0);
    }

    /// T-340: the identity of each screen goes under the same id [`detected`] gives it.
    #[test]
    fn each_screen_key_has_the_id_of_its_screen() {
        let key = |product| DisplayKey {
            vendor: 0x4C2D,
            product,
            serial: 0,
        };
        let displays = [
            (Rect::new(0, 0, 1512, 982), None),
            (Rect::new(0, 0, 0, 10), Some(key(9))),
            (Rect::new(1512, 0, 3440, 1440), Some(key(0x7105))),
        ];
        let rects: Vec<Rect> = displays.iter().map(|(r, _)| *r).collect();
        let ids: Vec<_> = detected(&rects).into_iter().map(|s| s.id.0).collect();
        assert_eq!(ids, ["screen-1", "screen-2"]);
        assert_eq!(
            detected_keys(&displays),
            [("screen-2".to_owned(), key(0x7105))]
        );
    }

    #[test]
    fn each_machine_puts_its_screens_on_its_side() {
        let desk = Config::default();
        let m34 = mon("pc-1", 0, 3440);
        let mut state = State::initial(Machine::A);
        state.on.b = true;
        let opts = LayoutOptions::default();
        let none = Links::new();
        let pc = layout(
            &desk,
            &state,
            Machine::B,
            (std::slice::from_ref(&m34), &[]),
            &none,
            opts,
        )
        .unwrap();
        let mac = layout(&desk, &state, Machine::A, (&[], &[m34]), &none, opts).unwrap();
        assert_eq!(pc, mac);
        assert_eq!(pc.screens(Machine::B).len(), 1);
        assert!(pc.screens(Machine::A).is_empty());
    }

    #[test]
    fn declared_screens_replace_the_detected_ones_and_set_the_side() {
        let mut desk = Config {
            screens: vec![ScreenSpec {
                machine: Machine::B,
                id: "34".into(),
                x: 0,
                y: 0,
                width: 3440,
                height: 1440,
                scale: Scale::from_milli(1250),
            }],
            ..Config::default()
        };
        desk.arrangement = Some(tandem_core::Arrangement {
            a_side: Side::Right,
            align: Align::End,
        });
        let mine = [mon("mac-1", 0, 1512)];
        let theirs = [mon("pc-1", 0, 1920)];
        let arr = arrangement(&desk, &both_on(), Machine::A, &mine, &theirs, &Links::new());
        assert_eq!(arr.a, mine);
        assert_eq!(arr.b.len(), 1);
        assert_eq!(arr.b[0].id, "34".into());
        assert_eq!(arr.b[0].scale, 1.25);
        assert_eq!((arr.a_side, arr.align), (Side::Right, Some(Align::End)));
        // Undeclared, the Mac goes to the left, without alignment, and the ones read apply.
        let arr = arrangement(
            &Config::default(),
            &both_on(),
            Machine::A,
            &mine,
            &theirs,
            &Links::new(),
        );
        assert_eq!((arr.a, arr.b), (mine.to_vec(), theirs.to_vec()));
        assert_eq!((arr.a_side, arr.align), (Side::Left, None));
    }

    #[test]
    fn a_machine_that_is_off_has_no_screens() {
        let mine = [mon("mac-1", 0, 1512)];
        let theirs = [mon("pc-1", 0, 1920)];
        let arr = arrangement(
            &Config::default(),
            &State::initial(Machine::A),
            Machine::A,
            &mine,
            &theirs,
            &Links::new(),
        );
        assert!(arr.b.is_empty());
        assert_eq!(arr.a, mine);
    }

    /// T-111: the 34" shows the Mac (A) -> it ceases to be a screen of the PC (B), which is left
    /// without screens; back to the PC -> it leaves the Mac. Without a declared screen nothing is touched.
    #[test]
    fn a_monitor_showing_the_other_machine_leaves_the_geometry() {
        let mut desk = Config::default();
        let mut cable = tandem_core::MonitorCable::new("m34");
        cable.a.screen = Some("mac-2".into());
        cable.b.screen = Some("pc-1".into());
        desk.cables.push(cable);
        let mine = [mon("mac-1", 0, 1512), mon("mac-2", 1512, 3440)];
        let theirs = [mon("pc-1", 0, 3440)];
        let shows =
            |m: Machine| Links::from([("m34".to_owned(), link(LinkState::ShowsMachine(m)))]);
        let arr = arrangement(
            &desk,
            &both_on(),
            Machine::A,
            &mine,
            &theirs,
            &shows(Machine::A),
        );
        assert_eq!(arr.a, mine);
        assert!(arr.b.is_empty(), "the PC has nowhere to show the cursor");
        let arr = arrangement(
            &desk,
            &both_on(),
            Machine::A,
            &mine,
            &theirs,
            &shows(Machine::B),
        );
        assert_eq!(arr.a, mine[..1]);
        assert_eq!(arr.b, theirs);
        // Without knowing what it shows, or without a declared screen, all remain.
        let arr = arrangement(&desk, &both_on(), Machine::A, &mine, &theirs, &Links::new());
        assert_eq!((arr.a.len(), arr.b.len()), (2, 1));
        desk.cables[0].a.screen = None;
        let arr = arrangement(
            &desk,
            &both_on(),
            Machine::A,
            &mine,
            &theirs,
            &shows(Machine::B),
        );
        assert_eq!(arr.a, mine);
    }

    /// Scene "only Mac": the PC has screens, but all show the Mac. That (and not screens still
    /// unknown, or a machine that is off) is what sends the cursor to the Mac by itself.
    #[test]
    fn a_machine_whose_every_screen_shows_the_other_is_marked() {
        let mut desk = Config::default();
        let mut cable = tandem_core::MonitorCable::new("m34");
        cable.a.screen = Some("mac-1".into());
        cable.b.screen = Some("pc-1".into());
        desk.cables.push(cable);
        let mac = [mon("mac-1", 0, 3440)];
        let pc = [mon("pc-1", 0, 3440)];
        let shows =
            |m: Machine| Links::from([("m34".to_owned(), link(LinkState::ShowsMachine(m)))]);
        let opts = LayoutOptions::default();
        let l = layout(
            &desk,
            &both_on(),
            Machine::B,
            (&pc, &mac),
            &shows(Machine::A),
            opts,
        )
        .unwrap();
        assert!(l.shown_elsewhere(Machine::B));
        assert!(!l.shown_elsewhere(Machine::A));
        // The same seen from the Mac.
        let l = layout(
            &desk,
            &both_on(),
            Machine::A,
            (&mac, &pc),
            &shows(Machine::A),
            opts,
        )
        .unwrap();
        assert!(l.shown_elsewhere(Machine::B));
        // Back to the PC, or without knowing what it shows: not marked.
        let l = layout(
            &desk,
            &both_on(),
            Machine::B,
            (&pc, &mac),
            &shows(Machine::B),
            opts,
        )
        .unwrap();
        assert!(!l.shown_elsewhere(Machine::B));
        let l = layout(
            &desk,
            &both_on(),
            Machine::B,
            (&pc, &mac),
            &Links::new(),
            opts,
        )
        .unwrap();
        assert!(!l.shown_elsewhere(Machine::B));
        // The PC's screens not read yet: no screens, but it is not a scene.
        let l = layout(
            &desk,
            &both_on(),
            Machine::B,
            (&[], &mac),
            &shows(Machine::A),
            opts,
        )
        .unwrap();
        assert!(l.screens(Machine::B).is_empty());
        assert!(!l.shown_elsewhere(Machine::B));
        // Off: it is not marked either.
        let off = State::initial(Machine::A);
        assert!(!shown_elsewhere(
            &desk,
            &off,
            Machine::B,
            &pc,
            &shows(Machine::A)
        ));
    }

    fn link(state: LinkState) -> MonitorLink {
        MonitorLink {
            state,
            ..MonitorLink::default()
        }
    }

    /// T-349: a monitor that goes to standby or off leaves the geometry of both machines; one
    /// whose state is unknown stays.
    #[test]
    fn a_monitor_in_standby_or_off_leaves_the_geometry() {
        let mut desk = Config::default();
        let mut cable = tandem_core::MonitorCable::new("m34");
        cable.a.screen = Some("mac-2".into());
        cable.b.screen = Some("pc-1".into());
        desk.cables.push(cable);
        let mine = [mon("mac-1", 0, 1512), mon("mac-2", 1512, 3440)];
        let theirs = [mon("pc-1", 0, 3440), mon("pc-2", 3440, 1920)];
        for state in [
            LinkState::Standby,
            LinkState::Off,
            LinkState::ActiveInputUnplugged(Machine::A),
        ] {
            let links = Links::from([("m34".to_owned(), link(state))]);
            let arr = arrangement(&desk, &both_on(), Machine::A, &mine, &theirs, &links);
            assert_eq!(arr.a, mine[..1], "{state:?}");
            assert_eq!(arr.b, theirs[1..], "{state:?}");
        }
        let links = Links::from([("m34".to_owned(), link(LinkState::Unknown))]);
        let arr = arrangement(&desk, &both_on(), Machine::A, &mine, &theirs, &links);
        assert_eq!((arr.a.len(), arr.b.len()), (2, 2));
    }

    proptest! {
        /// What travels comes back the same (with the scale in thousandths).
        #[test]
        fn wire_round_trip(
            x in any::<i32>(), y in any::<i32>(),
            w in 1u32.., h in 1u32..,
            milli in 1u32..10_000,
            id in "[a-z0-9-]{0,10}",
        ) {
            let r = ScreenRect { id, x, y, width: w, height: h, scale_milli: milli };
            prop_assert_eq!(to_wire(&from_wire(&r)), r);
        }
    }
}
