//! Scenes (DECISIONS D6, DESIGN §5): they replace the former modes.
//!
//! A scene says which machine shows each compatible monitor, how the desktops are laid out
//! (the arrangement) and which machine has to be on. Applying it is a transaction
//! ([`Scene::plan`]): power on what is needed → each monitor → the arrangement → the geometry
//! of the input engine. The monitor and arrangement steps are undone if a later one
//! fails; the power ones are not (see [`crate::txn`]).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::model::{Arrangement, Desk, Machine, Machines, Os};
use crate::monitor::MonitorCable;
use crate::plan::{Abilities, Plan, PlanError, Step};
use crate::state::State;

/// The id of a compatible monitor (`<model>-<hash>`, the one from `tandem-monitor`).
pub type MonitorId = String;

/// Maximum length of a scene name, in characters.
pub const MAX_SCENE_NAME: usize = 40;

/// Icon of a scene with no chosen icon (the names are the UI's).
pub const DEFAULT_ICON: &str = "desk";

/// A desk scene (D6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub id: String,
    pub name: String,
    #[serde(default = "default_icon")]
    pub icon: String,
    /// Which machine shows each compatible monitor. Incompatible ones do not enter (DESIGN §5).
    #[serde(default)]
    pub monitors: Vec<(MonitorId, Machine)>,
    /// How the desktops are laid out; `None`: the arrangement is not touched. An arrangement relates the
    /// two machines, so it requires both on ([`Scene::needs`]).
    #[serde(default)]
    pub layout: Option<Arrangement>,
    /// A machine that has to be on even if no monitor shows it.
    #[serde(default)]
    pub require_on: Option<Machine>,
    /// Global shortcut (all keys at once), with the `KeyboardEvent.code` names.
    /// Empty = no shortcut.
    #[serde(default)]
    pub shortcut: Vec<String>,
}

fn default_icon() -> String {
    DEFAULT_ICON.to_owned()
}

impl Scene {
    /// An empty scene: changes nothing when applied, except marking itself as active.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Scene {
            id: id.into(),
            name: name.into(),
            icon: default_icon(),
            monitors: Vec::new(),
            layout: None,
            require_on: None,
            shortcut: Vec::new(),
        }
    }

    /// The machines that have to be on with the scene in place: the one it asks for, the
    /// ones some monitor shows and, with an arrangement, both.
    pub fn needs(&self) -> BTreeSet<Machine> {
        let mut needs: BTreeSet<Machine> = self.monitors.iter().map(|(_, m)| *m).collect();
        needs.extend(self.require_on);
        if self.layout.is_some() {
            needs.extend(Machine::ALL);
        }
        needs
    }

    /// Whether the scene still holds given `state`: everything it needs is on.
    pub fn holds(&self, state: &State) -> bool {
        self.needs().iter().all(|&m| state.is_on(m))
    }

    /// The steps to apply it from `from`, with the desk's current arrangement `current`: power on
    /// what is missing → each monitor → the arrangement (if it changes) → the geometry (if anything
    /// of the former changed). What is already there generates no power or arrangement steps; monitors
    /// always go, because the state does not know what each one shows.
    pub fn plan(
        &self,
        from: &State,
        current: Option<Arrangement>,
        abilities: &Abilities,
    ) -> Result<Plan, PlanError> {
        let mut steps = Vec::new();
        for machine in self.needs() {
            steps.extend(Plan::power_on(from, machine, abilities)?.steps);
        }
        let power = steps.len();
        steps.extend(
            self.monitors
                .iter()
                .map(|(monitor, machine)| Step::ShowMachine {
                    monitor: monitor.clone(),
                    machine: *machine,
                }),
        );
        if let Some(layout) = self.layout
            && current != Some(layout)
        {
            steps.push(Step::Arrange {
                layout: Some(layout),
            });
        }
        if steps.len() > power {
            steps.push(Step::Geometry);
        }
        Ok(Plan { steps })
    }

    /// The same scene with the sides swapped ([`crate::Desk::swap_sides`]).
    pub fn swapped(self) -> Self {
        Scene {
            monitors: self
                .monitors
                .into_iter()
                .map(|(monitor, m)| (monitor, m.other()))
                .collect(),
            layout: self.layout.map(Arrangement::swapped),
            require_on: self.require_on.map(Machine::other),
            ..self
        }
    }

    /// What fails in the scene on its own (repeated ids and shortcuts are seen by the desk).
    pub fn problem(&self) -> Option<String> {
        if self.id.trim().is_empty() || self.name.trim().is_empty() {
            return Some("a scene has no id or name".to_owned());
        }
        if self.name.chars().count() > MAX_SCENE_NAME {
            return Some(format!(
                "a scene name cannot exceed {MAX_SCENE_NAME} characters"
            ));
        }
        let mut seen = BTreeSet::new();
        for (monitor, _) in &self.monitors {
            if monitor.trim().is_empty() {
                return Some(format!("a monitor of \"{}\" has no id", self.name));
            }
            if !seen.insert(monitor.as_str()) {
                return Some(format!(
                    "the monitor `{monitor}` appears twice in \"{}\"",
                    self.name
                ));
            }
        }
        None
    }
}

/// The name of a machine's "only" scene.
fn only_name(machine_name: &str) -> String {
    format!("Only {machine_name}")
}

/// The default shortcut of scene `n` (1, 2, 3…): ⌃⌥n, as in the mockups.
fn default_shortcut(n: u8) -> Vec<String> {
    vec![
        "ControlLeft".to_owned(),
        "AltLeft".to_owned(),
        format!("Digit{n}"),
    ]
}

/// The scenes created when pairing: "Work" (both, with the current arrangement) and
/// "Only <A>" and "Only <B>" with the real names. Their monitors are assigned by T-111.
pub fn default_scenes(machines: &Machines, arrangement: Option<Arrangement>) -> Vec<Scene> {
    let only = |side: Machine, n: u8| {
        let icon = match machines.get(side).map(|i| i.os) {
            Some(Os::Windows) => "tower",
            _ => "laptop",
        };
        Scene {
            icon: icon.to_owned(),
            require_on: Some(side),
            shortcut: default_shortcut(n),
            ..Scene::new(
                format!("only-{}", side.as_str()),
                only_name(&machines.name(side)),
            )
        }
    };
    vec![
        Scene {
            layout: Some(arrangement.unwrap_or_default()),
            shortcut: default_shortcut(1),
            ..Scene::new("work", "Work")
        },
        only(Machine::A, 2),
        only(Machine::B, 3),
    ]
}

/// Sets the default scenes in `desk` if it knows both machines and has none.
/// One whose shortcut the lock (or another scene) already uses is left without a shortcut; if even so the desk
/// were not valid, nothing is added. `true` if it added them.
pub fn add_defaults(desk: &mut Desk) -> bool {
    let both = Machine::ALL.iter().all(|&m| desk.machines.get(m).is_some());
    if !both || !desk.scenes.is_empty() {
        return false;
    }
    let keys = |k: &[String]| k.iter().cloned().collect::<BTreeSet<String>>();
    let mut taken = vec![keys(&desk.shortcuts.lock)];
    let mut scenes = default_scenes(&desk.machines, desk.arrangement);
    for s in &mut scenes {
        let k = keys(&s.shortcut);
        if taken.contains(&k) {
            s.shortcut.clear();
        } else {
            taken.push(k);
        }
    }
    let mut next = desk.clone();
    next.scenes = scenes;
    if next.validate().is_err() {
        return false;
    }
    *desk = next;
    true
}

/// Whether `scenes` are the default ones for `desk` (with or without their shortcuts): the agent set them when
/// pairing, not a person.
pub fn are_defaults(scenes: &[Scene], desk: &Desk) -> bool {
    let bare = |s: &Scene| Scene {
        shortcut: Vec::new(),
        ..s.clone()
    };
    let defaults = default_scenes(&desk.machines, desk.arrangement);
    scenes.len() == defaults.len()
        && scenes
            .iter()
            .zip(&defaults)
            .all(|(a, b)| bare(a) == bare(b))
}

/// A new id for a scene called `name` that does not clash with `taken`: the name in
/// lowercase, without accents or spaces ("Only MacBook" → `only-macbook`).
pub fn scene_id(name: &str, taken: &[&str]) -> String {
    let mut base = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        let c = match c {
            '\u{e1}' | '\u{e0}' | '\u{e4}' | '\u{e2}' => 'a',
            '\u{e9}' | '\u{e8}' | '\u{eb}' | '\u{ea}' => 'e',
            '\u{ed}' | '\u{ec}' | '\u{ef}' | '\u{ee}' => 'i',
            '\u{f3}' | '\u{f2}' | '\u{f6}' | '\u{f4}' => 'o',
            '\u{fa}' | '\u{f9}' | '\u{fc}' | '\u{fb}' => 'u',
            '\u{f1}' => 'n',
            c if c.is_ascii_alphanumeric() => c,
            _ => '-',
        };
        if c != '-' || !(base.is_empty() || base.ends_with('-')) {
            base.push(c);
        }
    }
    while base.ends_with('-') {
        base.pop();
    }
    if base.is_empty() {
        base.push_str("scene");
    }
    let mut id = base.clone();
    let mut n = 2;
    while taken.contains(&id.as_str()) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// Introduces a new compatible monitor to the desk (T-111): records its cabling and adds it to
/// the default scenes that still are so by id: "Only A" shows it on A, "Only B" on
/// B and "Work" with what it shows now (`shows`; without knowing, it does not enter). A scene that already
/// names it is not touched. `false` if it already had cabling (already introduced: whatever the user did
/// afterwards is respected) or if the desk would not stay valid.
pub fn introduce_monitor(desk: &mut Desk, cable: MonitorCable, shows: Option<Machine>) -> bool {
    if desk.cable(&cable.monitor).is_some() {
        return false;
    }
    let mut next = desk.clone();
    for s in &mut next.scenes {
        let machine = match s.id.as_str() {
            "work" => shows,
            "only-a" if s.require_on == Some(Machine::A) => Some(Machine::A),
            "only-b" if s.require_on == Some(Machine::B) => Some(Machine::B),
            _ => None,
        };
        if let Some(machine) = machine
            && s.monitors.iter().all(|(m, _)| *m != cable.monitor)
        {
            s.monitors.push((cable.monitor.clone(), machine));
        }
    }
    next.cables.push(cable);
    if next.validate().is_err() {
        return false;
    }
    *desk = next;
    true
}

/// Whether from `old` to `new` only what [`introduce_monitor`] does changed: new cables (the earlier
/// ones equal) and those monitors added to scenes. The agent did it, not a person.
pub fn introduced_by_agent(old: &Desk, new: &Desk) -> bool {
    let added: BTreeSet<&str> = new
        .cables
        .iter()
        .filter(|c| old.cable(&c.monitor).is_none())
        .map(|c| c.monitor.as_str())
        .collect();
    let kept = old.cables.iter().all(|c| new.cable(&c.monitor) == Some(c));
    if added.is_empty() || !kept || new.cables.len() != old.cables.len() + added.len() {
        return false;
    }
    let stripped: Vec<Scene> = new
        .scenes
        .iter()
        .map(|s| Scene {
            monitors: s
                .monitors
                .iter()
                .filter(|(m, _)| !added.contains(m.as_str()))
                .cloned()
                .collect(),
            ..s.clone()
        })
        .collect();
    new.scenes != old.scenes && stripped == old.scenes
}

/// The scene in place according to what each monitor really shows (`shows`, from the
/// real input read by the owner): of those that hold with `state`, have monitors, all
/// show their machine and whose arrangement (if it has one) is the desk's (undeclared, the
/// default), the one that asks for the most (more
/// monitors, more machines); on a tie, the first. A scene without monitors cannot be
/// recognized.
pub fn detect<'a>(
    desk: &'a Desk,
    state: &State,
    shows: &BTreeMap<MonitorId, Machine>,
) -> Option<&'a Scene> {
    desk.scenes
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            !s.monitors.is_empty()
                && s.holds(state)
                && s.layout
                    .is_none_or(|l| desk.arrangement.unwrap_or_default() == l)
                && s.monitors
                    .iter()
                    .all(|(m, machine)| shows.get(m) == Some(machine))
        })
        .max_by_key(|(i, s)| (s.monitors.len(), s.needs().len(), Reverse(*i)))
        .map(|(_, s)| s)
}

/// Whether what the monitors show contradicts `scene`: one shows another machine than the
/// scene's. A monitor of which nothing is known does not contradict.
pub fn contradicted(scene: &Scene, shows: &BTreeMap<MonitorId, Machine>) -> bool {
    scene
        .monitors
        .iter()
        .any(|(m, machine)| shows.get(m).is_some_and(|s| s != machine))
}

/// When renaming `side` from `old` to `new`, its scene "Only <old>" becomes "Only <new>". One
/// that the user already renamed to something else is respected.
pub fn rename_machine(scenes: &mut [Scene], side: Machine, old: &str, new: &str) {
    for s in scenes {
        if s.require_on == Some(side) && s.name == only_name(old) {
            s.name = only_name(new);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Align, MachineInfo, Side};

    fn machines() -> Machines {
        Machines {
            a: Some(MachineInfo::new(
                Machine::A,
                Os::MacOs,
                Some("MacBook Pro".into()),
            )),
            b: Some(MachineInfo::new(
                Machine::B,
                Os::Windows,
                Some("Desktop-PC".into()),
            )),
        }
    }

    fn left() -> Arrangement {
        Arrangement {
            a_side: Side::Left,
            align: Align::Center,
        }
    }

    #[test]
    fn the_defaults_use_the_real_names_and_the_current_arrangement() {
        let up = Arrangement {
            a_side: Side::Up,
            align: Align::Start,
        };
        let s = default_scenes(&machines(), Some(up));
        let names: Vec<&str> = s.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Work", "Only MacBook Pro", "Only Desktop-PC"]);
        let ids: Vec<&str> = s.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["work", "only-a", "only-b"]);
        assert_eq!(s[0].layout, Some(up));
        assert_eq!(s[0].needs(), BTreeSet::from(Machine::ALL));
        assert_eq!(s[1].needs(), BTreeSet::from([Machine::A]));
        assert_eq!(s[2].needs(), BTreeSet::from([Machine::B]));
        assert_eq!(
            (s[1].icon.as_str(), s[2].icon.as_str()),
            ("laptop", "tower")
        );
        assert_eq!(s[2].shortcut, ["ControlLeft", "AltLeft", "Digit3"]);
        assert!(
            s.iter()
                .all(|s| s.monitors.is_empty() && s.problem().is_none())
        );
        // Without a declared arrangement, "Work" carries the default one.
        assert_eq!(
            default_scenes(&machines(), None)[0].layout,
            Some(Arrangement::default())
        );
    }

    #[test]
    fn a_scene_reads_from_toml_with_only_id_and_name() {
        let s: Scene = toml::from_str("id = \"x\"\nname = \"X\"").unwrap();
        assert_eq!(s, Scene::new("x", "X"));
        let full: Scene = toml::from_str(
            "id = \"j\"\nname = \"Game\"\nicon = \"play\"\nmonitors = [[\"lc34-01\", \"b\"]]\n\
             layout = { a_side = \"up\", align = \"end\" }\nrequire_on = \"b\"\n\
             shortcut = [\"ControlLeft\", \"Digit4\"]",
        )
        .unwrap();
        assert_eq!(full.monitors, [("lc34-01".to_owned(), Machine::B)]);
        assert_eq!(full.layout.map(|l| l.a_side), Some(Side::Up));
        assert_eq!(full.require_on, Some(Machine::B));
        assert!(toml::from_str::<Scene>("id = \"x\"\nname = \"X\"\nmode = 1").is_err());
    }

    #[test]
    fn the_plan_wakes_first_then_monitors_then_arrangement_then_geometry() {
        let mut s = Scene::new("t", "Work");
        s.monitors = vec![("m34".into(), Machine::B), ("m27".into(), Machine::A)];
        s.layout = Some(left());
        let from = State::initial(Machine::A);
        let plan = s.plan(&from, None, &Abilities::both_wake()).unwrap();
        assert_eq!(
            plan.steps,
            [
                Step::PowerOn {
                    machine: Machine::B
                },
                Step::ShowMachine {
                    monitor: "m34".into(),
                    machine: Machine::B
                },
                Step::ShowMachine {
                    monitor: "m27".into(),
                    machine: Machine::A
                },
                Step::Arrange {
                    layout: Some(left())
                },
                Step::Geometry,
            ]
        );
        // What is already there is not repeated.
        let mut both = from.clone();
        both.on.set(Machine::B, true);
        s.monitors.clear();
        assert!(
            s.plan(&both, Some(left()), &Abilities::both_wake())
                .unwrap()
                .steps
                .is_empty()
        );
        // One that cannot be powered on is a plan error, before touching anything.
        assert_eq!(
            s.plan(&from, None, &Abilities::default()).unwrap_err(),
            PlanError::CannotWake(Machine::B)
        );
    }

    #[test]
    fn only_power_changes_need_no_geometry() {
        let mut s = Scene::new("b", "Only B");
        s.require_on = Some(Machine::B);
        let plan = s
            .plan(&State::initial(Machine::A), None, &Abilities::both_wake())
            .unwrap();
        assert_eq!(
            plan.steps,
            [Step::PowerOn {
                machine: Machine::B
            }]
        );
        assert!(!s.holds(&State::initial(Machine::A)));
        assert!(s.holds(&State::initial(Machine::B)));
    }

    #[test]
    fn swapping_sides_keeps_what_each_monitor_shows() {
        let mut s = default_scenes(&machines(), Some(left())).remove(1);
        s.monitors = vec![("m34".into(), Machine::A)];
        let w = s.clone().swapped();
        assert_eq!(w.monitors, [("m34".to_owned(), Machine::B)]);
        assert_eq!(w.require_on, Some(Machine::B));
        assert_eq!(w.clone().swapped(), s);
        let t = default_scenes(&machines(), Some(left()))
            .remove(0)
            .swapped();
        assert_eq!(t.layout, Some(left().swapped()));
    }

    #[test]
    fn problems_of_a_scene() {
        assert!(Scene::new("", "x").problem().is_some());
        assert!(Scene::new("x", " ").problem().is_some());
        assert!(Scene::new("x", "y".repeat(41)).problem().is_some());
        let mut s = Scene::new("x", "X");
        s.monitors = vec![("m".into(), Machine::A), ("m".into(), Machine::B)];
        assert!(s.problem().unwrap().contains("twice"));
        s.monitors = vec![(" ".into(), Machine::A)];
        assert!(s.problem().is_some());
    }

    fn paired() -> Desk {
        Desk {
            machines: machines(),
            ..Desk::default()
        }
    }

    #[test]
    fn the_defaults_never_take_the_lock_shortcut() {
        let mut d = paired();
        d.shortcuts.lock = vec!["ControlLeft".into(), "AltLeft".into(), "Digit1".into()];
        assert!(add_defaults(&mut d));
        assert!(d.validate().is_ok());
        assert!(
            d.scenes[0].shortcut.is_empty(),
            "\"Work\" is left without a shortcut"
        );
        assert_eq!(d.scenes[1].shortcut, ["ControlLeft", "AltLeft", "Digit2"]);
        assert!(are_defaults(&d.scenes, &d));
        // Only once, and only with both machines.
        assert!(!add_defaults(&mut d));
        let mut half = Desk::default();
        half.machines.a = machines().a;
        assert!(!add_defaults(&mut half));
        assert!(half.scenes.is_empty());
    }

    #[test]
    fn new_ids_come_from_the_name() {
        assert_eq!(scene_id("Only MacBook", &[]), "only-macbook");
        assert_eq!(
            scene_id("  Caf\u{e9} and Design!  ", &[]),
            "cafe-and-design"
        );
        assert_eq!(scene_id("!!!", &[]), "scene");
        assert_eq!(scene_id("Game", &["game", "game-2"]), "game-3");
    }

    fn cable(monitor: &str) -> MonitorCable {
        let mut c = MonitorCable::new(monitor);
        c.a.input = Some(crate::MonitorInput::Hdmi);
        c.b.input = Some(crate::MonitorInput::DisplayPort);
        c
    }

    fn with_defaults() -> Desk {
        let mut d = paired();
        assert!(add_defaults(&mut d));
        d
    }

    #[test]
    fn a_new_monitor_joins_the_default_scenes_once() {
        let mut d = with_defaults();
        let old = d.clone();
        assert!(introduce_monitor(&mut d, cable("m34"), Some(Machine::B)));
        assert_eq!(d.scenes[0].monitors, [("m34".to_owned(), Machine::B)]);
        assert_eq!(d.scenes[1].monitors, [("m34".to_owned(), Machine::A)]);
        assert_eq!(d.scenes[2].monitors, [("m34".to_owned(), Machine::B)]);
        assert_eq!(d.cable("m34"), Some(&cable("m34")));
        assert!(introduced_by_agent(&old, &d));
        assert_eq!(old.summary_of_changes(&d), None, "the agent did it");
        // Already introduced: what the user does afterwards is respected.
        d.scenes[1].monitors.clear();
        assert!(!introduce_monitor(&mut d, cable("m34"), Some(Machine::A)));
        assert!(d.scenes[1].monitors.is_empty());
        // Without knowing what it shows, "Work" does not carry it; neither does a user scene.
        d.scenes.push(Scene::new("game", "Game"));
        assert!(introduce_monitor(&mut d, cable("m27"), None));
        assert_eq!(d.scenes[0].monitors.len(), 1);
        assert!(d.scenes[3].monitors.is_empty());
        // A cable that a person declares does notify.
        let mut by_hand = old.clone();
        by_hand
            .apply(crate::DeskPatch::Cable(cable("m34")))
            .unwrap();
        assert!(!introduced_by_agent(&old, &by_hand));
        assert_eq!(
            old.summary_of_changes(&by_hand).as_deref(),
            Some("Monitors")
        );
    }

    #[test]
    fn the_scene_on_screen_is_recognized_from_the_real_inputs() {
        let mut d = with_defaults();
        introduce_monitor(&mut d, cable("m34"), Some(Machine::B));
        let both = State {
            on: crate::Powered::both(),
            scene: None,
        };
        let shows = |m: Machine| BTreeMap::from([("m34".to_owned(), m)]);
        // With both and the 34" on B: "Work" (it asks for more than "Only B").
        d.arrangement = Some(Arrangement::default());
        assert_eq!(
            detect(&d, &both, &shows(Machine::B)).map(|s| s.id.as_str()),
            Some("work")
        );
        // With another arrangement, "Work" is not in place: "Only B" remains.
        d.arrangement = Some(left().swapped());
        assert_eq!(
            detect(&d, &both, &shows(Machine::B)).map(|s| s.id.as_str()),
            Some("only-b")
        );
        assert_eq!(
            detect(&d, &both, &shows(Machine::A)).map(|s| s.id.as_str()),
            Some("only-a")
        );
        // Without B powered on, only "Only A" can hold; knowing nothing, none.
        let only_a = State::initial(Machine::A);
        assert_eq!(detect(&d, &only_a, &shows(Machine::B)), None);
        assert_eq!(detect(&d, &both, &BTreeMap::new()), None);
        assert!(contradicted(&d.scenes[1], &shows(Machine::B)));
        assert!(!contradicted(&d.scenes[1], &shows(Machine::A)));
        assert!(!contradicted(&d.scenes[1], &BTreeMap::new()));
    }

    #[test]
    fn renaming_a_machine_renames_its_only_scene() {
        let mut s = default_scenes(&machines(), None);
        s[2].name = "Gaming".into();
        rename_machine(&mut s, Machine::A, "MacBook Pro", "Mac");
        rename_machine(&mut s, Machine::B, "Desktop-PC", "Tower");
        assert_eq!(s[1].name, "Only Mac");
        assert_eq!(s[2].name, "Gaming");
        assert_eq!(s[0].name, "Work");
    }
}
