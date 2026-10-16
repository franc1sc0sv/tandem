//! Tragar la entrada y congelar/ocultar el cursor, de forma balanceada.
//!
//! `CGDisplayHideCursor` lleva un contador: cada ocultar necesita su mostrar, o el
//! cursor queda invisible. Esto garantiza que se oculta y congela una sola vez por
//! cada vez que se toma la entrada, y que se devuelve todo aunque la toma se
//! suelte desde el hilo del tap (el consumidor de eventos desapareció) o al
//! destruir la captura.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::CaptureError;
use crate::geometry::Point;

/// Lo que la toma necesita del sistema. En macOS lo implementa `sys` con
/// CoreGraphics; las pruebas usan un doble.
pub trait CursorBackend: Send {
    /// `true`: el mouse deja de mover el cursor (pero sus deltas siguen llegando
    /// al tap). `CGAssociateMouseAndMouseCursorPosition(!frozen)`.
    fn set_frozen(&mut self, frozen: bool) -> Result<(), CaptureError>;
    /// Ocultar o mostrar el cursor aunque Tandem no sea la app activa.
    fn set_hidden(&mut self, hidden: bool) -> Result<(), CaptureError>;
    /// Poner el cursor en `pos` (coordenadas globales en puntos).
    fn warp(&mut self, pos: Point) -> Result<(), CaptureError>;
    /// Secure Event Input (un campo de contraseña tiene el foco): el tap no ve
    /// el teclado, así que no se cruza a la otra máquina.
    fn secure_input_active(&self) -> bool;
}

struct Inner<C> {
    backend: C,
    /// Lo que está aplicado de verdad. Van por separado para que un fallo a mitad
    /// de camino no desbalancee el contador de ocultar: lo que no se pudo
    /// deshacer se reintenta la próxima vez.
    frozen: bool,
    hidden: bool,
}

/// Estado de la toma, compartido entre el dueño de la captura y el hilo del tap.
pub struct Grab<C> {
    /// Lo que lee el tap en cada evento, sin bloquear.
    grabbed: AtomicBool,
    inner: Mutex<Inner<C>>,
}

impl<C: CursorBackend> Grab<C> {
    pub fn new(backend: C) -> Self {
        Self {
            grabbed: AtomicBool::new(false),
            inner: Mutex::new(Inner {
                backend,
                frozen: false,
                hidden: false,
            }),
        }
    }

    /// Si el tap tiene que tragar la entrada.
    pub fn is_grabbed(&self) -> bool {
        self.grabbed.load(Ordering::Acquire)
    }

    /// Toma (`true`) o devuelve (`false`) la entrada local. Repetir el mismo
    /// valor no hace nada.
    ///
    /// Tomarla falla con [`CaptureError::SecureInput`] si hay un campo de
    /// contraseña activo. Si congelar u ocultar falla, se deshace lo hecho y la
    /// entrada sigue local.
    pub fn set(&self, grab: bool) -> Result<(), CaptureError> {
        let mut inner = self.lock();
        if grab {
            if self.is_grabbed() {
                return Ok(());
            }
            if inner.backend.secure_input_active() {
                return Err(CaptureError::SecureInput);
            }
            if !inner.frozen {
                inner.backend.set_frozen(true)?;
                inner.frozen = true;
            }
            if !inner.hidden {
                if let Err(e) = inner.backend.set_hidden(true) {
                    // Lo más importante es no dejar el mouse muerto.
                    if inner.backend.set_frozen(false).is_ok() {
                        inner.frozen = false;
                    }
                    return Err(e);
                }
                inner.hidden = true;
            }
            self.grabbed.store(true, Ordering::Release);
            Ok(())
        } else {
            // Primero deja de tragar: aunque falle lo demás, la Mac vuelve a
            // recibir su teclado.
            self.grabbed.store(false, Ordering::Release);
            let mut result = Ok(());
            if inner.hidden {
                match inner.backend.set_hidden(false) {
                    Ok(()) => inner.hidden = false,
                    Err(e) => result = Err(e),
                }
            }
            if inner.frozen {
                match inner.backend.set_frozen(false) {
                    Ok(()) => inner.frozen = false,
                    Err(e) => result = result.and(Err(e)),
                }
            }
            result
        }
    }

    /// Devuelve la entrada sin importar errores (al destruir la captura, o
    /// cuando el consumidor de eventos ya no existe).
    pub fn release(&self) {
        let _ = self.set(false);
    }

    /// Lleva el cursor a `pos`. Con la entrada local, vuelve a asociar el mouse:
    /// así macOS no ignora el movimiento durante el cuarto de segundo que sigue a
    /// un warp.
    pub fn warp(&self, pos: Point) -> Result<(), CaptureError> {
        let mut inner = self.lock();
        inner.backend.warp(pos)?;
        if !self.is_grabbed() {
            inner.backend.set_frozen(false)?;
            inner.frozen = false;
        }
        Ok(())
    }

    pub fn secure_input_active(&self) -> bool {
        self.lock().backend.secure_input_active()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner<C>> {
        // Un pánico con el candado tomado no invalida el estado (son booleanos),
        // y devolver el cursor importa más que propagar el veneno.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Call {
        Frozen(bool),
        Hidden(bool),
        Warp(Point),
    }

    /// Doble de prueba: registra las llamadas y simula el contador de
    /// `CGDisplayHideCursor`.
    #[derive(Debug, Default)]
    pub struct State {
        pub calls: Vec<Call>,
        pub hide_count: i32,
        pub frozen: bool,
        pub secure_input: bool,
        pub fail_frozen: bool,
        pub fail_hidden: bool,
    }

    #[derive(Debug, Clone, Default)]
    pub struct FakeCursor(pub Arc<Mutex<State>>);

    impl FakeCursor {
        pub fn state(&self) -> std::sync::MutexGuard<'_, State> {
            self.0.lock().unwrap()
        }
    }

    impl CursorBackend for FakeCursor {
        fn set_frozen(&mut self, frozen: bool) -> Result<(), CaptureError> {
            let mut s = self.state();
            s.calls.push(Call::Frozen(frozen));
            if s.fail_frozen {
                return Err(CaptureError::Cursor {
                    op: "CGAssociateMouseAndMouseCursorPosition",
                    code: 1001,
                });
            }
            s.frozen = frozen;
            Ok(())
        }

        fn set_hidden(&mut self, hidden: bool) -> Result<(), CaptureError> {
            let mut s = self.state();
            s.calls.push(Call::Hidden(hidden));
            if s.fail_hidden {
                return Err(CaptureError::Cursor {
                    op: "CGDisplayHideCursor",
                    code: 1001,
                });
            }
            s.hide_count += if hidden { 1 } else { -1 };
            Ok(())
        }

        fn warp(&mut self, pos: Point) -> Result<(), CaptureError> {
            self.state().calls.push(Call::Warp(pos));
            Ok(())
        }

        fn secure_input_active(&self) -> bool {
            self.state().secure_input
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::fake::{Call, FakeCursor};
    use super::*;

    fn grab() -> (Grab<FakeCursor>, FakeCursor) {
        let cursor = FakeCursor::default();
        (Grab::new(cursor.clone()), cursor)
    }

    #[test]
    fn tomar_congela_y_oculta_una_vez() {
        let (g, c) = grab();
        assert!(!g.is_grabbed());
        g.set(true).unwrap();
        g.set(true).unwrap();
        assert!(g.is_grabbed());
        {
            let s = c.state();
            assert_eq!(s.calls, vec![Call::Frozen(true), Call::Hidden(true)]);
            assert_eq!(s.hide_count, 1);
            assert!(s.frozen);
        }
        g.set(false).unwrap();
        g.set(false).unwrap();
        assert!(!g.is_grabbed());
        let s = c.state();
        assert_eq!(s.hide_count, 0);
        assert!(!s.frozen);
        assert_eq!(s.calls.len(), 4);
    }

    #[test]
    fn con_secure_input_no_se_toma() {
        let (g, c) = grab();
        c.state().secure_input = true;
        assert!(g.secure_input_active());
        assert!(matches!(g.set(true), Err(CaptureError::SecureInput)));
        assert!(!g.is_grabbed());
        assert!(c.state().calls.is_empty());
        // Soltar sigue funcionando.
        g.set(false).unwrap();
    }

    #[test]
    fn si_ocultar_falla_se_descongela() {
        let (g, c) = grab();
        c.state().fail_hidden = true;
        assert!(g.set(true).is_err());
        assert!(!g.is_grabbed());
        let s = c.state();
        assert!(!s.frozen);
        assert_eq!(s.hide_count, 0);
    }

    #[test]
    fn si_congelar_falla_no_se_oculta() {
        let (g, c) = grab();
        c.state().fail_frozen = true;
        assert!(g.set(true).is_err());
        assert!(!g.is_grabbed());
        assert_eq!(c.state().calls, vec![Call::Frozen(true)]);
    }

    #[test]
    fn soltar_deja_de_tragar_aunque_falle_mostrar() {
        let (g, c) = grab();
        g.set(true).unwrap();
        c.state().fail_hidden = true;
        assert!(g.set(false).is_err());
        assert!(!g.is_grabbed());
        // Igual intentó descongelar.
        assert!(!c.state().frozen);
    }

    #[test]
    fn release_desde_otro_hilo() {
        let (g, c) = grab();
        let g = std::sync::Arc::new(g);
        g.set(true).unwrap();
        let g2 = g.clone();
        std::thread::spawn(move || g2.release()).join().unwrap();
        assert!(!g.is_grabbed());
        assert_eq!(c.state().hide_count, 0);
    }

    #[test]
    fn warp_reasocia_solo_con_foco_local() {
        let (g, c) = grab();
        let p = Point::new(100.0, 200.0);
        g.warp(p).unwrap();
        assert_eq!(c.state().calls, vec![Call::Warp(p), Call::Frozen(false)]);
        g.set(true).unwrap();
        c.state().calls.clear();
        g.warp(p).unwrap();
        assert_eq!(c.state().calls, vec![Call::Warp(p)]);
        assert!(c.state().frozen);
    }

    proptest! {
        /// Cualquier secuencia de tomar/soltar (con fallos intermedios) deja el
        /// contador de ocultar en 0 o 1, coherente con `is_grabbed`, y soltar al
        /// final siempre deja el cursor visible y libre.
        #[test]
        fn el_cursor_queda_balanceado(ops in prop::collection::vec((any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()), 0..40)) {
            let (g, c) = grab();
            for (want, secure, fail_frozen, fail_hidden) in ops {
                {
                    let mut s = c.state();
                    s.secure_input = secure;
                    s.fail_frozen = fail_frozen;
                    s.fail_hidden = fail_hidden;
                }
                let _ = g.set(want);
                let s = c.state();
                prop_assert!(s.hide_count == 0 || s.hide_count == 1);
                if g.is_grabbed() {
                    prop_assert_eq!(s.hide_count, 1);
                    prop_assert!(s.frozen);
                }
            }
            {
                let mut s = c.state();
                s.fail_frozen = false;
                s.fail_hidden = false;
            }
            g.release();
            let s = c.state();
            prop_assert!(!g.is_grabbed());
            prop_assert_eq!(s.hide_count, 0);
            prop_assert!(!s.frozen);
        }
    }
}
