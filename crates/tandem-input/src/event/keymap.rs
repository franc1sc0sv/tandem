//! Tablas HID ↔ keycode de Mac ↔ scancode de Windows.
//!
//! Los datos salen del crate `keycode` (MIT, tabla de Chromium). Las búsquedas
//! inversas del crate dependen del orden de un `HashSet` en su macro y no
//! distinguen la página HID, así que aquí se construyen índices propios
//! recorriendo `KeyMap::from_usb_code`, que sí es exacto.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::{HidUsage, PAGE_CONSUMER, PAGE_GENERIC_DESKTOP, PAGE_KEYBOARD};

/// Keycode virtual de macOS (`kVK_*`, el `CGKeyCode` de los eventos).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MacKeycode(pub u16);

/// Scancode de Windows (Set 1). Las teclas extendidas llevan `0xE0` en el byte alto,
/// como en la tabla de Chromium: `0xE048` es la flecha arriba.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WinScancode(pub u16);

impl WinScancode {
    /// Lleva prefijo E0: `SendInput` necesita `KEYEVENTF_EXTENDEDKEY`.
    pub const fn is_extended(self) -> bool {
        self.0 >> 8 == 0xE0
    }

    /// El byte que va en `wScan`.
    pub const fn code(self) -> u8 {
        (self.0 & 0xFF) as u8
    }

    /// Reconstruye el scancode a partir de lo que entrega el hook de Windows.
    pub const fn from_parts(code: u8, extended: bool) -> Self {
        if extended {
            WinScancode(0xE000 | code as u16)
        } else {
            WinScancode(code as u16)
        }
    }
}

/// Sin equivalente en Windows en la tabla de Chromium.
const WIN_INVALID: u16 = 0x0000;
/// Sin equivalente en macOS en la tabla de Chromium.
const MAC_INVALID: u16 = 0xFFFF;

/// Teclas que llegan a un sistema con un código que no tiene en la tabla.
///
/// - IntlHash (ISO `#`/`ç`): ni Windows ni macOS la distinguen de Backslash; las dos
///   reportan el código de Backslash para esa tecla física.
/// - Volumen en la página de consumo (lo que manda un teclado real): Chromium solo
///   lo tiene en la página de teclado (0x7F–0x81), que es lo que devuelve la captura.
const ALIAS_ANY: &[(HidUsage, HidUsage)] = &[
    (HidUsage::key(0x32), HidUsage::key(0x31)),
    (HidUsage::consumer(0xE2), HidUsage::key(0x7F)),
    (HidUsage::consumer(0xE9), HidUsage::key(0x80)),
    (HidUsage::consumer(0xEA), HidUsage::key(0x81)),
];

/// Solo hacia Mac: macOS trata PrintScreen, ScrollLock y Pause de un teclado de PC
/// como F13, F14 y F15. Por eso la vuelta Mac→HID devuelve F13–F15, no la original.
const ALIAS_MAC: &[(HidUsage, HidUsage)] = &[
    (HidUsage::key(0x46), HidUsage::key(0x68)),
    (HidUsage::key(0x47), HidUsage::key(0x69)),
    (HidUsage::key(0x48), HidUsage::key(0x6A)),
];

struct Tables {
    to_mac: BTreeMap<HidUsage, MacKeycode>,
    to_win: BTreeMap<HidUsage, WinScancode>,
    from_mac: HashMap<MacKeycode, HidUsage>,
    from_win: HashMap<WinScancode, HidUsage>,
}

/// Rango de usos que se recorre en cada página. La de consumo llega a 0x29F.
const PAGES: [(u16, u16); 3] = [
    (PAGE_GENERIC_DESKTOP, 0xFF),
    // 0x00–0x03 son códigos de error y reservados, no teclas.
    (PAGE_KEYBOARD, 0xFF),
    (PAGE_CONSUMER, 0x3FF),
];

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut t = Tables {
            to_mac: BTreeMap::new(),
            to_win: BTreeMap::new(),
            from_mac: HashMap::new(),
            from_win: HashMap::new(),
        };
        for (page, max) in PAGES {
            let first = if page == PAGE_KEYBOARD { 0x04 } else { 0x01 };
            // En orden ascendente: si un código se repitiera, gana el uso más bajo.
            for id in first..=max {
                let Ok(km) = keycode::KeyMap::from_usb_code(page, id) else {
                    continue;
                };
                let usage = HidUsage::new(page, id);
                if km.mac != MAC_INVALID {
                    let mac = MacKeycode(km.mac);
                    t.to_mac.insert(usage, mac);
                    t.from_mac.entry(mac).or_insert(usage);
                }
                if km.win != WIN_INVALID {
                    let win = WinScancode(km.win);
                    t.to_win.insert(usage, win);
                    t.from_win.entry(win).or_insert(usage);
                }
            }
        }
        t
    })
}

fn alias(usage: HidUsage, table: &[(HidUsage, HidUsage)]) -> Option<HidUsage> {
    table
        .iter()
        .find(|(from, _)| *from == usage)
        .map(|(_, to)| *to)
}

/// Keycode de Mac para inyectar esta tecla, si macOS tiene uno.
pub fn to_mac(usage: HidUsage) -> Option<MacKeycode> {
    let t = tables();
    t.to_mac.get(&usage).copied().or_else(|| {
        let target = alias(usage, ALIAS_ANY).or_else(|| alias(usage, ALIAS_MAC))?;
        t.to_mac.get(&target).copied()
    })
}

/// Scancode de Windows para inyectar esta tecla, si Windows tiene uno.
pub fn to_win(usage: HidUsage) -> Option<WinScancode> {
    let t = tables();
    t.to_win
        .get(&usage)
        .copied()
        .or_else(|| t.to_win.get(&alias(usage, ALIAS_ANY)?).copied())
}

/// Tecla física de un keycode capturado en Mac.
pub fn from_mac(code: MacKeycode) -> Option<HidUsage> {
    tables().from_mac.get(&code).copied()
}

/// Tecla física de un scancode capturado en Windows.
pub fn from_win(code: WinScancode) -> Option<HidUsage> {
    tables().from_win.get(&code).copied()
}

/// La tecla con este nombre de `KeyboardEvent.code` (`ScrollLock`, `KeyL`,
/// `ControlLeft`), para las teclas de la configuración. Distingue mayúsculas.
pub fn from_code_name(name: &str) -> Option<HidUsage> {
    let code: keycode::KeyMappingCode = name.parse().ok()?;
    PAGES.iter().find_map(|&(page, max)| {
        (0..=max).find_map(|id| {
            let km = keycode::KeyMap::from_usb_code(page, id).ok()?;
            (km.code == Some(code)).then_some(HidUsage::new(page, id))
        })
    })
}

/// Todas las teclas con código directo en Mac, en orden HID.
pub fn mac_usages() -> impl Iterator<Item = (HidUsage, MacKeycode)> {
    tables().to_mac.iter().map(|(u, c)| (*u, *c))
}

/// Todas las teclas con código directo en Windows, en orden HID.
pub fn win_usages() -> impl Iterator<Item = (HidUsage, WinScancode)> {
    tables().to_win.iter().map(|(u, c)| (*u, *c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn teclas_por_su_nombre_de_code() {
        assert_eq!(from_code_name("ScrollLock"), Some(HidUsage::key(0x47)));
        assert_eq!(from_code_name("KeyL"), Some(HidUsage::key(0x0F)));
        assert_eq!(from_code_name("ControlLeft"), Some(HidUsage::key(0xE0)));
        assert_eq!(from_code_name("F14"), Some(HidUsage::key(0x69)));
        assert_eq!(from_code_name("scrolllock"), None);
        assert_eq!(from_code_name(""), None);
        assert_eq!(from_code_name("NoExiste"), None);
    }

    /// Las 105 teclas de un teclado ISO español de PC, por código físico HID.
    /// La `ç` está en Backslash (0x31): así la reportan Windows y macOS.
    const ISO_ES: &[(u16, &str)] = &[
        (0x29, "Esc"),
        (0x3A, "F1"),
        (0x3B, "F2"),
        (0x3C, "F3"),
        (0x3D, "F4"),
        (0x3E, "F5"),
        (0x3F, "F6"),
        (0x40, "F7"),
        (0x41, "F8"),
        (0x42, "F9"),
        (0x43, "F10"),
        (0x44, "F11"),
        (0x45, "F12"),
        (0x46, "ImprPant"),
        (0x47, "BloqDespl"),
        (0x48, "Pausa"),
        (0x35, "º ª \\"),
        (0x1E, "1 ! |"),
        (0x1F, "2 \" @"),
        (0x20, "3 · #"),
        (0x21, "4 $ ~"),
        (0x22, "5 % €"),
        (0x23, "6 & ¬"),
        (0x24, "7 /"),
        (0x25, "8 ("),
        (0x26, "9 )"),
        (0x27, "0 ="),
        (0x2D, "' ?"),
        (0x2E, "¡ ¿"),
        (0x2A, "Retroceso"),
        (0x2B, "Tab"),
        (0x14, "Q"),
        (0x1A, "W"),
        (0x08, "E"),
        (0x15, "R"),
        (0x17, "T"),
        (0x1C, "Y"),
        (0x18, "U"),
        (0x0C, "I"),
        (0x12, "O"),
        (0x13, "P"),
        (0x2F, "` ^ ["),
        (0x30, "+ * ]"),
        (0x28, "Intro"),
        (0x39, "BloqMayús"),
        (0x04, "A"),
        (0x16, "S"),
        (0x07, "D"),
        (0x09, "F"),
        (0x0A, "G"),
        (0x0B, "H"),
        (0x0D, "J"),
        (0x0E, "K"),
        (0x0F, "L"),
        (0x33, "Ñ"),
        (0x34, "´ ¨ {"),
        (0x31, "Ç }"),
        (0xE1, "Mayús izq."),
        (0x64, "< > (ISO)"),
        (0x1D, "Z"),
        (0x1B, "X"),
        (0x06, "C"),
        (0x19, "V"),
        (0x05, "B"),
        (0x11, "N"),
        (0x10, "M"),
        (0x36, ", ;"),
        (0x37, ". :"),
        (0x38, "- _"),
        (0xE5, "Mayús der."),
        (0xE0, "Ctrl izq."),
        (0xE3, "Windows izq."),
        (0xE2, "Alt"),
        (0x2C, "Espacio"),
        (0xE6, "AltGr"),
        (0xE7, "Windows der."),
        (0x65, "Menú"),
        (0xE4, "Ctrl der."),
        (0x49, "Insert"),
        (0x4A, "Inicio"),
        (0x4B, "RePág"),
        (0x4C, "Supr"),
        (0x4D, "Fin"),
        (0x4E, "AvPág"),
        (0x52, "Arriba"),
        (0x50, "Izquierda"),
        (0x51, "Abajo"),
        (0x4F, "Derecha"),
        (0x53, "BloqNum"),
        (0x54, "Num /"),
        (0x55, "Num *"),
        (0x56, "Num -"),
        (0x57, "Num +"),
        (0x58, "Num Intro"),
        (0x63, "Num ."),
        (0x62, "Num 0"),
        (0x59, "Num 1"),
        (0x5A, "Num 2"),
        (0x5B, "Num 3"),
        (0x5C, "Num 4"),
        (0x5D, "Num 5"),
        (0x5E, "Num 6"),
        (0x5F, "Num 7"),
        (0x60, "Num 8"),
        (0x61, "Num 9"),
    ];

    fn iso_es() -> impl Iterator<Item = (HidUsage, &'static str)> {
        ISO_ES.iter().map(|(id, name)| (HidUsage::key(*id), *name))
    }

    #[test]
    fn iso_es_tiene_105_teclas_distintas() {
        let mut ids: Vec<u16> = ISO_ES.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 105);
        assert_eq!(ISO_ES.len(), 105);
    }

    #[test]
    fn iso_es_ida_y_vuelta_hid_win_hid() {
        for (usage, name) in iso_es() {
            let win = to_win(usage).unwrap_or_else(|| panic!("{name}: sin scancode"));
            assert_eq!(from_win(win), Some(usage), "{name}: {win:?}");
        }
    }

    #[test]
    fn iso_es_ida_y_vuelta_hid_mac_hid() {
        // ImprPant, BloqDespl y Pausa llegan a la Mac como F13–F15 (ALIAS_MAC).
        let aliased = [0x46, 0x47, 0x48];
        for (usage, name) in iso_es() {
            let mac = to_mac(usage).unwrap_or_else(|| panic!("{name}: sin keycode"));
            let back = from_mac(mac).unwrap_or_else(|| panic!("{name}: {mac:?} no vuelve"));
            if aliased.contains(&usage.id) {
                assert_eq!(back, HidUsage::key(usage.id + 0x22), "{name}");
                assert_eq!(to_mac(back), Some(mac), "{name}: alias estable");
            } else {
                assert_eq!(back, usage, "{name}: {mac:?}");
            }
        }
    }

    #[test]
    fn iso_es_win_mac_win_pasando_por_hid() {
        for (usage, name) in iso_es().filter(|(u, _)| !(0x46..=0x48).contains(&u.id)) {
            let Some(win) = to_win(usage) else {
                panic!("{name}: sin scancode")
            };
            let mac = from_win(win).and_then(to_mac);
            let win_back = mac.and_then(from_mac).and_then(to_win);
            assert_eq!(win_back, Some(win), "{name}: {win:?} → {mac:?}");
        }
    }

    #[test]
    fn valores_conocidos() {
        // Comprobados contra la tabla de Chromium y la documentación de cada sistema.
        let a = HidUsage::key(0x04);
        assert_eq!(to_mac(a), Some(MacKeycode(0x00)));
        assert_eq!(to_win(a), Some(WinScancode(0x1E)));
        let up = HidUsage::key(0x52);
        assert_eq!(to_win(up), Some(WinScancode(0xE048)));
        assert_eq!(to_mac(up), Some(MacKeycode(0x7E)));
        let cmd = HidUsage::key(0xE3);
        assert_eq!(to_mac(cmd), Some(MacKeycode(0x37)));
        assert_eq!(to_win(cmd), Some(WinScancode(0xE05B)));
        let altgr = HidUsage::key(0xE6);
        assert_eq!(to_win(altgr), Some(WinScancode(0xE038)));
        // `<` ISO: kVK_ISO_Section en Mac (ojo con el intercambio §/` del teclado de Apple).
        assert_eq!(to_mac(HidUsage::key(0x64)), Some(MacKeycode(0x0A)));
        assert_eq!(to_mac(HidUsage::key(0x35)), Some(MacKeycode(0x32)));
    }

    #[test]
    fn intl_hash_se_inyecta_como_backslash() {
        let hash = HidUsage::key(0x32);
        let backslash = HidUsage::key(0x31);
        assert_eq!(to_win(hash), to_win(backslash));
        assert_eq!(to_mac(hash), to_mac(backslash));
        assert!(to_win(hash).is_some());
    }

    #[test]
    fn multimedia_en_pagina_de_consumo() {
        let play = HidUsage::consumer(0xCD);
        assert_eq!(to_win(play), Some(WinScancode(0xE022)));
        assert_eq!(from_win(WinScancode(0xE022)), Some(play));
        // La página 0x0C no se confunde con la 0x07 aunque el uso coincida.
        assert_ne!(to_win(HidUsage::key(0xCD)), to_win(play));
    }

    #[test]
    fn volumen_de_consumo_se_inyecta_como_el_de_teclado() {
        for (consumer, key) in [(0xE2, 0x7F), (0xE9, 0x80), (0xEA, 0x81)] {
            let c = HidUsage::consumer(consumer);
            let k = HidUsage::key(key);
            assert!(to_win(c).is_some() && to_mac(c).is_some());
            assert_eq!(to_win(c), to_win(k));
            assert_eq!(to_mac(c), to_mac(k));
        }
        assert_eq!(to_win(HidUsage::consumer(0xE2)), Some(WinScancode(0xE020)));
        assert_eq!(from_win(WinScancode(0xE020)), Some(HidUsage::key(0x7F)));
    }

    #[test]
    fn codigos_invalidos_no_se_traducen() {
        assert_eq!(to_win(HidUsage::key(0x00)), None);
        assert_eq!(to_win(HidUsage::key(0x01)), None);
        assert_eq!(to_mac(HidUsage::key(0x47 + 0x100)), None);
        assert_eq!(from_mac(MacKeycode(MAC_INVALID)), None);
        assert_eq!(from_win(WinScancode(WIN_INVALID)), None);
        assert_eq!(to_win(HidUsage::new(0x00, 0x12)), None, "Fn no es HID real");
    }

    #[test]
    fn scancode_extendido() {
        let s = WinScancode(0xE048);
        assert!(s.is_extended());
        assert_eq!(s.code(), 0x48);
        assert_eq!(WinScancode::from_parts(0x48, true), s);
        let a = WinScancode(0x1E);
        assert!(!a.is_extended());
        assert_eq!(WinScancode::from_parts(0x1E, false), a);
    }

    #[test]
    fn tablas_inversas_son_biyectivas() {
        assert_eq!(mac_usages().count(), tables().from_mac.len());
        assert_eq!(win_usages().count(), tables().from_win.len());
        assert!(mac_usages().count() > 100);
        assert!(win_usages().count() > 150);
    }

    fn any_mac_entry() -> impl Strategy<Value = (HidUsage, MacKeycode)> {
        let all: Vec<_> = mac_usages().collect();
        proptest::sample::select(all)
    }

    fn any_win_entry() -> impl Strategy<Value = (HidUsage, WinScancode)> {
        let all: Vec<_> = win_usages().collect();
        proptest::sample::select(all)
    }

    proptest! {
        #[test]
        fn ida_y_vuelta_mac((usage, mac) in any_mac_entry()) {
            prop_assert_eq!(to_mac(usage), Some(mac));
            prop_assert_eq!(from_mac(mac), Some(usage));
        }

        #[test]
        fn ida_y_vuelta_win((usage, win) in any_win_entry()) {
            prop_assert_eq!(to_win(usage), Some(win));
            prop_assert_eq!(from_win(win), Some(usage));
            prop_assert_eq!(WinScancode::from_parts(win.code(), win.is_extended()), win);
        }

        #[test]
        fn cualquier_scancode_capturado_vuelve_igual(raw in any::<u16>()) {
            if let Some(usage) = from_win(WinScancode(raw)) {
                prop_assert_eq!(to_win(usage), Some(WinScancode(raw)));
            }
        }

        #[test]
        fn cualquier_keycode_capturado_vuelve_igual(raw in any::<u16>()) {
            if let Some(usage) = from_mac(MacKeycode(raw)) {
                prop_assert_eq!(to_mac(usage), Some(MacKeycode(raw)));
            }
        }
    }
}
