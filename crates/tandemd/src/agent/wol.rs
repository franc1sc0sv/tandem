//! Wake-on-LAN from this machine to the other, without editing `tandem.toml` by hand (T-159): the
//! other's MAC arrives in its capabilities on connecting and is saved on its own; the UI can also
//! write or delete it.

use std::sync::atomic::Ordering;

use tandem_core::Os;
use tandem_power::{MacAddr, WolTarget};
use tandem_proto::{PowerCaps, Reply};

use super::{Agent, PoisonError};
use crate::capabilities;

impl Agent {
    /// Whom this machine wakes now.
    pub fn wol(&self) -> Option<WolTarget> {
        self.wol
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// This machine's power with the Wake-on-LAN that holds now.
    pub(super) fn power_caps(&self, os: Os) -> PowerCaps {
        capabilities::with_wol(capabilities::power(&self.cfg, os), self.wol().as_ref())
    }

    /// The other machine announced its card's MAC: if there is nothing yet to wake it with,
    /// it is saved. A MAC set by hand or already learned is not overwritten.
    pub(super) fn learn_wol(&self, mac: Option<&str>) {
        let Some(mac) = mac.and_then(|m| m.parse::<MacAddr>().ok()) else {
            return;
        };
        if self.wol().is_some() || self.wol_cleared.load(Ordering::Relaxed) {
            return;
        }
        match self.apply_wol(Some(mac)) {
            Ok(()) => tracing::info!(%mac, "the other machine's MAC learned for Wake-on-LAN"),
            Err(e) => tracing::warn!("could not save the other machine's MAC: {e}"),
        }
    }

    /// `UiCommand::SetWol`: the MAC this machine wakes the other with.
    pub(super) fn set_wol_mac(&self, mac: Option<&str>) -> Reply {
        let mac = match mac.map(str::trim).filter(|m| !m.is_empty()) {
            Some(text) => match text.parse::<MacAddr>() {
                Ok(mac) => Some(mac),
                Err(e) => return Reply::Rejected(e.to_string()),
            },
            None => None,
        };
        match self.apply_wol(mac) {
            Ok(()) => {
                // Deleting it is a choice: the next announcement from the other does not undo it.
                self.wol_cleared.store(mac.is_none(), Ordering::Relaxed);
                Reply::Done
            }
            Err(e) => Reply::Rejected(e),
        }
    }

    /// Changes the MAC in memory, in the power steps and in `tandem.toml`, and announces it.
    fn apply_wol(&self, mac: Option<MacAddr>) -> Result<(), String> {
        let _editing = self.edit();
        if let Some(path) = self.config_path() {
            crate::config_edit::set_wol_mac(&path, mac).map_err(|e| e.to_string())?;
        }
        let target = {
            let mut slot = self.wol.write().unwrap_or_else(PoisonError::into_inner);
            // The rest of `[power.wol]` is kept (source, destination, port).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentConfig, StateStore};

    fn agent(dir: &tempfile::TempDir, extra: &str) -> Agent {
        let path = dir.path().join("tandem.toml");
        if !extra.is_empty() || !path.exists() {
            std::fs::write(&path, format!("machine = \"a\"\n{extra}")).unwrap();
        }
        let cfg = AgentConfig::load(&path).unwrap();
        let a = Agent::new(cfg, StateStore::new(dir.path().join("state.json")));
        a.set_config_file(path);
        a
    }

    #[test]
    fn a_cleared_mac_is_not_learned_again() {
        let dir = tempfile::tempdir().unwrap();
        let a = agent(&dir, "");
        a.learn_wol(Some("02:00:00:00:00:02"));
        assert!(a.wol().is_some());
        assert_eq!(a.set_wol_mac(None), Reply::Done);
        a.learn_wol(Some("02:00:00:00:00:02"));
        assert_eq!(a.wol(), None);
        // And it stays that way after restart, through `[power.wol] cleared`.
        let b = agent(&dir, "");
        b.learn_wol(Some("02:00:00:00:00:02"));
        assert_eq!(b.wol(), None);
        // Writing one by hand brings it back.
        assert_eq!(b.set_wol_mac(Some("aa:bb:cc:dd:ee:ff")), Reply::Done);
        assert!(b.wol().is_some());
    }
}
