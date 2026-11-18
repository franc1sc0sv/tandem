//! Las pantallas de cada máquina como las ve el motor de entrada, cómo viajan al otro
//! agente ([`ScreenRect`]) y el acomodo que sale de ellas y del puesto (DESIGN §7).
//!
//! Por máquina valen sus `[[screens]]` declaradas en el puesto; sin declarar, las que lee
//! ella misma (los límites que usa la inyección). Una máquina apagada no tiene pantallas:
//! el cursor no cruza hacia ella. Un monitor que muestra a la otra máquina deja de ser una
//! pantalla de esta (DESIGN §7, T-111): su pantalla sale del `[[cables]]` del puesto.

use std::collections::BTreeMap;

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

/// Las que lee una máquina, con un id por pantalla (`pantalla-1`, `pantalla-2`...). El id
/// solo tiene que ser único en su máquina, así que no depende del lado.
pub fn detected(bounds: &[Rect]) -> Vec<Screen> {
    bounds
        .iter()
        .filter(|r| r.width > 0 && r.height > 0)
        .enumerate()
        .map(|(i, r)| Screen::new(format!("pantalla-{}", i + 1).as_str().into(), *r, 1.0))
        .collect()
}

/// Qué máquina muestra cada monitor compatible, por id (`crate::monitors::shows`).
pub type Shows = BTreeMap<String, Machine>;

/// Las pantallas de `machine` que son monitores mostrando a la otra: según `shows` y la
/// pantalla de cada `[[cables]]`. Un monitor sin pantalla declarada no se puede sacar.
pub fn hidden(desk: &Config, machine: Machine, shows: &Shows) -> Vec<String> {
    desk.cables
        .iter()
        .filter(|c| shows.get(&c.monitor) == Some(&machine.other()))
        .filter_map(|c| c.end(machine).screen.clone())
        .collect()
}

/// El acomodo: las pantallas de esta máquina (`me`) y las de la otra, declaradas en
/// `desk` o, si no, las leídas (`mine` aquí, `theirs` las que mandó el otro agente), sin las
/// que muestran a la otra máquina ([`hidden`]), y el lado y la alineación del puesto (D18).
/// Sin acomodo declarado, A a la izquierda de B con todo el borde (el valor por omisión de
/// siempre).
pub fn arrangement(
    desk: &Config,
    state: &State,
    me: Machine,
    mine: &[Screen],
    theirs: &[Screen],
    shows: &Shows,
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
        let hidden = hidden(desk, machine, shows);
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

/// La geometría del acomodo.
pub fn layout(
    desk: &Config,
    state: &State,
    me: Machine,
    (mine, theirs): (&[Screen], &[Screen]),
    shows: &Shows,
    opts: LayoutOptions,
) -> Result<Layout, GeometryError> {
    Layout::compute(&arrangement(desk, state, me, mine, theirs, shows), opts)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tandem_core::{Align, Powered, Scale, Side};

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
        assert_eq!(ids, ["pantalla-1", "pantalla-2"]);
        assert_eq!(got[0].rect, Rect::new(-3440, 0, 3440, 1440));
        assert_eq!(got[1].scale, 1.0);
    }

    #[test]
    fn each_machine_puts_its_screens_on_its_side() {
        let desk = Config::default();
        let m34 = mon("pc-1", 0, 3440);
        let mut state = State::initial(Machine::A);
        state.on.b = true;
        let opts = LayoutOptions::default();
        let none = Shows::new();
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
        let arr = arrangement(&desk, &both_on(), Machine::A, &mine, &theirs, &Shows::new());
        assert_eq!(arr.a, mine);
        assert_eq!(arr.b.len(), 1);
        assert_eq!(arr.b[0].id, "34".into());
        assert_eq!(arr.b[0].scale, 1.25);
        assert_eq!((arr.a_side, arr.align), (Side::Right, Some(Align::End)));
        // Sin declarar, la Mac va a la izquierda, sin alineación, y valen las leídas.
        let arr = arrangement(
            &Config::default(),
            &both_on(),
            Machine::A,
            &mine,
            &theirs,
            &Shows::new(),
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
            &Shows::new(),
        );
        assert!(arr.b.is_empty());
        assert_eq!(arr.a, mine);
    }

    /// T-111: el 34" muestra a la Mac (A) → deja de ser una pantalla de la PC (B), que se queda
    /// sin pantallas; vuelve a la PC → sale de la Mac. Sin pantalla declarada no se toca nada.
    #[test]
    fn a_monitor_showing_the_other_machine_leaves_the_geometry() {
        let mut desk = Config::default();
        let mut cable = tandem_core::MonitorCable::new("m34");
        cable.a.screen = Some("mac-2".into());
        cable.b.screen = Some("pc-1".into());
        desk.cables.push(cable);
        let mine = [mon("mac-1", 0, 1512), mon("mac-2", 1512, 3440)];
        let theirs = [mon("pc-1", 0, 3440)];
        let shows = |m: Machine| Shows::from([("m34".to_owned(), m)]);
        let arr = arrangement(
            &desk,
            &both_on(),
            Machine::A,
            &mine,
            &theirs,
            &shows(Machine::A),
        );
        assert_eq!(arr.a, mine);
        assert!(arr.b.is_empty(), "la PC no tiene dónde mostrar el cursor");
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
        // Sin saber qué muestra, o sin pantalla declarada, quedan todas.
        let arr = arrangement(&desk, &both_on(), Machine::A, &mine, &theirs, &Shows::new());
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
