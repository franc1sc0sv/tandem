//! Reasignación de modificadores, configurable por dirección.
//!
//! Por defecto Mac→Win: Cmd→Ctrl, Ctrl→Win, Option→Alt, Option derecha→AltGr;
//! y al revés para Win→Mac (docs/research/input-engine.md). Se aplica en el
//! servidor antes de enviar, así el cliente inyecta lo que recibe tal cual.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{InputEvent, ModifierKey, Modifiers};

/// Sentido en que viaja la entrada: de la máquina que captura a la que inyecta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    MacToWin,
    WinToMac,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RemapError {
    /// Dos modificadores irían a la misma tecla: soltar uno soltaría el otro en el
    /// destino mientras sigue presionado en el origen.
    #[error("{first:?} y {second:?} se reasignan los dos a {target:?}")]
    NotBijective {
        first: ModifierKey,
        second: ModifierKey,
        target: ModifierKey,
    },
}

/// Destino de cada modificador del origen. Siempre es una permutación de los ocho.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RemapTable", into = "RemapTable")]
pub struct ModifierRemap {
    /// Indexado por `ModifierKey::bit()` del origen.
    targets: [ModifierKey; 8],
}

/// Forma de la configuración: un campo por modificador del origen.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemapTable {
    control_left: ModifierKey,
    shift_left: ModifierKey,
    alt_left: ModifierKey,
    meta_left: ModifierKey,
    control_right: ModifierKey,
    shift_right: ModifierKey,
    alt_right: ModifierKey,
    meta_right: ModifierKey,
}

impl TryFrom<RemapTable> for ModifierRemap {
    type Error = RemapError;

    fn try_from(t: RemapTable) -> Result<Self, RemapError> {
        ModifierRemap::new([
            t.control_left,
            t.shift_left,
            t.alt_left,
            t.meta_left,
            t.control_right,
            t.shift_right,
            t.alt_right,
            t.meta_right,
        ])
    }
}

impl From<ModifierRemap> for RemapTable {
    fn from(r: ModifierRemap) -> Self {
        let [
            control_left,
            shift_left,
            alt_left,
            meta_left,
            control_right,
            shift_right,
            alt_right,
            meta_right,
        ] = r.targets;
        RemapTable {
            control_left,
            shift_left,
            alt_left,
            meta_left,
            control_right,
            shift_right,
            alt_right,
            meta_right,
        }
    }
}

impl ModifierRemap {
    /// `targets[i]` es el destino de `ModifierKey::ALL[i]`.
    pub fn new(targets: [ModifierKey; 8]) -> Result<Self, RemapError> {
        let mut seen: [Option<ModifierKey>; 8] = [None; 8];
        for (source, target) in ModifierKey::ALL.into_iter().zip(targets) {
            let slot = &mut seen[usize::from(target.bit())];
            if let Some(first) = *slot {
                return Err(RemapError::NotBijective {
                    first,
                    second: source,
                    target,
                });
            }
            *slot = Some(source);
        }
        Ok(Self { targets })
    }

    pub const fn identity() -> Self {
        Self {
            targets: ModifierKey::ALL,
        }
    }

    /// Mac→Win: Cmd→Ctrl, Ctrl→Win, Option→Alt, Option derecha→AltGr.
    pub const fn mac_to_win() -> Self {
        use ModifierKey::*;
        Self {
            targets: [
                MetaLeft,     // Ctrl izq. → Win izq.
                ShiftLeft,    //
                AltLeft,      // Option izq. → Alt
                ControlLeft,  // Cmd izq. → Ctrl izq.
                MetaRight,    // Ctrl der. → Win der.
                ShiftRight,   //
                AltRight,     // Option der. → AltGr
                ControlRight, // Cmd der. → Ctrl der.
            ],
        }
    }

    /// Win→Mac: la inversa de [`ModifierRemap::mac_to_win`].
    pub const fn win_to_mac() -> Self {
        Self::mac_to_win().inverse()
    }

    pub const fn inverse(self) -> Self {
        let mut targets = ModifierKey::ALL;
        let mut i = 0;
        while i < 8 {
            targets[self.targets[i].bit() as usize] = ModifierKey::ALL[i];
            i += 1;
        }
        Self { targets }
    }

    pub const fn map(&self, key: ModifierKey) -> ModifierKey {
        self.targets[key.bit() as usize]
    }

    pub fn map_modifiers(&self, mods: Modifiers) -> Modifiers {
        mods.iter().map(|k| self.map(k)).collect()
    }

    /// Reasigna un evento; lo que no es un modificador pasa igual.
    pub fn apply(&self, event: InputEvent) -> InputEvent {
        match event {
            InputEvent::Key { usage, state } => match usage.modifier() {
                Some(m) => InputEvent::Key {
                    usage: self.map(m).usage(),
                    state,
                },
                None => event,
            },
            InputEvent::Modifiers(m) => InputEvent::Modifiers(self.map_modifiers(m)),
            other => other,
        }
    }
}

impl Default for ModifierRemap {
    fn default() -> Self {
        Self::identity()
    }
}

/// La política completa: una tabla por dirección.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemapPolicy {
    pub mac_to_win: ModifierRemap,
    pub win_to_mac: ModifierRemap,
}

impl Default for RemapPolicy {
    fn default() -> Self {
        Self {
            mac_to_win: ModifierRemap::mac_to_win(),
            win_to_mac: ModifierRemap::win_to_mac(),
        }
    }
}

impl RemapPolicy {
    pub fn for_direction(&self, direction: Direction) -> &ModifierRemap {
        match direction {
            Direction::MacToWin => &self.mac_to_win,
            Direction::WinToMac => &self.win_to_mac,
        }
    }

    pub fn apply(&self, direction: Direction, event: InputEvent) -> InputEvent {
        self.for_direction(direction).apply(event)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Button, HidUsage, KeyState, Scroll};
    use super::*;
    use ModifierKey::*;
    use proptest::prelude::*;

    fn key(m: ModifierKey) -> InputEvent {
        InputEvent::Key {
            usage: m.usage(),
            state: KeyState::Pressed,
        }
    }

    #[test]
    fn por_defecto_mac_a_win() {
        let p = RemapPolicy::default();
        let d = Direction::MacToWin;
        assert_eq!(p.apply(d, key(MetaLeft)), key(ControlLeft), "Cmd→Ctrl");
        assert_eq!(
            p.apply(d, key(MetaRight)),
            key(ControlRight),
            "Cmd der.→Ctrl der."
        );
        assert_eq!(p.apply(d, key(ControlLeft)), key(MetaLeft), "Ctrl→Win");
        assert_eq!(p.apply(d, key(ControlRight)), key(MetaRight));
        assert_eq!(p.apply(d, key(AltLeft)), key(AltLeft), "Option→Alt");
        assert_eq!(
            p.apply(d, key(AltRight)),
            key(AltRight),
            "Option der.→AltGr"
        );
        assert_eq!(p.apply(d, key(ShiftLeft)), key(ShiftLeft));
        assert_eq!(p.apply(d, key(ShiftRight)), key(ShiftRight));
    }

    #[test]
    fn por_defecto_win_a_mac() {
        let p = RemapPolicy::default();
        let d = Direction::WinToMac;
        assert_eq!(p.apply(d, key(ControlLeft)), key(MetaLeft), "Ctrl→Cmd");
        assert_eq!(p.apply(d, key(ControlRight)), key(MetaRight));
        assert_eq!(p.apply(d, key(MetaLeft)), key(ControlLeft), "Win→Ctrl");
        assert_eq!(p.apply(d, key(MetaRight)), key(ControlRight));
        assert_eq!(p.apply(d, key(AltLeft)), key(AltLeft), "Alt→Option");
        assert_eq!(
            p.apply(d, key(AltRight)),
            key(AltRight),
            "AltGr→Option der."
        );
    }

    #[test]
    fn copiar_en_mac_es_ctrl_c_en_windows() {
        let p = RemapPolicy::default();
        let c = InputEvent::Key {
            usage: HidUsage::key(0x06),
            state: KeyState::Pressed,
        };
        let cmd_c = Modifiers::from_iter([MetaLeft]);
        assert_eq!(p.apply(Direction::MacToWin, c), c);
        assert_eq!(
            p.apply(Direction::MacToWin, InputEvent::Modifiers(cmd_c)),
            InputEvent::Modifiers(Modifiers::from_iter([ControlLeft]))
        );
    }

    #[test]
    fn otros_eventos_pasan_igual() {
        let p = RemapPolicy::default();
        for e in [
            InputEvent::Button {
                button: Button::Left,
                state: KeyState::Released,
            },
            InputEvent::MotionRelative { dx: 1.5, dy: -2.0 },
            InputEvent::MotionAbsolute { x: 10.0, y: 20.0 },
            InputEvent::Scroll(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: 120,
            }),
            InputEvent::Key {
                usage: HidUsage::consumer(0xE0),
                state: KeyState::Pressed,
            },
        ] {
            assert_eq!(p.apply(Direction::MacToWin, e), e);
            assert_eq!(p.apply(Direction::WinToMac, e), e);
        }
    }

    #[test]
    fn rechaza_tablas_no_biyectivas() {
        let mut t = ModifierKey::ALL;
        t[0] = MetaLeft; // Ctrl izq. y Cmd izq. → Cmd izq.
        assert_eq!(
            ModifierRemap::new(t),
            Err(RemapError::NotBijective {
                first: ControlLeft,
                second: MetaLeft,
                target: MetaLeft,
            })
        );
    }

    #[test]
    fn identidad_y_default() {
        let id = ModifierRemap::default();
        assert_eq!(id, ModifierRemap::identity());
        for k in ModifierKey::ALL {
            assert_eq!(id.map(k), k);
        }
    }

    #[test]
    fn configuracion_toml_por_direccion() {
        let toml = r#"
            [mac_to_win]
            control_left = "control_left"
            shift_left = "shift_left"
            alt_left = "alt_left"
            meta_left = "meta_left"
            control_right = "control_right"
            shift_right = "shift_right"
            alt_right = "alt_right"
            meta_right = "meta_right"
        "#;
        let p: RemapPolicy = toml::from_str(toml).unwrap();
        assert_eq!(p.mac_to_win, ModifierRemap::identity());
        assert_eq!(
            p.win_to_mac,
            ModifierRemap::win_to_mac(),
            "lo omitido queda por defecto"
        );

        let s = toml::to_string(&RemapPolicy::default()).unwrap();
        assert!(s.contains("meta_left = \"control_left\""), "{s}");
        assert_eq!(
            toml::from_str::<RemapPolicy>(&s).unwrap(),
            RemapPolicy::default()
        );
    }

    #[test]
    fn configuracion_invalida_se_rechaza() {
        let toml = r#"
            [win_to_mac]
            control_left = "meta_left"
            shift_left = "shift_left"
            alt_left = "alt_left"
            meta_left = "meta_left"
            control_right = "control_right"
            shift_right = "shift_right"
            alt_right = "alt_right"
            meta_right = "meta_right"
        "#;
        let err = toml::from_str::<RemapPolicy>(toml).unwrap_err().to_string();
        assert!(err.contains("se reasignan los dos"), "{err}");
    }

    fn any_remap() -> impl Strategy<Value = ModifierRemap> {
        Just(ModifierKey::ALL.to_vec())
            .prop_shuffle()
            .prop_map(|v| {
                let mut t = ModifierKey::ALL;
                t.copy_from_slice(&v);
                ModifierRemap::new(t).unwrap()
            })
    }

    proptest! {
        #[test]
        fn inversa_deshace(r in any_remap(), bits in any::<u8>()) {
            let m = Modifiers::from_bits(bits);
            prop_assert_eq!(r.inverse().map_modifiers(r.map_modifiers(m)), m);
            prop_assert_eq!(r.inverse().inverse(), r);
        }

        #[test]
        fn conserva_la_cantidad_de_modificadores(r in any_remap(), bits in any::<u8>()) {
            let m = Modifiers::from_bits(bits);
            prop_assert_eq!(r.map_modifiers(m).bits().count_ones(), bits.count_ones());
        }

        #[test]
        fn modificadores_coinciden_con_teclas(r in any_remap(), bits in any::<u8>()) {
            // Reasignar el conjunto = reasignar cada tecla presionada.
            let m = Modifiers::from_bits(bits);
            let por_tecla: Modifiers = m
                .iter()
                .filter_map(|k| match r.apply(key(k)) {
                    InputEvent::Key { usage, .. } => usage.modifier(),
                    _ => None,
                })
                .collect();
            prop_assert_eq!(r.map_modifiers(m), por_tecla);
        }
    }

    #[test]
    fn win_a_mac_deshace_mac_a_win() {
        let p = RemapPolicy::default();
        for k in ModifierKey::ALL {
            assert_eq!(p.win_to_mac.map(p.mac_to_win.map(k)), k);
        }
    }
}
