//! El [`State`] persistido en JSON.
//!
//! Los monitores no dicen de verdad qué entrada muestran (DESIGN §3), así que lo que se
//! guarda aquí es lo único que sabe el agente al arrancar.

use std::io;
use std::path::{Path, PathBuf};

use tandem_core::{Config, Machine, State};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("no se pudo leer o escribir {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("el estado guardado en {path} está dañado: {source}")]
    Corrupt {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("no se pudo serializar el estado: {0}")]
    Serialize(#[source] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct StateStore {
    path: PathBuf,
}

impl StateStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        StateStore { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `None` si todavía no se guardó nunca.
    pub fn load(&self) -> Result<Option<State>, StoreError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(self.io(source)),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| StoreError::Corrupt {
                path: self.path.clone(),
                source,
            })
    }

    /// Escribe a un archivo temporal y lo renombra: un corte a mitad no deja un JSON a medias.
    pub fn save(&self, state: &State) -> Result<(), StoreError> {
        if let Some(dir) = self.path.parent() {
            crate::paths::create_private_dir(dir).map_err(|e| self.io(e))?;
        }
        let json = serde_json::to_vec_pretty(state).map_err(StoreError::Serialize)?;
        let tmp = self.sibling("tmp");
        std::fs::write(&tmp, json).map_err(|e| self.io(e))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| self.io(e))
    }

    /// El estado de arranque, ajustado a la configuración actual.
    ///
    /// Si no hay nada guardado, el estado seguro de partida. Si el archivo está dañado,
    /// se aparta como `*.corrupt` (para poder mirarlo) y también se parte del estado seguro.
    pub fn load_or_initial(&self, cfg: &Config) -> State {
        match self.load() {
            Ok(Some(s)) => reconcile(s, cfg),
            Ok(None) => State::initial(cfg),
            Err(e) => {
                tracing::warn!("{e}; se parte del estado inicial");
                if matches!(e, StoreError::Corrupt { .. }) {
                    let aside = self.sibling("corrupt");
                    if let Err(e) = std::fs::rename(&self.path, &aside) {
                        tracing::warn!("no se pudo apartar {}: {e}", self.path.display());
                    }
                }
                State::initial(cfg)
            }
        }
    }

    fn sibling(&self, ext: &str) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(".");
        name.push(ext);
        PathBuf::from(name)
    }

    fn io(&self, source: io::Error) -> StoreError {
        StoreError::Io {
            path: self.path.clone(),
            source,
        }
    }
}

/// Ajusta un estado guardado a la configuración: se olvidan los monitores que ya no
/// están y los nuevos se suponen en la Mac, como en [`State::initial`].
pub fn reconcile(mut state: State, cfg: &Config) -> State {
    state.owner.retain(|id, _| cfg.monitor(id).is_some());
    for m in &cfg.monitors {
        state.owner.entry(m.id.clone()).or_insert(Machine::Mac);
    }
    state
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proptest::prelude::*;
    use tandem_core::{BuiltinDisplay, MonitorId};

    use super::*;
    use crate::config::fixtures;

    fn sample() -> State {
        State {
            owner: BTreeMap::from([("m34".into(), Machine::Pc), ("m32".into(), Machine::Mac)]),
            pc_on: true,
            builtin_pref: BuiltinDisplay::Mirror,
        }
    }

    #[test]
    fn saves_and_reloads_the_same_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::new(dir.path().join("state.json"));
        store.save(&sample()).unwrap();
        assert_eq!(store.load().unwrap(), Some(sample()));
        // Un agente nuevo sobre el mismo archivo arranca donde quedó el anterior.
        let again = StateStore::new(store.path());
        assert_eq!(again.load_or_initial(&fixtures::desk().desk), sample());
    }

    #[test]
    fn save_overwrites_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::new(dir.path().join("state.json"));
        store.save(&sample()).unwrap();
        let mut other = sample();
        other.pc_on = false;
        store.save(&other).unwrap();
        assert_eq!(store.load().unwrap(), Some(other));
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["state.json"]);
    }

    #[test]
    fn save_creates_missing_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::new(dir.path().join("a/b/state.json"));
        store.save(&sample()).unwrap();
        assert_eq!(store.load().unwrap(), Some(sample()));
    }

    #[test]
    fn missing_file_is_none_and_starts_initial() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::new(dir.path().join("state.json"));
        assert_eq!(store.load().unwrap(), None);
        let cfg = fixtures::desk().desk;
        assert_eq!(store.load_or_initial(&cfg), State::initial(&cfg));
    }

    #[test]
    fn corrupt_file_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, b"{ no es json").unwrap();
        let store = StateStore::new(&path);
        assert!(matches!(store.load(), Err(StoreError::Corrupt { .. })));
        let cfg = fixtures::desk().desk;
        assert_eq!(store.load_or_initial(&cfg), State::initial(&cfg));
        assert!(!path.exists());
        assert!(dir.path().join("state.json.corrupt").exists());
    }

    #[test]
    fn reconcile_follows_config() {
        let cfg = fixtures::desk().desk;
        let mut s = sample();
        s.owner.remove(&MonitorId::from("m32"));
        s.owner.insert("viejo".into(), Machine::Pc);
        let r = reconcile(s, &cfg);
        assert_eq!(
            r.owner,
            BTreeMap::from([("m34".into(), Machine::Pc), ("m32".into(), Machine::Mac)])
        );
        assert!(r.pc_on);
        assert_eq!(r.builtin_pref, BuiltinDisplay::Mirror);
    }

    fn machine() -> impl Strategy<Value = Machine> {
        prop_oneof![Just(Machine::Mac), Just(Machine::Pc)]
    }

    fn builtin() -> impl Strategy<Value = BuiltinDisplay> {
        prop_oneof![
            Just(BuiltinDisplay::Off),
            Just(BuiltinDisplay::Mirror),
            Just(BuiltinDisplay::Extend),
        ]
    }

    fn state() -> impl Strategy<Value = State> {
        (
            prop::collection::btree_map(any::<String>().prop_map(MonitorId), machine(), 0..6),
            any::<bool>(),
            builtin(),
        )
            .prop_map(|(owner, pc_on, builtin_pref)| State {
                owner,
                pc_on,
                builtin_pref,
            })
    }

    proptest! {
        #[test]
        fn any_state_round_trips(s in state()) {
            let dir = tempfile::tempdir().unwrap();
            let store = StateStore::new(dir.path().join("state.json"));
            store.save(&s).unwrap();
            prop_assert_eq!(store.load().unwrap(), Some(s));
        }

        #[test]
        fn reconciled_owner_matches_config_monitors(s in state()) {
            let cfg = fixtures::desk().desk;
            let r = reconcile(s.clone(), &cfg);
            let ids: Vec<_> = r.owner.keys().cloned().collect();
            let mut want: Vec<_> = cfg.monitors.iter().map(|m| m.id.clone()).collect();
            want.sort();
            prop_assert_eq!(ids, want);
            // Lo que ya estaba asignado se respeta.
            for (id, owner) in &r.owner {
                if let Some(prev) = s.owner.get(id) {
                    prop_assert_eq!(owner, prev);
                }
            }
        }
    }
}
