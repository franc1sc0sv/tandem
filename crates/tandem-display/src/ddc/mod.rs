//! DDC/CI portable: el trait del bus, el parser de capabilities, el EDID, la base de
//! perfiles y el cambio de entrada con vuelta automática. La implementación real vive en
//! `crate::windows::ddc`.

pub mod caps;
pub mod edid;
pub mod fake;
pub mod profiles;
pub mod switch;

use std::fmt;

pub use caps::{Capabilities, CapsError};
pub use edid::{Edid, EdidError};

/// Código VCP de la entrada activa (MCCS "Input Select").
pub const VCP_INPUT_SOURCE: u8 = 0x60;
/// Código VCP del modo de energía: escribirlo puede apagar la imagen.
pub const VCP_POWER_MODE: u8 = 0xD6;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DdcError {
    #[error("no hay monitor con índice {0}")]
    NoSuchMonitor(usize),
    /// Fallo del sistema. Los Samsung devuelven `0xC0262589` al azar en lecturas sueltas,
    /// así que un fallo aislado no significa que el monitor no haya obedecido.
    #[error("{op} falló: {message} ({code:#010X})")]
    Os {
        op: &'static str,
        code: u32,
        message: String,
    },
    #[error("DDC/CI no está disponible en esta plataforma")]
    Unsupported,
    /// Un plan de cambio que no garantiza la vuelta no se ejecuta.
    #[error(transparent)]
    InvalidPlan(#[from] switch::PlanError),
}

/// Monitor físico tal como se enumeró.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    pub index: usize,
    /// Descripción del sistema (en Windows suele ser "Monitor PnP genérico").
    pub description: String,
    /// Nombre de la salida del escritorio, por ejemplo `\\.\DISPLAY1`.
    pub device: String,
    pub edid: Option<Edid>,
}

impl MonitorInfo {
    /// Nombre del modelo según el EDID, o la descripción del sistema si no hay EDID.
    pub fn model(&self) -> &str {
        self.edid
            .as_ref()
            .and_then(|e| e.name.as_deref())
            .unwrap_or(&self.description)
    }

    /// ¿Coincide con un texto de búsqueda (modelo, id PnP, descripción o salida)?
    pub fn matches(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        let mut fields = vec![self.description.to_lowercase(), self.device.to_lowercase()];
        if let Some(e) = &self.edid {
            fields.push(e.pnp_id().to_lowercase());
            if let Some(name) = &e.name {
                fields.push(name.to_lowercase());
            }
            if let Some(serial) = &e.serial_text {
                fields.push(serial.to_lowercase());
            }
        }
        fields.iter().any(|f| f.contains(&q))
    }
}

impl fmt::Display for MonitorInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.index, self.model())?;
        if let Some(e) = &self.edid {
            write!(f, " ({}", e.pnp_id())?;
            if let Some(serial) = &e.serial_text {
                write!(f, ", serie {serial}")?;
            }
            write!(f, ")")?;
        }
        if !self.device.is_empty() {
            write!(f, " en {}", self.device)?;
        }
        Ok(())
    }
}

/// Respuesta a una lectura de VCP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VcpValue {
    pub current: u32,
    pub maximum: u32,
}

/// Acceso DDC/CI a los monitores físicos. Los índices son los de [`DdcBus::monitors`].
pub trait DdcBus {
    fn monitors(&self) -> &[MonitorInfo];
    fn capabilities(&mut self, monitor: usize) -> Result<String, DdcError>;
    fn get_vcp(&mut self, monitor: usize, code: u8) -> Result<VcpValue, DdcError>;
    fn set_vcp(&mut self, monitor: usize, code: u8, value: u32) -> Result<(), DdcError>;
}
