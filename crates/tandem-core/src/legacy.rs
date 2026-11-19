//! Identifiers persisted by releases before T-209, when they were still Spanish: the default
//! scene ids (`trabajo`, `solo-a`, `solo-b`) and names ("Trabajo", "Solo <machine>"), the
//! icon `puesto` and the detected screen ids (`pantalla-N`).
//!
//! They are mapped to the English ones when reading the configuration and the stored state, and the
//! agent writes the result back once. The old ids are still understood for one release; after that
//! this module goes away. Mapping is idempotent: what is already new is left alone.

use crate::model::{Desk, Machine};
use crate::state::State;

/// Scene ids from before T-209 and their replacement.
const SCENE_IDS: [(&str, &str); 3] = [
    ("trabajo", "work"),
    ("solo-a", "only-a"),
    ("solo-b", "only-b"),
];
const OLD_ICON: &str = "puesto";
const OLD_SCREEN_PREFIX: &str = "pantalla-";
const NEW_SCREEN_PREFIX: &str = "screen-";

/// The new id of a scene saved with a Spanish id, or `None` if `id` is not one of them.
pub fn scene_id(id: &str) -> Option<&'static str> {
    SCENE_IDS
        .iter()
        .find(|(old, _)| *old == id)
        .map(|(_, new)| *new)
}

/// The new id of a screen detected as `pantalla-N`; `None` for any other.
pub fn screen_id(id: &str) -> Option<String> {
    let n = id.strip_prefix(OLD_SCREEN_PREFIX)?;
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .then(|| format!("{NEW_SCREEN_PREFIX}{n}"))
}

/// Maps a saved scene id in place. `true` if it changed.
fn map_scene_id(id: &mut String) -> bool {
    match scene_id(id) {
        Some(new) => {
            *id = new.to_owned();
            true
        }
        None => false,
    }
}

impl Desk {
    /// Replaces the Spanish identifiers a desk saved by an earlier release carries (scene ids,
    /// default scene names, the `puesto` icon and `pantalla-N` screen ids, also in `[[cables]]`) with
    /// the English ones. `true` if anything changed, so the caller writes it back.
    ///
    /// A scene is not renamed if its new id is already taken, so ids stay unique. A name is only
    /// translated if it is exactly the default one: what the user typed is respected.
    pub fn migrate_legacy_ids(&mut self) -> bool {
        let mut changed = false;
        let taken: Vec<String> = self.scenes.iter().map(|s| s.id.clone()).collect();
        let machines = self.machines.clone();
        for s in &mut self.scenes {
            if scene_id(&s.id).is_some_and(|new| !taken.iter().any(|t| t == new)) {
                changed |= map_scene_id(&mut s.id);
            }
            if s.icon == OLD_ICON {
                s.icon = crate::scene::DEFAULT_ICON.to_owned();
                changed = true;
            }
            if s.name == "Trabajo" && s.require_on.is_none() {
                s.name = "Work".to_owned();
                changed = true;
            } else if let Some(side) = s.require_on
                && s.name == format!("Solo {}", machines.name(side))
            {
                s.name = format!("Only {}", machines.name(side));
                changed = true;
            }
        }
        for screen in &mut self.screens {
            if let Some(new) = screen_id(&screen.id) {
                screen.id = new;
                changed = true;
            }
        }
        for cable in &mut self.cables {
            for side in Machine::ALL {
                let end = cable.end_mut(side);
                if let Some(new) = end.screen.as_deref().and_then(screen_id) {
                    end.screen = Some(new);
                    changed = true;
                }
            }
        }
        changed
    }
}

impl State {
    /// Replaces the scene id saved by an earlier release with the English one. `true` if it changed.
    pub fn migrate_legacy_ids(&mut self) -> bool {
        self.scene.as_mut().is_some_and(map_scene_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MachineInfo, Machines, Os, ScreenSpec};
    use crate::monitor::MonitorCable;
    use crate::scene::Scene;

    fn old_desk() -> Desk {
        let mut desk = Desk {
            machines: Machines {
                a: Some(MachineInfo::new(
                    Machine::A,
                    Os::MacOs,
                    Some("MacBook".into()),
                )),
                b: Some(MachineInfo::new(Machine::B, Os::Windows, Some("PC".into()))),
            },
            ..Desk::default()
        };
        let mut work = Scene::new("trabajo", "Trabajo");
        work.icon = "puesto".into();
        let mut a = Scene::new("solo-a", "Solo MacBook");
        a.require_on = Some(Machine::A);
        let mut b = Scene::new("solo-b", "Solo PC");
        b.require_on = Some(Machine::B);
        desk.scenes = vec![work, a, b];
        for (machine, id) in [(Machine::A, "pantalla-1"), (Machine::B, "pantalla-2")] {
            desk.screens.push(ScreenSpec {
                machine,
                id: id.into(),
                x: 0,
                y: 0,
                width: 100,
                height: 100,
                scale: Default::default(),
            });
        }
        let mut cable = MonitorCable::new("lc34-01");
        cable.a.screen = Some("pantalla-1".into());
        desk.cables.push(cable);
        desk
    }

    #[test]
    fn an_old_desk_gets_the_english_identifiers() {
        let mut desk = old_desk();
        assert!(desk.migrate_legacy_ids());
        let ids: Vec<_> = desk.scenes.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["work", "only-a", "only-b"]);
        let names: Vec<_> = desk.scenes.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Work", "Only MacBook", "Only PC"]);
        assert_eq!(desk.scenes[0].icon, "desk");
        assert_eq!(desk.screens[0].id, "screen-1");
        assert_eq!(desk.screens[1].id, "screen-2");
        assert_eq!(desk.cables[0].a.screen.as_deref(), Some("screen-1"));
    }

    #[test]
    fn migrating_twice_changes_nothing_more() {
        let mut desk = old_desk();
        assert!(desk.migrate_legacy_ids());
        let once = desk.clone();
        assert!(!desk.migrate_legacy_ids());
        assert_eq!(desk, once);
    }

    #[test]
    fn a_new_desk_is_left_untouched() {
        let mut desk = old_desk();
        desk.migrate_legacy_ids();
        let new = desk.clone();
        let mut again = new.clone();
        assert!(!again.migrate_legacy_ids());
        assert_eq!(again, new);
    }

    #[test]
    fn names_the_user_chose_are_respected() {
        let mut desk = old_desk();
        desk.scenes[0].name = "Trabajo duro".into();
        desk.scenes[1].name = "Solo yo".into();
        assert!(desk.migrate_legacy_ids());
        assert_eq!(desk.scenes[0].name, "Trabajo duro");
        assert_eq!(desk.scenes[1].name, "Solo yo");
        assert_eq!(desk.scenes[0].id, "work");
    }

    #[test]
    fn an_id_already_taken_is_not_duplicated() {
        let mut desk = old_desk();
        desk.scenes.push(Scene::new("work", "My work"));
        desk.migrate_legacy_ids();
        let mut ids: Vec<_> = desk.scenes.iter().map(|s| s.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), desk.scenes.len());
        assert_eq!(desk.scenes[0].id, "trabajo");
    }

    #[test]
    fn the_stored_state_maps_its_scene() {
        for (old, new) in [
            ("trabajo", "work"),
            ("solo-a", "only-a"),
            ("solo-b", "only-b"),
        ] {
            let mut s = State::initial(Machine::A);
            s.scene = Some(old.into());
            assert!(s.migrate_legacy_ids());
            assert_eq!(s.scene.as_deref(), Some(new));
            assert!(!s.migrate_legacy_ids());
        }
        let mut s = State::initial(Machine::A);
        assert!(!s.migrate_legacy_ids());
        s.scene = Some("game".into());
        assert!(!s.migrate_legacy_ids());
    }

    #[test]
    fn only_numbered_screens_map() {
        assert_eq!(screen_id("pantalla-3").as_deref(), Some("screen-3"));
        assert_eq!(screen_id("pantalla-"), None);
        assert_eq!(screen_id("pantalla-x"), None);
        assert_eq!(screen_id("screen-3"), None);
    }
}
