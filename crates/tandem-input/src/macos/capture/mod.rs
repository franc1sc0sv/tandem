//! Captura de teclado y mouse en macOS (docs/research/input-engine.md).
//!
//! Un CGEventTap **activo** en `kCGHIDEventTap`, en un hilo propio con su
//! CFRunLoop, ve toda la entrada antes que cualquier app. Cada evento se traduce
//! al modelo portable ([`translate`]) y se entrega al consumidor (el driver de la
//! sesión); mientras el foco está en la otra máquina el tap lo traga (devuelve
//! NULL) y el cursor local queda congelado y oculto ([`grab`]).
//!
//! - Si el sistema apaga el tap por tardar (`kCGEventTapDisabledByTimeout`), se
//!   vuelve a encender en el mismo callback.
//! - Con Secure Event Input (un campo de contraseña) no se toma la entrada: el
//!   tap no ve el teclado y el usuario quedaría escribiendo a ciegas.
//! - Hace falta el permiso de Accesibilidad (TCC) para un tap activo; sin él,
//!   [`MacCapture::start`] falla antes de crear nada ([`Permissions`]).
//! - Si el consumidor de eventos desaparece con la entrada tomada, el tap la
//!   devuelve solo: la Mac nunca queda sin teclado.

pub mod grab;
pub mod translate;

#[cfg(target_os = "macos")]
mod sys;

use thiserror::Error;

pub use grab::{CursorBackend, Grab};
#[cfg(target_os = "macos")]
pub use sys::{MacCapture, SysCursor, permissions, request_permissions, secure_input_active};
pub use translate::{INJECTED_MARKER, RawEvent, TapAction, Translator};

#[derive(Debug, Error)]
pub enum CaptureError {
    /// Falta un permiso de TCC. Se pide con `request_permissions` y el usuario lo
    /// concede en Ajustes del Sistema → Privacidad y seguridad.
    #[error("falta el permiso de {0} (Ajustes del Sistema → Privacidad y seguridad)")]
    Permission(Permission),
    /// Hay un campo de contraseña activo (Secure Event Input).
    #[error("hay un campo de contraseña activo (Secure Event Input); no se cruza de pantalla")]
    SecureInput,
    /// `CGEventTapCreate` devolvió NULL (normalmente, permisos).
    #[error("no se pudo crear el CGEventTap")]
    TapCreate,
    /// El hilo del tap no arrancó o murió antes de avisar.
    #[error("el hilo del tap de eventos no arrancó: {0}")]
    Thread(String),
    /// Una llamada de CoreGraphics sobre el cursor devolvió un `CGError`.
    #[error("{op} falló con CGError {code}")]
    Cursor { op: &'static str, code: i32 },
}

/// Permisos de TCC que importan para capturar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Accesibilidad: necesaria para un tap activo (y para inyectar).
    Accessibility,
    /// Monitorización de entrada: basta para un tap que solo escucha.
    InputMonitoring,
}

impl std::fmt::Display for Permission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Permission::Accessibility => "Accesibilidad",
            Permission::InputMonitoring => "Monitorización de entrada",
        })
    }
}

/// Estado de los permisos de TCC del proceso. El permiso queda atado a la firma
/// del binario: hace falta una `.app` firmada de forma estable para no volver a
/// pedirlo en cada compilación.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Permissions {
    pub accessibility: bool,
    pub input_monitoring: bool,
}

impl Permissions {
    /// Lo que falta para un tap activo, que traga eventos.
    pub fn missing_for_active_tap(self) -> Option<Permission> {
        (!self.accessibility).then_some(Permission::Accessibility)
    }

    /// Todo lo que falta, para mostrarlo en la UI.
    pub fn missing(self) -> Vec<Permission> {
        let mut v = Vec::new();
        if !self.accessibility {
            v.push(Permission::Accessibility);
        }
        if !self.input_monitoring {
            v.push(Permission::InputMonitoring);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_tap_activo_solo_exige_accesibilidad() {
        let p = Permissions {
            accessibility: true,
            input_monitoring: false,
        };
        assert_eq!(p.missing_for_active_tap(), None);
        assert_eq!(p.missing(), vec![Permission::InputMonitoring]);
        let p = Permissions::default();
        assert_eq!(p.missing_for_active_tap(), Some(Permission::Accessibility));
        assert_eq!(
            p.missing(),
            vec![Permission::Accessibility, Permission::InputMonitoring]
        );
        assert!(
            Permissions {
                accessibility: true,
                input_monitoring: true
            }
            .missing()
            .is_empty()
        );
    }

    #[test]
    fn mensajes_de_error_en_espanol() {
        assert_eq!(
            CaptureError::Permission(Permission::Accessibility).to_string(),
            "falta el permiso de Accesibilidad (Ajustes del Sistema → Privacidad y seguridad)"
        );
        assert_eq!(
            CaptureError::Cursor {
                op: "CGDisplayHideCursor",
                code: 1001
            }
            .to_string(),
            "CGDisplayHideCursor falló con CGError 1001"
        );
    }
}
