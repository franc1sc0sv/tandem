//! Captura de teclado y mouse en Windows (docs/research/input-engine.md).
//!
//! Dos hilos:
//!
//! - **Hilo de hooks** (`cfg(windows)`): instala `WH_KEYBOARD_LL` y `WH_MOUSE_LL` y
//!   recibe Raw Input (`RIDEV_INPUTSINK`) en una ventana solo de mensajes. Cada
//!   callback copia la estructura del sistema, decide si se traga el evento
//!   ([`HookFilter`]) y lo encola sin bloquear. Windows quita en silencio un hook
//!   que tarde más que `LowLevelHooksTimeout`, así que ahí no se hace nada más.
//! - **Hilo de proceso** ([`run_pipeline`], portable): traduce a [`InputEvent`]
//!   ([`Translator`]: filtro de inyectados, Ctrl falso de AltGr, teclas por
//!   scancode, deltas de Raw Input) y corre el [`Watchdog`], que compara los
//!   hooks con Raw Input de dispositivos reales y los reinstala si Windows los
//!   quitó. Nunca inyecta nada.
//!
//! Contrato con la sesión (T-026): el movimiento llega como `MotionRelative` de
//! Raw Input (sin aceleración, en píxeles físicos) siempre, y como
//! `MotionAbsolute` con la posición real del cursor mientras la entrada no se
//! traga. Las coordenadas son físicas porque el proceso se declara
//! Per-Monitor-V2 DPI aware.

mod keyboard;
mod mouse;
mod watchdog;

#[cfg(windows)]
mod sys;

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::event::InputEvent;

pub use keyboard::{AltGrFilter, key_event};
pub use mouse::{RawMotion, mouse_event};
pub use watchdog::{HookKind, Watchdog, WatchdogAction, WatchdogConfig};

#[cfg(windows)]
pub use sys::{WindowsCapture, ensure_per_monitor_v2};

/// Constantes de Win32 que usa la lógica portable. `sys` comprueba en compilación
/// que coinciden con las del crate `windows`.
pub mod consts {
    pub const LLKHF_EXTENDED: u32 = 0x01;
    pub const LLKHF_INJECTED: u32 = 0x10;
    pub const LLKHF_UP: u32 = 0x80;
    pub const LLMHF_INJECTED: u32 = 0x01;

    pub const WM_MOUSEMOVE: u32 = 0x0200;
    pub const WM_LBUTTONDOWN: u32 = 0x0201;
    pub const WM_LBUTTONUP: u32 = 0x0202;
    pub const WM_RBUTTONDOWN: u32 = 0x0204;
    pub const WM_RBUTTONUP: u32 = 0x0205;
    pub const WM_MBUTTONDOWN: u32 = 0x0207;
    pub const WM_MBUTTONUP: u32 = 0x0208;
    pub const WM_MOUSEWHEEL: u32 = 0x020A;
    pub const WM_XBUTTONDOWN: u32 = 0x020B;
    pub const WM_XBUTTONUP: u32 = 0x020C;
    pub const WM_MOUSEHWHEEL: u32 = 0x020E;

    pub const XBUTTON1: u16 = 0x0001;
    pub const XBUTTON2: u16 = 0x0002;

    pub const MOUSE_MOVE_ABSOLUTE: u16 = 0x01;
    pub const MOUSE_VIRTUAL_DESKTOP: u16 = 0x02;

    pub const VK_CANCEL: u32 = 0x03;
    pub const VK_VOLUME_MUTE: u32 = 0xAD;
    pub const VK_VOLUME_DOWN: u32 = 0xAE;
    pub const VK_VOLUME_UP: u32 = 0xAF;
    pub const VK_MEDIA_NEXT_TRACK: u32 = 0xB0;
    pub const VK_MEDIA_PREV_TRACK: u32 = 0xB1;
    pub const VK_MEDIA_STOP: u32 = 0xB2;
    pub const VK_MEDIA_PLAY_PAUSE: u32 = 0xB3;
}

/// Copia de `KBDLLHOOKSTRUCT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardHook {
    pub vk: u32,
    pub scan_code: u32,
    pub flags: u32,
    /// Milisegundos del sistema; el Ctrl falso de AltGr lleva el mismo que AltGr.
    pub time: u32,
    pub extra_info: usize,
}

impl KeyboardHook {
    pub fn injected(&self) -> bool {
        self.flags & consts::LLKHF_INJECTED != 0
    }

    pub fn is_up(&self) -> bool {
        self.flags & consts::LLKHF_UP != 0
    }

    pub fn is_extended(&self) -> bool {
        self.flags & consts::LLKHF_EXTENDED != 0
    }
}

/// Copia de `MSLLHOOKSTRUCT` más el mensaje (`wParam`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseHook {
    pub message: u32,
    /// Posición del cursor en píxeles físicos del escritorio virtual.
    pub x: i32,
    pub y: i32,
    pub mouse_data: u32,
    pub flags: u32,
    pub time: u32,
    pub extra_info: usize,
}

impl MouseHook {
    pub fn injected(&self) -> bool {
        self.flags & consts::LLMHF_INJECTED != 0
    }

    /// Palabra alta de `mouseData`: delta de rueda o número de botón X.
    pub fn high_word(&self) -> u16 {
        (self.mouse_data >> 16) as u16
    }
}

/// Lo que importa de un `RAWMOUSE`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawMouse {
    /// `hDevice`; 0 = inyectado con `SendInput`.
    pub device: usize,
    /// `usFlags`.
    pub flags: u16,
    pub last_x: i32,
    pub last_y: i32,
    /// Ancho y alto en píxeles del área que cubre el rango 0–65535 de los
    /// movimientos absolutos (escritorio virtual o pantalla principal).
    pub span: (f64, f64),
}

/// Lo que el hilo de hooks encola.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HookEvent {
    Keyboard {
        hook: KeyboardHook,
        swallowed: bool,
    },
    Mouse {
        hook: MouseHook,
        swallowed: bool,
    },
    Raw(RawMouse),
    /// Raw Input de teclado; solo le sirve al watchdog.
    RawKeyboard {
        /// `hDevice`; 0 = inyectado con `SendInput`.
        device: usize,
    },
}

impl HookEvent {
    /// Qué hook demuestra que sigue vivo este evento.
    pub fn hook_kind(&self) -> Option<HookKind> {
        match self {
            HookEvent::Keyboard { .. } => Some(HookKind::Keyboard),
            HookEvent::Mouse { .. } => Some(HookKind::Mouse),
            HookEvent::Raw(_) | HookEvent::RawKeyboard { .. } => None,
        }
    }

    /// Qué tipo de entrada real (de un dispositivo, no inyectada) demuestra este
    /// evento de Raw Input. Un mouse sin movimiento no cuenta: basta con que el
    /// watchdog no actúe de más.
    pub fn real_activity(&self) -> Option<HookKind> {
        match self {
            HookEvent::Raw(r) if r.device != 0 && (r.last_x != 0 || r.last_y != 0) => {
                Some(HookKind::Mouse)
            }
            HookEvent::RawKeyboard { device } if *device != 0 => Some(HookKind::Keyboard),
            _ => None,
        }
    }
}

/// Qué hace el hook con un evento.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Seguir la cadena: el sistema local lo recibe.
    Pass,
    /// Tragarlo: el foco está en la otra máquina.
    Swallow,
}

/// Decisión del hook, con memoria de lo que el sistema local vio presionarse.
///
/// Si una tecla o un botón se presionó con la entrada local (por ejemplo,
/// arrastrando hasta el borde) y se suelta ya con la entrada tragada, la
/// liberación pasa igual: si no, quedaría pegada en Windows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookFilter {
    /// Bit por código virtual (0–255).
    keys: [u64; 4],
    /// Bit por botón: izquierdo, derecho, medio, X1, X2.
    buttons: u8,
}

impl HookFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn keyboard(&mut self, grab: bool, k: &KeyboardHook) -> Verdict {
        if k.injected() {
            return Verdict::Pass;
        }
        let vk = (k.vk & 0xFF) as usize;
        let (word, bit) = (vk / 64, 1u64 << (vk % 64));
        let local = self.keys[word] & bit != 0;
        if k.is_up() {
            self.keys[word] &= !bit;
            if local || !grab {
                return Verdict::Pass;
            }
            return Verdict::Swallow;
        }
        if grab {
            Verdict::Swallow
        } else {
            self.keys[word] |= bit;
            Verdict::Pass
        }
    }

    pub fn mouse(&mut self, grab: bool, m: &MouseHook) -> Verdict {
        if m.injected() {
            return Verdict::Pass;
        }
        let Some((index, down)) = mouse::button_bit(m) else {
            return if grab {
                Verdict::Swallow
            } else {
                Verdict::Pass
            };
        };
        let bit = 1u8 << index;
        let local = self.buttons & bit != 0;
        if !down {
            self.buttons &= !bit;
            return if local || !grab {
                Verdict::Pass
            } else {
                Verdict::Swallow
            };
        }
        if grab {
            Verdict::Swallow
        } else {
            self.buttons |= bit;
            Verdict::Pass
        }
    }
}

/// Traduce lo que encola el hilo de hooks a eventos de Tandem.
#[derive(Debug)]
pub struct Translator {
    altgr: AltGrFilter,
    raw: RawMotion,
}

impl Translator {
    /// `altgr_window`: cuánto se retiene un Ctrl izquierdo esperando a ver si es
    /// el falso de AltGr.
    pub fn new(altgr_window: Duration) -> Self {
        Self {
            altgr: AltGrFilter::new(altgr_window),
            raw: RawMotion::new(),
        }
    }

    pub fn push(&mut self, event: HookEvent, now: Instant, out: &mut impl FnMut(InputEvent)) {
        let mut emit_key = |k: KeyboardHook| {
            if let Some(ev) = key_event(&k) {
                out(ev);
            }
        };
        match event {
            HookEvent::Keyboard { hook, .. } => {
                if !hook.injected() {
                    self.altgr.push(hook, now, &mut emit_key);
                }
            }
            HookEvent::Mouse { hook, swallowed } => {
                if hook.injected() {
                    return;
                }
                if let Some(ev) = mouse_event(&hook, swallowed) {
                    // El Ctrl retenido ocurrió antes: va primero.
                    self.altgr.flush_all(&mut emit_key);
                    out(ev);
                }
            }
            HookEvent::Raw(raw) => {
                if let Some((dx, dy)) = self.raw.push(&raw) {
                    self.altgr.flush_all(&mut emit_key);
                    out(InputEvent::MotionRelative { dx, dy });
                }
            }
            HookEvent::RawKeyboard { .. } => {}
        }
    }

    /// Suelta el Ctrl retenido si ya pasó la ventana de AltGr.
    pub fn flush_expired(&mut self, now: Instant, out: &mut impl FnMut(InputEvent)) {
        self.altgr.flush_expired(now, &mut |k| {
            if let Some(ev) = key_event(&k) {
                out(ev);
            }
        });
    }

    pub fn flush_all(&mut self, out: &mut impl FnMut(InputEvent)) {
        self.altgr.flush_all(&mut |k| {
            if let Some(ev) = key_event(&k) {
                out(ev);
            }
        });
    }

    pub fn has_pending(&self) -> bool {
        self.altgr.has_pending()
    }
}

/// Errores al arrancar la captura.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("{op}: {message} (0x{code:08X})")]
    Os {
        op: &'static str,
        code: u32,
        message: String,
    },
    #[error("el proceso no pudo declararse Per-Monitor-V2 DPI aware")]
    DpiAwareness,
    #[error("no se pudo crear el hilo de captura: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("el hilo de hooks terminó antes de arrancar")]
    ThreadDied,
}

/// Parámetros de la captura.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureConfig {
    /// Ventana para reconocer el Ctrl falso de AltGr por tiempo. Los dos eventos
    /// llegan seguidos en el mismo hilo, así que basta con muy poco.
    pub altgr_window: Duration,
    /// Cada cuánto despierta el hilo de proceso sin eventos (watchdog).
    pub tick: Duration,
    pub watchdog: WatchdogConfig,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            altgr_window: Duration::from_millis(10),
            tick: Duration::from_millis(100),
            watchdog: WatchdogConfig::default(),
        }
    }
}

/// Lo que el hilo de proceso le pide al sistema; [`sys`] lo implementa con un
/// mensaje al hilo de hooks, y las pruebas con un doble.
pub trait HookControl {
    /// Quita y vuelve a instalar los dos hooks.
    fn reinstall(&mut self);
}

/// Bucle del hilo de proceso: traduce lo que llega por `rx` y lo entrega a `out`.
/// Termina cuando el hilo de hooks suelta su extremo del canal.
pub fn run_pipeline(
    rx: &Receiver<HookEvent>,
    control: &mut impl HookControl,
    out: &mut impl FnMut(InputEvent),
    config: &CaptureConfig,
) {
    let mut translator = Translator::new(config.altgr_window);
    let mut watchdog = Watchdog::new(config.watchdog);
    loop {
        let wait = if translator.has_pending() {
            config.altgr_window
        } else {
            config.tick
        };
        match rx.recv_timeout(wait) {
            Ok(event) => {
                let now = Instant::now();
                if let Some(kind) = event.hook_kind() {
                    watchdog.hook_activity(kind, now);
                }
                if let Some(kind) = event.real_activity() {
                    watchdog.real_activity(kind, now);
                }
                translator.push(event, now, out);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                translator.flush_all(out);
                return;
            }
        }
        let now = Instant::now();
        translator.flush_expired(now, out);
        if watchdog.tick(now) == WatchdogAction::Reinstall {
            control.reinstall();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::consts::*;
    use super::*;
    use crate::event::{Button, HidUsage, KeyState, Scroll};
    use crate::windows::TANDEM_EXTRA_INFO;

    pub(super) fn key(scan: u32, flags: u32, time: u32) -> KeyboardHook {
        KeyboardHook {
            vk: 0,
            scan_code: scan,
            flags,
            time,
            extra_info: 0,
        }
    }

    fn mouse(message: u32, mouse_data: u32) -> MouseHook {
        MouseHook {
            message,
            x: 10,
            y: 20,
            mouse_data,
            flags: 0,
            time: 0,
            extra_info: 0,
        }
    }

    fn kb(hook: KeyboardHook) -> HookEvent {
        HookEvent::Keyboard {
            hook,
            swallowed: false,
        }
    }

    #[derive(Default)]
    struct FakeControl {
        reinstalls: u32,
    }

    impl HookControl for FakeControl {
        fn reinstall(&mut self) {
            self.reinstalls += 1;
        }
    }

    fn run(events: Vec<HookEvent>) -> Vec<InputEvent> {
        let (tx, rx) = mpsc::channel();
        for e in events {
            tx.send(e).unwrap();
        }
        drop(tx);
        let mut out = Vec::new();
        run_pipeline(
            &rx,
            &mut FakeControl::default(),
            &mut |e| out.push(e),
            &CaptureConfig::default(),
        );
        out
    }

    #[test]
    fn filtro_traga_solo_con_grab_y_nunca_lo_inyectado() {
        let mut f = HookFilter::new();
        let a = KeyboardHook {
            vk: 0x41,
            ..key(0x1E, 0, 0)
        };
        assert_eq!(f.keyboard(false, &a), Verdict::Pass);
        assert_eq!(
            f.keyboard(
                false,
                &KeyboardHook {
                    flags: LLKHF_UP,
                    ..a
                }
            ),
            Verdict::Pass
        );
        assert_eq!(f.keyboard(true, &a), Verdict::Swallow);
        assert_eq!(
            f.keyboard(
                true,
                &KeyboardHook {
                    flags: LLKHF_UP,
                    ..a
                }
            ),
            Verdict::Swallow
        );
        let injected = KeyboardHook {
            flags: LLKHF_INJECTED,
            extra_info: TANDEM_EXTRA_INFO,
            ..a
        };
        assert_eq!(f.keyboard(true, &injected), Verdict::Pass);

        let mv = mouse(WM_MOUSEMOVE, 0);
        assert_eq!(f.mouse(false, &mv), Verdict::Pass);
        assert_eq!(f.mouse(true, &mv), Verdict::Swallow);
        let injected = MouseHook {
            flags: LLMHF_INJECTED,
            ..mv
        };
        assert_eq!(f.mouse(true, &injected), Verdict::Pass);
    }

    #[test]
    fn soltar_lo_presionado_en_local_pasa_aunque_haya_grab() {
        let mut f = HookFilter::new();
        let ctrl = KeyboardHook {
            vk: 0xA2,
            ..key(0x1D, 0, 0)
        };
        let up = KeyboardHook {
            flags: LLKHF_UP,
            ..ctrl
        };
        assert_eq!(f.keyboard(false, &ctrl), Verdict::Pass);
        // Repetición con el foco ya remoto: se traga, pero la tecla sigue local.
        assert_eq!(f.keyboard(true, &ctrl), Verdict::Swallow);
        assert_eq!(f.keyboard(true, &up), Verdict::Pass);
        // Ya soltada: la siguiente vuelta completa se traga.
        assert_eq!(f.keyboard(true, &ctrl), Verdict::Swallow);
        assert_eq!(f.keyboard(true, &up), Verdict::Swallow);

        let down = mouse(WM_LBUTTONDOWN, 0);
        let release = mouse(WM_LBUTTONUP, 0);
        assert_eq!(f.mouse(false, &down), Verdict::Pass);
        assert_eq!(f.mouse(true, &release), Verdict::Pass);
        assert_eq!(f.mouse(true, &down), Verdict::Swallow);
        assert_eq!(f.mouse(true, &release), Verdict::Swallow);
        // X1 y X2 se distinguen por la palabra alta.
        let x1 = mouse(WM_XBUTTONDOWN, u32::from(XBUTTON1) << 16);
        let x2_up = mouse(WM_XBUTTONUP, u32::from(XBUTTON2) << 16);
        assert_eq!(f.mouse(false, &x1), Verdict::Pass);
        assert_eq!(f.mouse(true, &x2_up), Verdict::Swallow);
    }

    #[test]
    fn pipeline_traduce_en_orden_y_descarta_inyectados() {
        let a = key(0x1E, 0, 1);
        let out = run(vec![
            kb(a),
            kb(KeyboardHook {
                flags: LLKHF_INJECTED,
                ..key(0x30, 0, 2)
            }),
            HookEvent::Mouse {
                hook: mouse(WM_LBUTTONDOWN, 0),
                swallowed: true,
            },
            HookEvent::Raw(RawMouse {
                device: 7,
                flags: 0,
                last_x: 3,
                last_y: -2,
                span: (0.0, 0.0),
            }),
            HookEvent::Raw(RawMouse {
                device: 0,
                flags: 0,
                last_x: 50,
                last_y: 50,
                span: (0.0, 0.0),
            }),
            HookEvent::RawKeyboard { device: 7 },
            HookEvent::Mouse {
                hook: mouse(WM_MOUSEWHEEL, (120u32) << 16),
                swallowed: true,
            },
            kb(KeyboardHook {
                flags: LLKHF_UP,
                ..a
            }),
        ]);
        assert_eq!(
            out,
            vec![
                InputEvent::Key {
                    usage: HidUsage::key(0x04),
                    state: KeyState::Pressed
                },
                InputEvent::Button {
                    button: Button::Left,
                    state: KeyState::Pressed
                },
                InputEvent::MotionRelative { dx: 3.0, dy: -2.0 },
                InputEvent::Scroll(Scroll::Wheel {
                    dx_v120: 0,
                    dy_v120: 120
                }),
                InputEvent::Key {
                    usage: HidUsage::key(0x04),
                    state: KeyState::Released
                },
            ]
        );
    }

    #[test]
    fn pipeline_quita_el_ctrl_falso_de_altgr() {
        let ralt = key(0x38, LLKHF_EXTENDED, 50);
        let lctrl = key(0x1D, 0, 50);
        let out = run(vec![
            kb(lctrl),
            kb(ralt),
            kb(KeyboardHook {
                flags: LLKHF_UP,
                ..lctrl
            }),
            kb(KeyboardHook {
                flags: LLKHF_EXTENDED | LLKHF_UP,
                ..ralt
            }),
        ]);
        let alt_right = HidUsage::key(0xE6);
        assert_eq!(
            out,
            vec![
                InputEvent::Key {
                    usage: alt_right,
                    state: KeyState::Pressed
                },
                InputEvent::Key {
                    usage: alt_right,
                    state: KeyState::Released
                },
            ]
        );
    }

    #[test]
    fn pipeline_suelta_el_ctrl_retenido_al_cerrar_y_ante_otros_eventos() {
        let lctrl = key(0x1D, 0, 5);
        let ctrl = HidUsage::key(0xE0);
        // Solo Ctrl y se corta el canal: no se pierde.
        assert_eq!(
            run(vec![kb(lctrl)]),
            vec![InputEvent::Key {
                usage: ctrl,
                state: KeyState::Pressed
            }]
        );
        // Ctrl + clic: el Ctrl va antes del clic.
        let out = run(vec![
            kb(lctrl),
            HookEvent::Mouse {
                hook: mouse(WM_LBUTTONDOWN, 0),
                swallowed: false,
            },
        ]);
        assert_eq!(
            out,
            vec![
                InputEvent::Key {
                    usage: ctrl,
                    state: KeyState::Pressed
                },
                InputEvent::Button {
                    button: Button::Left,
                    state: KeyState::Pressed
                },
            ]
        );
    }

    fn raw_mouse(device: usize, dx: i32) -> HookEvent {
        HookEvent::Raw(RawMouse {
            device,
            flags: 0,
            last_x: dx,
            last_y: 0,
            span: (0.0, 0.0),
        })
    }

    #[test]
    fn solo_el_raw_input_real_cuenta_como_entrada_real() {
        assert_eq!(raw_mouse(1, 1).hook_kind(), None);
        assert_eq!(raw_mouse(1, 1).real_activity(), Some(HookKind::Mouse));
        // Inyectado o sin movimiento: no cuenta.
        assert_eq!(raw_mouse(0, 1).real_activity(), None);
        assert_eq!(raw_mouse(1, 0).real_activity(), None);
        let kb_raw = HookEvent::RawKeyboard { device: 4 };
        assert_eq!(kb_raw.hook_kind(), None);
        assert_eq!(kb_raw.real_activity(), Some(HookKind::Keyboard));
        assert_eq!(HookEvent::RawKeyboard { device: 0 }.real_activity(), None);
        assert_eq!(kb(key(0x1E, 0, 0)).hook_kind(), Some(HookKind::Keyboard));
        assert_eq!(kb(key(0x1E, 0, 0)).real_activity(), None);
    }

    /// Corre el pipeline con plazos cortos mientras otro hilo manda `step()` cada
    /// 5 ms durante unos 300 ms. Devuelve cuántas veces reinstaló.
    fn reinstalls_with(step: fn() -> Vec<HookEvent>) -> u32 {
        let (tx, rx) = mpsc::channel();
        let feeder = std::thread::spawn(move || {
            for _ in 0..60 {
                for e in step() {
                    tx.send(e).unwrap();
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let config = CaptureConfig {
            tick: Duration::from_millis(5),
            watchdog: WatchdogConfig {
                hook_timeout: Duration::from_millis(100),
            },
            ..CaptureConfig::default()
        };
        let mut control = FakeControl::default();
        run_pipeline(&rx, &mut control, &mut |_| {}, &config);
        feeder.join().unwrap();
        control.reinstalls
    }

    #[test]
    fn pipeline_reinstala_si_el_teclado_real_no_llega_al_hook() {
        assert!(reinstalls_with(|| vec![HookEvent::RawKeyboard { device: 3 }]) >= 1);
        assert!(reinstalls_with(|| vec![raw_mouse(3, 2)]) >= 1);
    }

    #[test]
    fn pipeline_no_reinstala_con_hooks_vivos_ni_con_entrada_inyectada() {
        // Inyectado: Raw Input sin hDevice y nada en los hooks.
        assert_eq!(
            reinstalls_with(|| vec![HookEvent::RawKeyboard { device: 0 }, raw_mouse(0, 2)]),
            0
        );
        // Real y visto por los hooks.
        assert_eq!(
            reinstalls_with(|| vec![
                kb(key(0x1E, 0, 0)),
                HookEvent::RawKeyboard { device: 3 },
                raw_mouse(3, 2),
                HookEvent::Mouse {
                    hook: mouse(WM_MOUSEMOVE, 0),
                    swallowed: false,
                },
            ]),
            0
        );
    }

    #[test]
    fn pipeline_inactivo_no_toca_nada() {
        assert_eq!(reinstalls_with(Vec::new), 0);
    }
}
