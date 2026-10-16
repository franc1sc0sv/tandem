//! Teclado: scancode del hook → tecla HID, y el Ctrl falso de AltGr.

use std::time::{Duration, Instant};

use super::KeyboardHook;
use super::consts::*;
use crate::event::keymap::{self, WinScancode};
use crate::event::{InputEvent, KeyState};

/// Scancode con el que Windows marca el Ctrl izquierdo que inventa al presionar
/// AltGr en distribuciones que la tienen (como la española).
const FAKE_CTRL_SCAN: u32 = 0x21D;
const SCAN_CONTROL: u32 = 0x1D;
const SCAN_ALT: u32 = 0x38;

/// Teclas que pueden llegar sin scancode (algunos teclados multimedia) o con uno
/// que la tabla no tiene: se resuelven por el código virtual.
fn scancode_from_vk(vk: u32) -> Option<WinScancode> {
    let code = match vk {
        // Ctrl+Pausa llega como VK_CANCEL con E0 46: es la tecla Pausa.
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

/// La tecla física de un evento del hook. `None` para lo que no es una tecla
/// real: los Shift falsos (E0 2A / E0 36) que Windows agrega alrededor de las
/// flechas con BloqNum, o códigos que no están en la tabla.
pub fn key_event(k: &KeyboardHook) -> Option<InputEvent> {
    let by_scan = u8::try_from(k.scan_code)
        .ok()
        .filter(|code| *code != 0)
        .and_then(|code| keymap::from_win(WinScancode::from_parts(code, k.is_extended())));
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

/// Quita el Ctrl izquierdo que Windows manda junto con AltGr.
///
/// Se reconoce por el scancode `0x21D` o, si no lo trae, porque llega justo antes
/// de Alt derecho con el mismo `time` y el mismo sentido. Para saberlo, cada Ctrl
/// izquierdo se retiene hasta el siguiente evento o hasta que pase la ventana.
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
    fn scancodes_del_hook_a_hid() {
        // A, Enter, Enter del teclado numérico (E0), flechas, Win.
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
        // AltGr es Alt derecho; Ctrl derecho lleva E0.
        assert_eq!(
            usage(key(0x38, LLKHF_EXTENDED, 0)),
            Some(HidUsage::key(0xE6))
        );
        assert_eq!(
            usage(key(0x1D, LLKHF_EXTENDED, 0)),
            Some(HidUsage::key(0xE4))
        );
        // La tecla `<` de ISO.
        assert_eq!(usage(key(0x56, 0, 0)), Some(HidUsage::key(0x64)));
    }

    #[test]
    fn bloqnum_y_pausa_comparten_scancode() {
        // El hook entrega 0x45 para las dos; BloqNum con la marca de extendida.
        assert_eq!(
            usage(key(0x45, LLKHF_EXTENDED, 0)),
            Some(HidUsage::key(0x53))
        );
        assert_eq!(usage(key(0x45, 0, 0)), Some(HidUsage::key(0x48)));
        // Ctrl+Pausa (Inter).
        let brk = KeyboardHook {
            vk: VK_CANCEL,
            ..key(0x46, LLKHF_EXTENDED, 0)
        };
        assert_eq!(usage(brk), Some(HidUsage::key(0x48)));
    }

    #[test]
    fn multimedia_sin_scancode_por_codigo_virtual() {
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
        // El mismo volumen con su scancode real da lo mismo.
        assert_eq!(
            usage(key(0x20, LLKHF_EXTENDED, 0)),
            Some(HidUsage::key(0x7F))
        );
    }

    #[test]
    fn shift_falso_y_codigos_desconocidos_se_descartan() {
        assert_eq!(key_event(&key(0x2A, LLKHF_EXTENDED, 0)), None);
        assert_eq!(key_event(&key(0x36, LLKHF_EXTENDED, 0)), None);
        assert_eq!(key_event(&key(0, 0, 0)), None);
        assert_eq!(key_event(&key(0x1_0000, 0, 0)), None);
    }

    #[test]
    fn el_estado_sale_de_llkhf_up() {
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
    fn ctrl_con_0x21d_se_descarta_siempre() {
        let fake = key(FAKE_CTRL_SCAN, 0, 1);
        let ralt = key(SCAN_ALT, LLKHF_EXTENDED, 2);
        let fake_up = key(FAKE_CTRL_SCAN, LLKHF_UP, 3);
        assert_eq!(filter(&[fake, ralt, fake_up]), vec![ralt]);
    }

    #[test]
    fn ctrl_real_mantenido_sobrevive_a_altgr() {
        // Ctrl real (t=1), AltGr con su Ctrl falso (t=9), suelta AltGr, suelta Ctrl.
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
    fn ctrl_y_alt_derecho_en_tiempos_distintos_son_reales() {
        let ctrl = key(SCAN_CONTROL, 0, 1);
        let ralt = key(SCAN_ALT, LLKHF_EXTENDED, 2);
        assert_eq!(filter(&[ctrl, ralt]), vec![ctrl, ralt]);
        // Mismo tiempo pero sentido opuesto: tampoco es el par de AltGr.
        let ctrl_up = key(SCAN_CONTROL, LLKHF_UP, 3);
        let ralt_down = key(SCAN_ALT, LLKHF_EXTENDED, 3);
        assert_eq!(filter(&[ctrl_up, ralt_down]), vec![ctrl_up, ralt_down]);
        // Ctrl derecho (E0 1D) no se retiene.
        let rctrl = key(SCAN_CONTROL, LLKHF_EXTENDED, 4);
        assert_eq!(filter(&[rctrl]), vec![rctrl]);
    }

    #[test]
    fn el_ctrl_retenido_sale_al_vencer_la_ventana() {
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

    proptest! {
        /// El filtro solo quita Ctrl izquierdos (y todo `0x21D`): lo demás sale
        /// tal cual y en el mismo orden.
        #[test]
        fn altgr_solo_quita_ctrl_izquierdo(events in prop::collection::vec(arb_key(), 0..40)) {
            let out = filter(&events);
            let others = |v: &[KeyboardHook]| {
                v.iter()
                    .copied()
                    .filter(|k| !is_left_ctrl(k) && k.scan_code != FAKE_CTRL_SCAN)
                    .collect::<Vec<_>>()
            };
            prop_assert_eq!(others(&out), others(&events));
            // Lo que sale es una subsecuencia de lo que entró.
            let mut it = events.iter();
            for k in &out {
                prop_assert!(it.any(|e| e == k));
            }
            prop_assert!(out.iter().all(|k| k.scan_code != FAKE_CTRL_SCAN));
        }

        /// Sin Alt derecho cerca, ningún Ctrl real se pierde.
        #[test]
        fn sin_altgr_no_se_pierde_ningun_ctrl(events in prop::collection::vec(arb_key(), 0..40)) {
            let events: Vec<_> = events
                .into_iter()
                .filter(|k| !is_right_alt(k) && k.scan_code != FAKE_CTRL_SCAN)
                .collect();
            prop_assert_eq!(filter(&events), events);
        }
    }
}
