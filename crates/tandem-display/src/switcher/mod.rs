//! Cambio de la entrada de un monitor durante un cambio de modo (DESIGN §8).
//!
//! El orquestador no sabe cómo se cambia una entrada: pide un [`SwitchRequest`] a un
//! [`MonitorSwitcher`] y espera. Backends:
//!
//! - [`DdcSwitcher`]: DDC/CI con el perfil del monitor (`ddc-windows` en la PC).
//! - [`ManualSwitcher`]: le pide al usuario que cambie la entrada con los botones del
//!   monitor y espera su confirmación desde la UI.
//! - [`PerMonitor`]: elige uno de los dos según el monitor tenga perfil DDC o no.
//!
//! A diferencia de `tandem-ddc switch`, aquí no hay vuelta automática: la red de
//! seguridad es la vuelta atrás de la transacción, que manda el cambio inverso.

mod ddc;
mod manual;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tandem_core::{Input, Machine, MonitorConfig, MonitorId};

use crate::ddc::DdcError;

pub use ddc::{BusFactory, DdcSwitcher};
pub use manual::{ManualEvent, ManualPrompt, ManualSwitcher, UnknownPrompt};

/// Qué monitor cambiar y a qué entrada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchRequest {
    pub monitor: MonitorConfig,
    /// La máquina que va a mostrar el monitor.
    pub to: Machine,
    /// La entrada cableada a `to`.
    pub input: Input,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SwitchError {
    #[error("{0}: falta `edid_model` en la configuración para encontrarlo por DDC")]
    NoEdidModel(MonitorId),
    #[error("{0}: no tiene perfil `ddc` en la configuración")]
    NoProfile(MonitorId),
    #[error("{monitor}: ningún monitor conectado coincide con `{model}`")]
    NotFound { monitor: MonitorId, model: String },
    #[error(
        "{monitor}: `{model}` coincide con {count} monitores; hace falta un modelo más preciso"
    )]
    Ambiguous {
        monitor: MonitorId,
        model: String,
        count: usize,
    },
    #[error("{monitor}: {error}")]
    Ddc { monitor: MonitorId, error: DdcError },
    #[error("{0}: no hay una UI conectada para pedir el cambio manual")]
    NoUi(MonitorId),
    #[error("{0}: el usuario no confirmó el cambio manual")]
    Declined(MonitorId),
    #[error("{monitor}: {message}")]
    Internal { monitor: MonitorId, message: String },
}

/// Resultado de un cambio de entrada.
pub type SwitchFuture<'a> = Pin<Box<dyn Future<Output = Result<(), SwitchError>> + Send + 'a>>;

/// Un mecanismo para cambiar la entrada de un monitor.
///
/// Termina cuando el cambio está hecho (o confirmado, en el manual). Se puede cancelar
/// soltando el futuro: así lo corta el tiempo máximo del paso.
pub trait MonitorSwitcher: Send + Sync {
    fn switch<'a>(&'a self, req: &'a SwitchRequest) -> SwitchFuture<'a>;
}

/// Monitores con perfil `ddc` por DDC/CI; los demás, a mano.
///
/// El perfil se escribe en la configuración solo cuando se probaron los códigos que el
/// monitor obedece (T-050), así que sin perfil no se manda nada por el bus.
pub struct PerMonitor {
    pub ddc: Arc<dyn MonitorSwitcher>,
    pub manual: Arc<dyn MonitorSwitcher>,
}

impl MonitorSwitcher for PerMonitor {
    fn switch<'a>(&'a self, req: &'a SwitchRequest) -> SwitchFuture<'a> {
        if req.monitor.ddc.is_some() {
            self.ddc.switch(req)
        } else {
            self.manual.switch(req)
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Mutex, PoisonError};

    use tandem_core::{Config, DdcProfile};

    use super::*;

    pub fn desk() -> Config {
        Config::from_toml(
            r#"
            [[monitors]]
            id = "m34"
            label = '34"'
            edid_model = "LC34G55T"
            wiring = { pc = "displayport", mac = "hdmi" }
            ddc = { hdmi = 0x06, display_port = 0x0F }

            [[monitors]]
            id = "m32"
            label = '32"'
            wiring = { pc = "displayport", mac = "hdmi" }
            "#,
        )
        .unwrap()
    }

    pub fn request(monitor: &str, to: Machine) -> SwitchRequest {
        let monitor = desk().monitor(&monitor.into()).unwrap().clone();
        let input = monitor.input_for(to).unwrap();
        SwitchRequest { monitor, to, input }
    }

    /// Registra los monitores que se le piden cambiar.
    #[derive(Default)]
    pub struct Recorder(pub Mutex<Vec<MonitorId>>);

    impl MonitorSwitcher for Recorder {
        fn switch<'a>(&'a self, req: &'a SwitchRequest) -> SwitchFuture<'a> {
            Box::pin(async move {
                self.0
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(req.monitor.id.clone());
                Ok(())
            })
        }
    }

    impl Recorder {
        pub fn seen(&self) -> Vec<MonitorId> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    #[tokio::test]
    async fn per_monitor_uses_ddc_only_with_a_profile() {
        let (ddc, manual) = (Arc::new(Recorder::default()), Arc::new(Recorder::default()));
        let s = PerMonitor {
            ddc: ddc.clone(),
            manual: manual.clone(),
        };
        s.switch(&request("m34", Machine::Mac)).await.unwrap();
        s.switch(&request("m32", Machine::Mac)).await.unwrap();
        let mut with_profile = request("m32", Machine::Pc);
        with_profile.monitor.ddc = Some(DdcProfile {
            hdmi: 0x11,
            display_port: 0x0F,
        });
        s.switch(&with_profile).await.unwrap();
        assert_eq!(ddc.seen(), [MonitorId::from("m34"), "m32".into()]);
        assert_eq!(manual.seen(), [MonitorId::from("m32")]);
    }
}
