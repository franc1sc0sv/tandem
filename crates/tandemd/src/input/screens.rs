//! Las pantallas de cada máquina como las ve el motor de entrada, y cómo viajan al
//! otro agente ([`ScreenRect`]).

use tandem_core::{Config, Machine, State};
use tandem_display::topology::PlacedScreen;
use tandem_input::geometry::{GeometryError, Layout, LayoutOptions, Rect, Screen, ScreenId};
use tandem_proto::ScreenRect;

/// Una pantalla del escritorio, con escala 1: en macOS el cursor ya va en puntos, y en
/// Windows la topología no da los DPI de cada monitor (queda para la verificación en
/// el puesto). `None` si no tiene área.
pub fn from_placed(p: PlacedScreen) -> Option<Screen> {
    let width = u32::try_from(p.bounds.width).ok().filter(|w| *w > 0)?;
    let height = u32::try_from(p.bounds.height).ok().filter(|h| *h > 0)?;
    let id = match p.monitor {
        Some(m) => ScreenId::Monitor(m),
        None => ScreenId::Builtin,
    };
    Some(Screen::new(
        id,
        Rect::new(p.bounds.x, p.bounds.y, width, height),
        1.0,
    ))
}

pub fn to_wire(s: &Screen) -> ScreenRect {
    ScreenRect {
        monitor: match &s.id {
            ScreenId::Monitor(m) => Some(m.clone()),
            ScreenId::Builtin => None,
        },
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
        match &r.monitor {
            Some(m) => ScreenId::Monitor(m.clone()),
            None => ScreenId::Builtin,
        },
        Rect::new(r.x, r.y, r.width, r.height),
        f64::from(r.scale_milli) / 1000.0,
    )
}

/// La geometría con las pantallas de esta máquina (`mine`) y las del otro agente.
pub fn layout(
    desk: &Config,
    state: &State,
    me: Machine,
    mine: &[Screen],
    theirs: &[Screen],
    opts: LayoutOptions,
) -> Result<Layout, GeometryError> {
    let (mac, pc) = match me {
        Machine::Mac => (mine, theirs),
        Machine::Pc => (theirs, mine),
    };
    Layout::compute(desk, state, mac, pc, opts)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tandem_display::topology::Bounds;

    use super::*;

    fn desk() -> Config {
        Config::from_toml(
            r#"
            [[monitors]]
            id = "m34"
            label = '34"'
            wiring = { pc = "displayport", mac = "hdmi" }
            "#,
        )
        .unwrap()
    }

    #[test]
    fn placed_screens_become_input_screens() {
        let s = from_placed(PlacedScreen {
            monitor: Some("m34".into()),
            bounds: Bounds {
                x: -3440,
                y: 10,
                width: 3440,
                height: 1440,
            },
        })
        .unwrap();
        assert_eq!(s.id, ScreenId::Monitor("m34".into()));
        assert_eq!(s.rect, Rect::new(-3440, 10, 3440, 1440));
        assert_eq!(s.scale, 1.0);
        let builtin = from_placed(PlacedScreen {
            monitor: None,
            bounds: Bounds {
                x: 0,
                y: 0,
                width: 1512,
                height: 982,
            },
        })
        .unwrap();
        assert_eq!(builtin.id, ScreenId::Builtin);
        for (w, h) in [(0, 10), (10, 0), (-5, 10)] {
            assert_eq!(
                from_placed(PlacedScreen {
                    monitor: None,
                    bounds: Bounds {
                        x: 0,
                        y: 0,
                        width: w,
                        height: h
                    },
                }),
                None
            );
        }
    }

    #[test]
    fn each_machine_puts_its_screens_on_its_side() {
        let m34 = Screen::new(
            ScreenId::Monitor("m34".into()),
            Rect::new(0, 0, 3440, 1440),
            1.0,
        );
        let mut state = State::initial(&desk(), Machine::Mac);
        state.on.pc = true;
        state.owner.insert("m34".into(), Machine::Pc);
        let opts = LayoutOptions::default();
        let pc = layout(
            &desk(),
            &state,
            Machine::Pc,
            std::slice::from_ref(&m34),
            &[],
            opts,
        )
        .unwrap();
        let mac = layout(&desk(), &state, Machine::Mac, &[], &[m34], opts).unwrap();
        assert_eq!(pc, mac);
        assert_eq!(pc.screens(Machine::Pc).len(), 1);
        assert!(pc.screens(Machine::Mac).is_empty());
    }

    proptest! {
        /// Lo que viaja vuelve igual (con la escala en milésimas).
        #[test]
        fn wire_round_trip(
            x in any::<i32>(), y in any::<i32>(),
            w in 1u32.., h in 1u32..,
            milli in 1u32..10_000,
            builtin in any::<bool>(),
        ) {
            let r = ScreenRect {
                monitor: (!builtin).then(|| "m34".into()),
                x, y, width: w, height: h, scale_milli: milli,
            };
            prop_assert_eq!(to_wire(&from_wire(&r)), r);
        }
    }
}
