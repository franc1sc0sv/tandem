//! El [`State`] persistido en JSON: lo que sabe el agente al arrancar (qué máquinas
//! estaban encendidas).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tandem_core::{Machine, State};

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
    ///
    /// Cada guardado usa su propio temporal: dos guardados a la vez no se pisan, y el
    /// rename deja entero el último que termina.
    pub fn save(&self, state: &State) -> Result<(), StoreError> {
        static NEXT_TMP: AtomicU64 = AtomicU64::new(0);
        if let Some(dir) = self.path.parent() {
            crate::paths::create_private_dir(dir).map_err(|e| self.io(e))?;
        }
        let json = serde_json::to_vec_pretty(state).map_err(StoreError::Serialize)?;
        let n = NEXT_TMP.fetch_add(1, Ordering::Relaxed);
        let tmp = self.sibling(&format!("tmp-{}-{n}", std::process::id()));
        let result = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, &self.path));
        if let Err(e) = result {
            // Mejor esfuerzo: el error que importa es el de la escritura.
            let _ = std::fs::remove_file(&tmp);
            return Err(self.io(e));
        }
        Ok(())
    }

    /// El estado de arranque.
    ///
    /// Si no hay nada guardado, el estado seguro de partida. Si el archivo está dañado,
    /// se aparta como `*.corrupt` (para poder mirarlo) y también se parte del estado seguro.
    pub fn load_or_initial(&self, me: Machine) -> State {
        match self.load() {
            Ok(Some(s)) => s,
            Ok(None) => State::initial(me),
            Err(e) => {
                tracing::warn!("{e}; se parte del estado inicial");
                if matches!(e, StoreError::Corrupt { .. }) {
                    let aside = self.sibling("corrupt");
                    if let Err(e) = std::fs::rename(&self.path, &aside) {
                        tracing::warn!("no se pudo apartar {}: {e}", self.path.display());
                    }
                }
                State::initial(me)
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

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn sample() -> State {
        State {
            on: tandem_core::Powered::both(),
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
        assert_eq!(again.load_or_initial(Machine::A), sample());
    }

    #[test]
    fn save_overwrites_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::new(dir.path().join("state.json"));
        store.save(&sample()).unwrap();
        let mut other = sample();
        other.on.b = false;
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
        assert_eq!(
            store.load_or_initial(Machine::A),
            State::initial(Machine::A)
        );
    }

    #[test]
    fn corrupt_file_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, b"{ no es json").unwrap();
        let store = StateStore::new(&path);
        assert!(matches!(store.load(), Err(StoreError::Corrupt { .. })));
        assert_eq!(
            store.load_or_initial(Machine::B),
            State::initial(Machine::B)
        );
        assert!(!path.exists());
        assert!(dir.path().join("state.json.corrupt").exists());
    }

    /// Un estado guardado por v0.1 (con los monitores) sigue cargando.
    #[test]
    fn a_state_saved_with_monitors_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(
            &path,
            r#"{"owner":{"m34":"b"},"on":{"a":true,"b":true},"builtin_pref":"off","doubt":{}}"#,
        )
        .unwrap();
        let store = StateStore::new(&path);
        assert_eq!(store.load_or_initial(Machine::A), sample());
    }

    /// Uno de antes de los lados A/B (`mac`/`pc`) carga sin adivinar cuál era cuál: nada
    /// prendido, y el agente pone la suya y la otra según los enlaces.
    #[test]
    fn a_state_saved_with_mac_and_pc_loads_with_nothing_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, r#"{"on":{"mac":true,"pc":true}}"#).unwrap();
        let store = StateStore::new(&path);
        assert_eq!(
            store.load_or_initial(Machine::A).on,
            tandem_core::Powered::default()
        );
    }

    // QA ronda 1: todas las escrituras usaban el mismo `state.json.tmp`. Dos guardados
    // a la vez (el agente se comparte entre conexiones) se pisaban el temporal: uno
    // renombraba el archivo a medio escribir del otro y el segundo fallaba con NotFound.
    #[test]
    fn concurrent_saves_never_fail_or_tear_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::new(dir.path().join("state.json"));
        let states: Vec<State> = (0..8)
            .map(|i| {
                let mut s = sample();
                s.on.b = i % 2 == 0;
                s.on.a = i % 3 == 0;
                s
            })
            .collect();
        std::thread::scope(|scope| {
            for s in &states {
                let store = store.clone();
                scope.spawn(move || {
                    for _ in 0..40 {
                        store.save(s).unwrap();
                    }
                });
            }
        });
        let last = store.load().unwrap().unwrap();
        assert!(states.contains(&last));
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["state.json"]);
    }

    fn state() -> impl Strategy<Value = State> {
        (any::<bool>(), any::<bool>()).prop_map(|(mac, pc)| State {
            on: tandem_core::Powered { a: mac, b: pc },
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
    }
}
