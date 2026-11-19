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

/// What [`Desk::migrate_legacy_ids`] did: whether the desk changed and which scenes it renamed.
///
/// The stored state follows the desk with it ([`State::migrate_legacy_ids`]): its scene is only
/// renamed if the desk really renamed that scene, never just because the old id looks like a default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyRenames {
    changed: bool,
    scenes: Vec<(String, &'static str)>,
}

impl LegacyRenames {
    /// Something in the desk changed, so the caller writes it back.
    pub fn changed(&self) -> bool {
        self.changed
    }

    /// The new id the desk gave the scene `old` in this pass, if it renamed it.
    pub fn scene(&self, old: &str) -> Option<&'static str> {
        self.scenes
            .iter()
            .find(|(o, _)| o == old)
            .map(|(_, new)| *new)
    }
}

impl Desk {
    /// Replaces the Spanish identifiers a desk saved by an earlier release carries (scene ids,
    /// default scene names, the `puesto` icon and `pantalla-N` screen ids, also in `[[cables]]`) with
    /// the English ones, and says what it renamed so the caller writes it back.
    ///
    /// A scene or screen is not renamed if its new id is already taken (on that machine, for a
    /// screen), so ids stay unique and the desk stays valid. A default name is only translated
    /// together with its scene's id: what the user typed, or a scene left alone, is respected.
    pub fn migrate_legacy_ids(&mut self) -> LegacyRenames {
        let mut out = LegacyRenames::default();
        let taken: Vec<String> = self.scenes.iter().map(|s| s.id.clone()).collect();
        let machines = self.machines.clone();
        for s in &mut self.scenes {
            if s.icon == OLD_ICON {
                s.icon = crate::scene::DEFAULT_ICON.to_owned();
                out.changed = true;
            }
            let Some(new) = scene_id(&s.id).filter(|new| !taken.iter().any(|t| t == new)) else {
                continue;
            };
            out.scenes
                .push((std::mem::replace(&mut s.id, new.to_owned()), new));
            out.changed = true;
            if s.name == "Trabajo" && s.require_on.is_none() {
                s.name = "Work".to_owned();
            } else if let Some(side) = s.require_on
                && s.name == format!("Solo {}", machines.name(side))
            {
                s.name = format!("Only {}", machines.name(side));
            }
        }

        let screens: Vec<(Machine, String)> = self
            .screens
            .iter()
            .map(|s| (s.machine, s.id.clone()))
            .collect();
        let taken =
            |machine: Machine, id: &str| screens.iter().any(|(m, t)| *m == machine && t == id);
        for screen in &mut self.screens {
            if let Some(new) = screen_id(&screen.id).filter(|new| !taken(screen.machine, new)) {
                screen.id = new;
                out.changed = true;
            }
        }
        // A cable names a screen of its own side and follows it: renamed if the screen was, kept
        // if it stayed because of a collision. One naming a screen not saved yet is mapped the
        // way the detector names it now, unless that would point at another screen.
        for cable in &mut self.cables {
            for side in Machine::ALL {
                let end = cable.end_mut(side);
                if let Some(new) = end.screen.as_deref().and_then(screen_id)
                    && !taken(side, &new)
                {
                    end.screen = Some(new);
                    out.changed = true;
                }
            }
        }
        out
    }
}

impl State {
    /// Follows the desk's renames ([`Desk::migrate_legacy_ids`]): the stored scene gets the new id
    /// only if the desk renamed that very scene. `true` if it changed.
    pub fn migrate_legacy_ids(&mut self, desk: &LegacyRenames) -> bool {
        let Some(new) = self.scene.as_deref().and_then(|id| desk.scene(id)) else {
            return false;
        };
        self.scene = Some(new.to_owned());
        true
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
        assert!(desk.migrate_legacy_ids().changed());
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
        assert!(desk.migrate_legacy_ids().changed());
        let once = desk.clone();
        assert!(!desk.migrate_legacy_ids().changed());
        assert_eq!(desk, once);
    }

    #[test]
    fn a_new_desk_is_left_untouched() {
        let mut desk = old_desk();
        desk.migrate_legacy_ids();
        let new = desk.clone();
        let mut again = new.clone();
        assert!(!again.migrate_legacy_ids().changed());
        assert_eq!(again, new);
    }

    #[test]
    fn names_the_user_chose_are_respected() {
        let mut desk = old_desk();
        desk.scenes[0].name = "Trabajo duro".into();
        desk.scenes[1].name = "Solo yo".into();
        assert!(desk.migrate_legacy_ids().changed());
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

    /// Review of #152: a desk with both `pantalla-1` and `screen-1` on one machine keeps both
    /// (a duplicate would fail `validate`), and the cable still names the screen it named.
    #[test]
    fn a_screen_id_already_taken_is_not_duplicated() {
        let mut desk = old_desk();
        let mut taken = desk.screens[0].clone();
        taken.id = "screen-1".into();
        taken.x = 100;
        desk.screens.push(taken);
        let mut other = MonitorCable::new("lc34-02");
        other.a.screen = Some("screen-1".into());
        desk.cables.push(other);
        desk.migrate_legacy_ids();
        desk.validate().unwrap();
        let ids: Vec<_> = desk
            .screens
            .iter()
            .map(|s| (s.machine, s.id.as_str()))
            .collect();
        assert_eq!(
            ids,
            [
                (Machine::A, "pantalla-1"),
                (Machine::B, "screen-2"),
                (Machine::A, "screen-1")
            ]
        );
        assert_eq!(desk.cables[0].a.screen.as_deref(), Some("pantalla-1"));
        assert_eq!(desk.cables[1].a.screen.as_deref(), Some("screen-1"));
    }

    /// The same number on the other machine is not a collision.
    #[test]
    fn a_screen_id_taken_on_the_other_machine_still_maps() {
        let mut desk = old_desk();
        let mut b = desk.screens[1].clone();
        b.id = "screen-1".into();
        desk.screens.push(b);
        desk.migrate_legacy_ids();
        assert_eq!(desk.screens[0].id, "screen-1");
        assert_eq!(desk.cables[0].a.screen.as_deref(), Some("screen-1"));
    }

    /// Review of #152: the default name goes with the id. If the id stays because of a
    /// collision, the name stays too (no two scenes called "Work", no lost name).
    #[test]
    fn a_name_is_not_translated_when_its_id_stays() {
        let mut desk = old_desk();
        desk.scenes.push(Scene::new("work", "Work"));
        let mut mine = Scene::new("only-a", "Mine");
        mine.require_on = Some(Machine::A);
        desk.scenes.push(mine);
        assert!(desk.migrate_legacy_ids().changed());
        let scenes: Vec<_> = desk
            .scenes
            .iter()
            .map(|s| (s.id.as_str(), s.name.as_str()))
            .collect();
        assert_eq!(
            scenes,
            [
                ("trabajo", "Trabajo"),
                ("solo-a", "Solo MacBook"),
                ("only-b", "Only PC"),
                ("work", "Work"),
                ("only-a", "Mine"),
            ]
        );
        desk.validate().unwrap();
    }

    #[test]
    fn the_stored_state_follows_the_scenes_the_desk_renamed() {
        let mut desk = old_desk();
        let renames = desk.migrate_legacy_ids();
        for (old, new) in [
            ("trabajo", "work"),
            ("solo-a", "only-a"),
            ("solo-b", "only-b"),
        ] {
            let mut s = State::initial(Machine::A);
            s.scene = Some(old.into());
            assert!(s.migrate_legacy_ids(&renames));
            assert_eq!(s.scene.as_deref(), Some(new));
            assert!(!s.migrate_legacy_ids(&renames));
        }
        let mut s = State::initial(Machine::A);
        assert!(!s.migrate_legacy_ids(&renames));
        s.scene = Some("game".into());
        assert!(!s.migrate_legacy_ids(&renames));
    }

    /// Review of #152: the desk kept `trabajo` because the user has a scene `work`. The state
    /// keeps pointing at `trabajo`, not at the user's other scene.
    #[test]
    fn the_stored_state_is_not_renamed_when_the_desk_kept_the_scene() {
        let mut desk = old_desk();
        desk.scenes.push(Scene::new("work", "My work"));
        let renames = desk.migrate_legacy_ids();
        let mut s = State::initial(Machine::A);
        s.scene = Some("trabajo".into());
        assert!(!s.migrate_legacy_ids(&renames));
        assert_eq!(s.scene.as_deref(), Some("trabajo"));
        // Nothing renamed at all (an already migrated desk): nothing to follow.
        assert!(!s.migrate_legacy_ids(&LegacyRenames::default()));
        assert_eq!(s.scene.as_deref(), Some("trabajo"));
    }

    #[test]
    fn only_numbered_screens_map() {
        assert_eq!(screen_id("pantalla-3").as_deref(), Some("screen-3"));
        assert_eq!(screen_id("pantalla-"), None);
        assert_eq!(screen_id("pantalla-x"), None);
        assert_eq!(screen_id("screen-3"), None);
    }
}
