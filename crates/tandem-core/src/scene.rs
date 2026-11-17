//! Escenas (DECISIONS D6, DESIGN §5): reemplazan a los modos de antes.
//!
//! Una escena dice qué máquina muestra cada monitor compatible, cómo quedan los escritorios
//! (el acomodo) y qué máquina tiene que estar encendida. Aplicarla es una transacción
//! ([`Scene::plan`]): encender lo que haga falta → cada monitor → el acomodo → la geometría
//! del motor de entrada. Los pasos de monitor y de acomodo se deshacen si falla uno
//! posterior; los de energía no (ver [`crate::txn`]).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::model::{Arrangement, Desk, Machine, Machines, Os};
use crate::monitor::MonitorCable;
use crate::plan::{Abilities, Plan, PlanError, Step};
use crate::state::State;

/// El id de un monitor compatible (`<modelo>-<hash>`, el de `tandem-monitor`).
pub type MonitorId = String;

/// Largo máximo del nombre de una escena, en caracteres.
pub const MAX_SCENE_NAME: usize = 40;

/// Ícono de una escena sin ícono elegido (los nombres son los de la UI).
pub const DEFAULT_ICON: &str = "puesto";

/// Una escena del puesto (D6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub id: String,
    pub name: String,
    #[serde(default = "default_icon")]
    pub icon: String,
    /// Qué máquina muestra cada monitor compatible. Los no compatibles no entran (DESIGN §5).
    #[serde(default)]
    pub monitors: Vec<(MonitorId, Machine)>,
    /// Cómo quedan los escritorios; `None`: el acomodo no se toca. Un acomodo relaciona a las
    /// dos máquinas, así que pide las dos encendidas ([`Scene::needs`]).
    #[serde(default)]
    pub layout: Option<Arrangement>,
    /// Una máquina que tiene que estar encendida aunque ningún monitor la muestre.
    #[serde(default)]
    pub require_on: Option<Machine>,
    /// Atajo global (todas las teclas a la vez), con los nombres de `KeyboardEvent.code`.
    /// Vacío = sin atajo.
    #[serde(default)]
    pub shortcut: Vec<String>,
}

fn default_icon() -> String {
    DEFAULT_ICON.to_owned()
}

impl Scene {
    /// Una escena vacía: no cambia nada al aplicarla, salvo marcarse como activa.
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

    /// Las máquinas que tienen que estar encendidas con la escena puesta: la que pide, las
    /// que muestra algún monitor y, con acomodo, las dos.
    pub fn needs(&self) -> BTreeSet<Machine> {
        let mut needs: BTreeSet<Machine> = self.monitors.iter().map(|(_, m)| *m).collect();
        needs.extend(self.require_on);
        if self.layout.is_some() {
            needs.extend(Machine::ALL);
        }
        needs
    }

    /// Si la escena sigue valiendo con `state`: todo lo que necesita está encendido.
    pub fn holds(&self, state: &State) -> bool {
        self.needs().iter().all(|&m| state.is_on(m))
    }

    /// Los pasos para aplicarla desde `from`, con el acomodo `current` del puesto: encender
    /// lo que falte → cada monitor → el acomodo (si cambia) → la geometría (si cambió algo
    /// de lo anterior). Lo que ya está no genera pasos de energía ni de acomodo; los monitores
    /// van siempre, porque el estado no sabe qué muestra cada uno.
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

    /// La misma escena con los lados cambiados ([`crate::Desk::swap_sides`]).
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

    /// Qué falla en la escena por sí sola (los ids y atajos repetidos los ve el puesto).
    pub fn problem(&self) -> Option<String> {
        if self.id.trim().is_empty() || self.name.trim().is_empty() {
            return Some("una escena no tiene id o nombre".to_owned());
        }
        if self.name.chars().count() > MAX_SCENE_NAME {
            return Some(format!(
                "el nombre de una escena no puede pasar de {MAX_SCENE_NAME} caracteres"
            ));
        }
        let mut seen = BTreeSet::new();
        for (monitor, _) in &self.monitors {
            if monitor.trim().is_empty() {
                return Some(format!("un monitor de «{}» no tiene id", self.name));
            }
            if !seen.insert(monitor.as_str()) {
                return Some(format!(
                    "el monitor `{monitor}` está dos veces en «{}»",
                    self.name
                ));
            }
        }
        None
    }
}

/// El nombre de la escena «solo» de una máquina.
fn solo_name(machine_name: &str) -> String {
    format!("Solo {machine_name}")
}

/// El atajo por omisión de la escena `n` (1, 2, 3…): ⌃⌥n, como en los mockups.
fn default_shortcut(n: u8) -> Vec<String> {
    vec![
        "ControlLeft".to_owned(),
        "AltLeft".to_owned(),
        format!("Digit{n}"),
    ]
}

/// Las escenas que se crean al emparejar: «Trabajo» (las dos, con el acomodo de ahora) y
/// «Solo <A>» y «Solo <B>» con los nombres reales. Sus monitores los asigna T-111.
pub fn default_scenes(machines: &Machines, arrangement: Option<Arrangement>) -> Vec<Scene> {
    let solo = |side: Machine, n: u8| {
        let icon = match machines.get(side).map(|i| i.os) {
            Some(Os::Windows) => "tower",
            _ => "laptop",
        };
        Scene {
            icon: icon.to_owned(),
            require_on: Some(side),
            shortcut: default_shortcut(n),
            ..Scene::new(
                format!("solo-{}", side.as_str()),
                solo_name(&machines.name(side)),
            )
        }
    };
    vec![
        Scene {
            layout: Some(arrangement.unwrap_or_default()),
            shortcut: default_shortcut(1),
            ..Scene::new("trabajo", "Trabajo")
        },
        solo(Machine::A, 2),
        solo(Machine::B, 3),
    ]
}

/// Pone las escenas por omisión en `desk` si conoce a las dos máquinas y no tiene ninguna.
/// Una cuyo atajo ya usa el bloqueo (u otra escena) queda sin atajo; si aun así el puesto no
/// fuera válido, no se agrega nada. `true` si las agregó.
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

/// Si `scenes` son las de omisión para `desk` (con o sin sus atajos): las puso el agente al
/// emparejar, no una persona.
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

/// Un id nuevo para una escena llamada `name` que no choque con `taken`: el nombre en
/// minúsculas, sin acentos ni espacios («Solo MacBook» → `solo-macbook`).
pub fn scene_id(name: &str, taken: &[&str]) -> String {
    let mut base = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        let c = match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
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
        base.push_str("escena");
    }
    let mut id = base.clone();
    let mut n = 2;
    while taken.contains(&id.as_str()) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// Presenta un monitor compatible nuevo al puesto (T-111): anota su cableado y lo agrega a
/// las escenas por omisión que siguen siéndolo por id: «Solo A» lo muestra en A, «Solo B» en
/// B y «Trabajo» con lo que muestra ahora (`shows`; sin saberlo, no entra). Una escena que ya
/// lo nombra no se toca. `false` si ya tenía cableado (ya se presentó: lo que el usuario haya
/// hecho después se respeta) o si el puesto no quedaría válido.
pub fn introduce_monitor(desk: &mut Desk, cable: MonitorCable, shows: Option<Machine>) -> bool {
    if desk.cable(&cable.monitor).is_some() {
        return false;
    }
    let mut next = desk.clone();
    for s in &mut next.scenes {
        let machine = match s.id.as_str() {
            "trabajo" => shows,
            "solo-a" if s.require_on == Some(Machine::A) => Some(Machine::A),
            "solo-b" if s.require_on == Some(Machine::B) => Some(Machine::B),
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

/// Si de `old` a `new` solo cambió lo que hace [`introduce_monitor`]: cables nuevos (los de
/// antes iguales) y esos monitores agregados a escenas. Lo hizo el agente, no una persona.
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

/// La escena que está puesta según lo que muestra de verdad cada monitor (`shows`, de la
/// entrada real que lee el dueño): de las que valen con `state`, tienen monitores, todos
/// muestran su máquina y su acomodo (si tiene) es el del puesto (sin declarar, el de
/// omisión), la que más pide (más
/// monitores, más máquinas); con empate, la primera. Una escena sin monitores no se puede
/// reconocer.
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

/// Si lo que muestran los monitores contradice a `scene`: alguno muestra otra máquina que la
/// de la escena. Un monitor del que no se sabe nada no contradice.
pub fn contradicted(scene: &Scene, shows: &BTreeMap<MonitorId, Machine>) -> bool {
    scene
        .monitors
        .iter()
        .any(|(m, machine)| shows.get(m).is_some_and(|s| s != machine))
}

/// Al renombrar `side` de `old` a `new`, su escena «Solo <old>» pasa a «Solo <new>». Una
/// que el usuario ya renombró a otra cosa se respeta.
pub fn rename_machine(scenes: &mut [Scene], side: Machine, old: &str, new: &str) {
    for s in scenes {
        if s.require_on == Some(side) && s.name == solo_name(old) {
            s.name = solo_name(new);
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
                Some("PC-Escritorio".into()),
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
        assert_eq!(names, ["Trabajo", "Solo MacBook Pro", "Solo PC-Escritorio"]);
        let ids: Vec<&str> = s.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["trabajo", "solo-a", "solo-b"]);
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
        // Sin acomodo declarado, «Trabajo» lleva el de omisión.
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
            "id = \"j\"\nname = \"Juego\"\nicon = \"play\"\nmonitors = [[\"lc34-01\", \"b\"]]\n\
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
        let mut s = Scene::new("t", "Trabajo");
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
        // Lo que ya está no se repite.
        let mut both = from.clone();
        both.on.set(Machine::B, true);
        s.monitors.clear();
        assert!(
            s.plan(&both, Some(left()), &Abilities::both_wake())
                .unwrap()
                .steps
                .is_empty()
        );
        // Una que no se puede encender es un error del plan, antes de tocar nada.
        assert_eq!(
            s.plan(&from, None, &Abilities::default()).unwrap_err(),
            PlanError::CannotWake(Machine::B)
        );
    }

    #[test]
    fn only_power_changes_need_no_geometry() {
        let mut s = Scene::new("b", "Solo B");
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
        assert!(s.problem().unwrap().contains("dos veces"));
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
        assert!(d.scenes[0].shortcut.is_empty(), "«Trabajo» queda sin atajo");
        assert_eq!(d.scenes[1].shortcut, ["ControlLeft", "AltLeft", "Digit2"]);
        assert!(are_defaults(&d.scenes, &d));
        // Solo una vez, y solo con las dos máquinas.
        assert!(!add_defaults(&mut d));
        let mut half = Desk::default();
        half.machines.a = machines().a;
        assert!(!add_defaults(&mut half));
        assert!(half.scenes.is_empty());
    }

    #[test]
    fn new_ids_come_from_the_name() {
        assert_eq!(scene_id("Solo MacBook", &[]), "solo-macbook");
        assert_eq!(scene_id("  Música y Diseño!  ", &[]), "musica-y-diseno");
        assert_eq!(scene_id("!!!", &[]), "escena");
        assert_eq!(scene_id("Juego", &["juego", "juego-2"]), "juego-3");
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
        assert_eq!(old.summary_of_changes(&d), None, "lo hizo el agente");
        // Ya presentado: lo que haga el usuario después se respeta.
        d.scenes[1].monitors.clear();
        assert!(!introduce_monitor(&mut d, cable("m34"), Some(Machine::A)));
        assert!(d.scenes[1].monitors.is_empty());
        // Sin saber qué muestra, «Trabajo» no lo lleva; una escena del usuario, tampoco.
        d.scenes.push(Scene::new("juego", "Juego"));
        assert!(introduce_monitor(&mut d, cable("m27"), None));
        assert_eq!(d.scenes[0].monitors.len(), 1);
        assert!(d.scenes[3].monitors.is_empty());
        // Un cable que declara una persona sí avisa.
        let mut by_hand = old.clone();
        by_hand
            .apply(crate::DeskPatch::Cable(cable("m34")))
            .unwrap();
        assert!(!introduced_by_agent(&old, &by_hand));
        assert_eq!(
            old.summary_of_changes(&by_hand).as_deref(),
            Some("Monitores")
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
        // Con las dos y el 34" en B: «Trabajo» (pide más que «Solo B»).
        d.arrangement = Some(Arrangement::default());
        assert_eq!(
            detect(&d, &both, &shows(Machine::B)).map(|s| s.id.as_str()),
            Some("trabajo")
        );
        // Con otro acomodo, «Trabajo» no está puesta: queda «Solo B».
        d.arrangement = Some(left().swapped());
        assert_eq!(
            detect(&d, &both, &shows(Machine::B)).map(|s| s.id.as_str()),
            Some("solo-b")
        );
        assert_eq!(
            detect(&d, &both, &shows(Machine::A)).map(|s| s.id.as_str()),
            Some("solo-a")
        );
        // Sin B prendida, solo «Solo A» puede valer; sin saber nada, ninguna.
        let only_a = State::initial(Machine::A);
        assert_eq!(detect(&d, &only_a, &shows(Machine::B)), None);
        assert_eq!(detect(&d, &both, &BTreeMap::new()), None);
        assert!(contradicted(&d.scenes[1], &shows(Machine::B)));
        assert!(!contradicted(&d.scenes[1], &shows(Machine::A)));
        assert!(!contradicted(&d.scenes[1], &BTreeMap::new()));
    }

    #[test]
    fn renaming_a_machine_renames_its_solo_scene() {
        let mut s = default_scenes(&machines(), None);
        s[2].name = "Gaming".into();
        rename_machine(&mut s, Machine::A, "MacBook Pro", "Mac");
        rename_machine(&mut s, Machine::B, "PC-Escritorio", "Torre");
        assert_eq!(s[1].name, "Solo Mac");
        assert_eq!(s[2].name, "Gaming");
        assert_eq!(s[0].name, "Trabajo");
    }
}
