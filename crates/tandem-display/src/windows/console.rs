//! Ctrl+C y cierre de consola durante `tandem-ddc switch`.
//!
//! Si el proceso muere entre la ida y la vuelta, el monitor se queda en la otra entrada.
//! Por eso Ctrl+C no mata el proceso: levanta una bandera que adelanta la vuelta. Al cerrar
//! la ventana Windows termina el proceso en cuanto el manejador regresa, así que el
//! manejador espera (con tope) a que la vuelta se haya mandado.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ::windows::Win32::System::Console::{
    CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler,
};
use ::windows::core::BOOL;

use crate::ddc::DdcError;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static FINISHED: AtomicBool = AtomicBool::new(false);
/// Windows da unos 5 s al manejador de cierre antes de matar el proceso.
const CLOSE_GRACE: Duration = Duration::from_millis(4500);

unsafe extern "system" fn on_ctrl(kind: u32) -> BOOL {
    INTERRUPTED.store(true, Ordering::SeqCst);
    if [CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT].contains(&kind) {
        let start = Instant::now();
        while !FINISHED.load(Ordering::SeqCst) && start.elapsed() < CLOSE_GRACE {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    true.into()
}

/// Instala el manejador y devuelve la bandera que se levanta con Ctrl+C.
pub fn install_interrupt_handler() -> Result<&'static AtomicBool, DdcError> {
    // SAFETY: `on_ctrl` es una función estática que solo toca atómicos.
    unsafe { SetConsoleCtrlHandler(Some(on_ctrl), true) }.map_err(|e| DdcError::Os {
        op: "SetConsoleCtrlHandler",
        code: e.code().0 as u32,
        message: e.message(),
    })?;
    Ok(&INTERRUPTED)
}

/// Avisa al manejador de cierre que ya no hace falta esperar.
pub fn mark_finished() {
    FINISHED.store(true, Ordering::SeqCst);
}
