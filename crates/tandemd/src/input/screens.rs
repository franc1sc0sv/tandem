//! Las pantallas de cada máquina como las ve el motor de entrada, cómo viajan al otro
//! agente ([`ScreenRect`]) y el acomodo que sale de ellas y del puesto (DESIGN §7).
//!
//! Por máquina valen sus `[[screens]]` declaradas en el puesto; sin declarar, las que lee
//! ella misma (los límites que usa la inyección). Una máquina apagada no tiene pantallas:
//! el cursor no cruza hacia ella.

use tandem_core::{Config, Machine, ScreenSpec, State};
use tandem_input::geometry::{Arrangement, GeometryError, Layout, LayoutOptions, Rect, Screen};
use tandem_proto::ScreenRect;

pub fn to_wire(s: &Screen) -> ScreenRect {
    ScreenRect {
        id: s.id.0.clone(),
        x: s.rect.x,
        y: s.rect.y,
        width: s.rect.width,
        height: s.rect.height,
        // Escalas absurdas no llegan: `Layout::compute` las rechaza antes.
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

/// Una pantalla declarada en el puesto.
pub fn from_spec(s: &ScreenSpec) -> Screen {
    Screen::new(
        s.id.as_str().into(),
        Rect::new(s.x, s.y, s.width, s.height),
        s.scale.as_f64(),
    )
}

/// Las que lee una máquina, con un id por pantalla (`mac-1`, `pc-2`...).
pub fn detected(machine: Machine, bounds: &[Rect]) -> Vec<Screen> {
    bounds
        .iter()
        .filter(|r| r.width > 0 && r.height > 0)
        .enumerate()
        .map(|(i, r)| Screen::new(format!("{machine}-{}", i + 1).as_str().into(), *r, 1.0))
        .collect()
}

/// El acomodo: las pantallas de esta máquina (`me`) y las de la otra, declaradas en
/// `desk` o, si no, las leídas (`mine` aquí, `theirs` las que mandó el otro agente).
pub fn arrangement(
    desk: &Config,
    state: &State,
    me: Machine,
    mine: &[Screen],
    theirs: &[Screen],
) -> Arrangement {
    let of = |machine: Machine| -> Vec<Screen> {
        if !state.is_on(machine) {
            return Vec::new();
        }
        let declared: Vec<Screen> = desk.screens_of(machine).map(from_spec).collect();
        if !declared.is_empty() {
            return declared;
        }
        if machine == me {
            mine.to_vec()
        } else {
            theirs.to_vec()
        }
    };
    Arrangement {
        mac: of(Machine::Mac),
        pc: of(Machine::Pc),
        mac_side: desk.arrangement.mac_side,
    }
}

/// La geometría del acomodo.
pub fn layout(
    desk: &Config,
    state: &State,
    me: Machine,
    mine: &[Screen],
    theirs: &[Screen],
    opts: LayoutOptions,
) -> Result<Layout, GeometryError> {
    Layout::compute(&arrangement(desk, state, me, mine, theirs), opts)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tandem_core::{Powered, Scale, Side};

    use super::*;

    fn both_on() -> State {
        State {
            on: Powered::both(),
        }
    }

    fn mon(id: &str, x: i32, w: u32) -> Screen {
        Screen::new(id.into(), Rect::new(x, 0, w, 1440), 1.0)
    }

    #[test]
    fn detected_screens_get_one_id_each() {
        let got = detected(
            Machine::Mac,
            &[
                Rect::new(-3440, 0, 3440, 1440),
                Rect::new(0, 0, 0, 10),
                Rect::new(0, 0, 1512, 982),
            ],
        );
        let ids: Vec<_> = got.iter().map(|s| s.id.0.as_str()).collect();
        assert_eq!(ids, ["mac-1", "mac-2"]);
        assert_eq!(got[0].rect, Rect::new(-3440, 0, 3440, 1440));
        assert_eq!(got[1].scale, 1.0);
    }

    #[test]
    fn each_machine_puts_its_screens_on_its_side() {
        let desk = Config::default();
        let m34 = mon("pc-1", 0, 3440);
        let mut state = State::initial(Machine::Mac);
        state.on.pc = true;
        let opts = LayoutOptions::default();
        let pc = layout(
            &desk,
            &state,
            Machine::Pc,
            std::slice::from_ref(&m34),
            &[],
            opts,
        )
        .unwrap();
        let mac = layout(&desk, &state, Machine::Mac, &[], &[m34], opts).unwrap();
        assert_eq!(pc, mac);
        assert_eq!(pc.screens(Machine::Pc).len(), 1);
        assert!(pc.screens(Machine::Mac).is_empty());
    }

    #[test]
    fn declared_screens_replace_the_detected_ones_and_set_the_side() {
        let mut desk = Config {
            screens: vec![ScreenSpec {
                machine: Machine::Pc,
                id: "34".into(),
                x: 0,
                y: 0,
                width: 3440,
                height: 1440,
                scale: Scale::from_milli(1250),
            }],
            ..Config::default()
        };
        desk.arrangement.mac_side = Side::Right;
        let mine = [mon("mac-1", 0, 1512)];
        let theirs = [mon("pc-1", 0, 1920)];
        let arr = arrangement(&desk, &both_on(), Machine::Mac, &mine, &theirs);
        assert_eq!(arr.mac, mine);
        assert_eq!(arr.pc.len(), 1);
        assert_eq!(arr.pc[0].id, "34".into());
        assert_eq!(arr.pc[0].scale, 1.25);
        assert_eq!(arr.mac_side, Side::Right);
        // Sin declarar, la Mac va a la izquierda y valen las leídas.
        let arr = arrangement(&Config::default(), &both_on(), Machine::Mac, &mine, &theirs);
        assert_eq!((arr.mac, arr.pc), (mine.to_vec(), theirs.to_vec()));
        assert_eq!(arr.mac_side, Side::Left);
    }

    #[test]
    fn a_machine_that_is_off_has_no_screens() {
        let mine = [mon("mac-1", 0, 1512)];
        let theirs = [mon("pc-1", 0, 1920)];
        let arr = arrangement(
            &Config::default(),
            &State::initial(Machine::Mac),
            Machine::Mac,
            &mine,
            &theirs,
        );
        assert!(arr.pc.is_empty());
        assert_eq!(arr.mac, mine);
    }

    proptest! {
        /// Lo que viaja vuelve igual (con la escala en milésimas).
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
