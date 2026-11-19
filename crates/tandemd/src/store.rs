//! The [`State`] persisted as JSON: what the agent knows on start (which machines
//! were on).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tandem_core::{Machine, State};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("could not read or write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the state saved at {path} is damaged: {source}")]
    Corrupt {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("could not serialize the state: {0}")]
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

    /// `None` if it was never saved yet.
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

    /// Writes to a temporary file and renames it: a cut midway does not leave a half JSON.
    ///
    /// Each save uses its own temporary: two saves at once do not trample each other, and the
    /// rename leaves whole the last one that finishes.
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
            // Best effort: the error that matters is the write's.
            let _ = std::fs::remove_file(&tmp);
            return Err(self.io(e));
        }
        Ok(())
    }

    /// The startup state.
    ///
    /// If nothing is saved, the safe starting state. If the file is damaged,
    /// it is set aside as `*.corrupt` (so it can be looked at) and it also starts from the safe state.
    pub fn load_or_initial(&self, me: Machine) -> State {
        match self.load() {
            Ok(Some(mut s)) => {
                // Scene ids from before T-209: translated and written back once.
                if s.migrate_legacy_ids()
                    && let Err(e) = self.save(&s)
                {
                    tracing::warn!("{e}; the migrated state was not written back");
                }
                s
            }
            Ok(None) => State::initial(me),
            Err(e) => {
                tracing::warn!("{e}; starting from the initial state");
                if matches!(e, StoreError::Corrupt { .. }) {
                    let aside = self.sibling("corrupt");
                    if let Err(e) = std::fs::rename(&self.path, &aside) {
                        tracing::warn!("could not set aside {}: {e}", self.path.display());
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
            scene: None,
        }
    }

    #[test]
    fn saves_and_reloads_the_same_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::new(dir.path().join("state.json"));
        store.save(&sample()).unwrap();
        assert_eq!(store.load().unwrap(), Some(sample()));
        // A new agent over the same file starts where the previous one left off.
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

    /// T-209: the scene id saved by an earlier release is translated and written back once.
    #[test]
    fn an_old_scene_id_is_migrated_and_written_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::new(dir.path().join("state.json"));
        std::fs::write(
            store.path(),
            r#"{"on":{"a":true,"b":true},"scene":"solo-b"}"#,
        )
        .unwrap();
        let loaded = store.load_or_initial(Machine::A);
        assert_eq!(loaded.scene.as_deref(), Some("only-b"));
        // Written back: the file already says the new one.
        assert_eq!(
            store.load().unwrap().unwrap().scene.as_deref(),
            Some("only-b")
        );
        let text = std::fs::read_to_string(store.path()).unwrap();
        assert!(!text.contains("solo"), "{text}");
        // Idempotent, and a new state is untouched.
        assert_eq!(store.load_or_initial(Machine::A), loaded);
        assert_eq!(std::fs::read_to_string(store.path()).unwrap(), text);
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

    /// A state saved by v0.1 (with the monitors) still loads.
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

    /// One from before sides A/B (`mac`/`pc`) loads without guessing which was which: nothing
    /// on, and the agent sets its own and the other according to the links.
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

    // QA round 1: all writes used the same `state.json.tmp`. Two saves
    // at once (the agent is shared between connections) trampled the temporary: one
    // renamed the other's half-written file and the second failed with NotFound.
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
        (
            any::<bool>(),
            any::<bool>(),
            proptest::option::of("[a-z-]{1,8}"),
        )
            .prop_map(|(mac, pc, scene)| State {
                on: tandem_core::Powered { a: mac, b: pc },
                scene,
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
