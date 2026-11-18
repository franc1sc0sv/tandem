//! Wake-on-LAN de esta máquina hacia la otra, sin editar `tandem.toml` a mano (T-159): la
//! MAC de la otra llega en sus capacidades al conectarse y se guarda sola; la UI también la
//! puede escribir o borrar.

use tandem_core::Os;
use tandem_power::{MacAddr, WolTarget};
use tandem_proto::{PowerCaps, Reply};

use super::{Agent, PoisonError};
use crate::capabilities;

impl Agent {
    /// A quién despierta esta máquina ahora.
    pub fn wol(&self) -> Option<WolTarget> {
        self.wol
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// La energía de esta máquina con el Wake-on-LAN que vale ahora.
    pub(super) fn power_caps(&self, os: Os) -> PowerCaps {
        capabilities::with_wol(capabilities::power(&self.cfg, os), self.wol().as_ref())
    }

    /// La otra máquina anunció la MAC de su tarjeta: si todavía no hay con qué despertarla,
    /// se guarda. Una MAC puesta a mano o ya aprendida no se pisa.
    pub(super) fn learn_wol(&self, mac: Option<&str>) {
        let Some(mac) = mac.and_then(|m| m.parse::<MacAddr>().ok()) else {
            return;
        };
        if self.wol().is_some() {
            return;
        }
        match self.apply_wol(Some(mac)) {
            Ok(()) => tracing::info!(%mac, "MAC de la otra máquina aprendida para Wake-on-LAN"),
            Err(e) => tracing::warn!("no se pudo guardar la MAC de la otra máquina: {e}"),
        }
    }

    /// `UiCommand::SetWol`: la MAC con que esta máquina despierta a la otra.
    pub(super) fn set_wol_mac(&self, mac: Option<&str>) -> Reply {
        let mac = match mac.map(str::trim).filter(|m| !m.is_empty()) {
            Some(text) => match text.parse::<MacAddr>() {
                Ok(mac) => Some(mac),
                Err(e) => return Reply::Rejected(e.to_string()),
            },
            None => None,
        };
        match self.apply_wol(mac) {
            Ok(()) => Reply::Done,
            Err(e) => Reply::Rejected(e),
        }
    }

    /// Cambia la MAC en memoria, en los pasos de energía y en `tandem.toml`, y lo anuncia.
    fn apply_wol(&self, mac: Option<MacAddr>) -> Result<(), String> {
        let _editing = self.edit();
        if let Some(path) = self.config_path() {
            crate::config_edit::set_wol_mac(&path, mac).map_err(|e| e.to_string())?;
        }
        let target = {
            let mut slot = self.wol.write().unwrap_or_else(PoisonError::into_inner);
            // Se conserva lo demás de `[power.wol]` (origen, destino, puerto).
            let old = slot.take();
            *slot = mac.map(|mac| match old {
                Some(old) => WolTarget { mac, ..old },
                None => WolTarget::new(mac),
            });
            slot.clone()
        };
        self.orch.set_wol(target);
        self.refresh_capabilities();
        Ok(())
    }
}
