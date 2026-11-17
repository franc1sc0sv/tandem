//! El documento del puesto ([`Desk`]) como se edita y se sincroniza (DESIGN §4.4).
//!
//! - Las secciones que no son del acomodo: escenas, atajos, preferencias de teclado y mouse
//!   y módulos encendidos.
//! - [`DeskPatch`]: los cambios tipados que pide la UI; el agente los aplica.
//! - [`DeskVersion`]: el puesto con su sello (versión, fecha y autor). Entre las dos
//!   máquinas **gana el último** (D5): la versión más alta; con la misma versión, la fecha
//!   más nueva; con la misma fecha, el lado `A`. Así las dos eligen lo mismo sin hablar.
//! - [`DeskHistory`]: las últimas [`HISTORY_LEN`] versiones, para «Deshacer».

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::model::{Arrangement, ConfigError, Desk, Machine, ScreenSpec};

/// Cuántas versiones guarda el historial (DESIGN §4.4).
pub const HISTORY_LEN: usize = 20;

/// Largo máximo del nombre de una máquina, en caracteres.
pub const MAX_NAME: usize = 40;

/// Una escena (D6). Por ahora solo su nombre; lo que muestra cada monitor y el acomodo
/// llegan con T-100.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub id: String,
    pub name: String,
}

/// Los atajos del puesto. Las teclas van con los nombres de `KeyboardEvent.code`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shortcuts {
    /// Bloquea el cursor en la máquina donde está (todas a la vez). Vacío = sin atajo.
    #[serde(default = "default_lock")]
    pub lock: Vec<String>,
}

/// ScrollLock, como en Synergy/Deskflow. En la Mac llega como F14 y vale igual.
fn default_lock() -> Vec<String> {
    vec!["ScrollLock".to_owned()]
}

impl Default for Shortcuts {
    fn default() -> Self {
        Shortcuts {
            lock: default_lock(),
        }
    }
}

/// Preferencias de teclado y mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputPrefs {
    /// Cmd↔Ctrl entre macOS y Windows (la tabla de cada dirección es local, `[input.remap]`).
    #[serde(default = "yes")]
    pub cmd_ctrl: bool,
    /// Arrastrar archivos al cruzar el borde (D14).
    #[serde(default = "yes")]
    pub drag_files: bool,
    /// Compartir el portapapeles.
    #[serde(default = "yes")]
    pub clipboard: bool,
}

fn yes() -> bool {
    true
}

impl Default for InputPrefs {
    fn default() -> Self {
        InputPrefs {
            cmd_ctrl: true,
            drag_files: true,
            clipboard: true,
        }
    }
}

/// Qué módulos encendió o apagó el usuario en cada máquina (D3). Un módulo que no aparece
/// queda como lo deja su máquina.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modules {
    #[serde(default)]
    pub a: BTreeMap<String, bool>,
    #[serde(default)]
    pub b: BTreeMap<String, bool>,
}

impl Modules {
    pub fn of(&self, side: Machine) -> &BTreeMap<String, bool> {
        match side {
            Machine::A => &self.a,
            Machine::B => &self.b,
        }
    }

    pub fn get(&self, side: Machine, module: &str) -> Option<bool> {
        self.of(side).get(module).copied()
    }

    pub fn set(&mut self, side: Machine, module: &str, on: bool) {
        let map = match side {
            Machine::A => &mut self.a,
            Machine::B => &mut self.b,
        };
        map.insert(module.to_owned(), on);
    }

    pub fn swapped(self) -> Self {
        Modules {
            a: self.b,
            b: self.a,
        }
    }
}

/// `#rrggbb`.
pub fn valid_color(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Un cambio del puesto pedido desde la UI (`UiCommand::EditDesk`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeskPatch {
    /// Nombre y color de una máquina ya conocida; lo que venga en `None` no cambia.
    Machine {
        side: Machine,
        name: Option<String>,
        color: Option<String>,
    },
    /// El acomodo (D18); `None` vuelve al de omisión.
    Arrangement(Option<Arrangement>),
    /// Las pantallas declaradas de `machine` (reemplazan todas las suyas; vacío = las que
    /// lee ella misma). El `machine` de cada una se ignora.
    Screens {
        machine: Machine,
        screens: Vec<ScreenSpec>,
    },
    Scenes(Vec<Scene>),
    Shortcuts(Shortcuts),
    Prefs(InputPrefs),
    /// Encender o apagar un módulo de una máquina.
    Module {
        machine: Machine,
        module: String,
        on: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatchError {
    #[error("la máquina {0} todavía no se conoce")]
    UnknownMachine(Machine),
    #[error("el nombre no puede quedar vacío ni pasar de {MAX_NAME} caracteres")]
    BadName,
    #[error("«{0}» no es un color #rrggbb")]
    BadColor(String),
    #[error("ese color ya es el de {0}")]
    ColorTaken(String),
    #[error("{0}")]
    Invalid(String),
}

impl From<ConfigError> for PatchError {
    fn from(e: ConfigError) -> Self {
        PatchError::Invalid(e.to_string())
    }
}

impl Desk {
    /// Aplica `patch`. Si el resultado no es un puesto válido, no cambia nada.
    pub fn apply(&mut self, patch: DeskPatch) -> Result<(), PatchError> {
        let mut next = self.clone();
        next.apply_unchecked(patch)?;
        next.validate()?;
        *self = next;
        Ok(())
    }

    fn apply_unchecked(&mut self, patch: DeskPatch) -> Result<(), PatchError> {
        match patch {
            DeskPatch::Machine { side, name, color } => {
                let other = self.machines.get(side.other()).cloned();
                let mut info = self
                    .machines
                    .get(side)
                    .cloned()
                    .ok_or(PatchError::UnknownMachine(side))?;
                if let Some(name) = name {
                    let name = name.trim();
                    if name.is_empty() || name.chars().count() > MAX_NAME {
                        return Err(PatchError::BadName);
                    }
                    info.name = name.to_owned();
                }
                if let Some(color) = color {
                    let color = color.trim().to_ascii_lowercase();
                    if !valid_color(&color) {
                        return Err(PatchError::BadColor(color));
                    }
                    if let Some(other) = other
                        && other.color.eq_ignore_ascii_case(&color)
                    {
                        return Err(PatchError::ColorTaken(other.name));
                    }
                    info.color = color;
                }
                self.machines.set(side, Some(info));
            }
            DeskPatch::Arrangement(arrangement) => self.arrangement = arrangement,
            DeskPatch::Screens { machine, screens } => {
                // Las de la otra máquina quedan donde estaban; las de esta, donde estaba la
                // primera (o al final).
                let at = self
                    .screens
                    .iter()
                    .position(|s| s.machine == machine)
                    .unwrap_or(self.screens.len());
                let before = self.screens[..at]
                    .iter()
                    .filter(|s| s.machine != machine)
                    .count();
                self.screens.retain(|s| s.machine != machine);
                let mine = screens
                    .into_iter()
                    .map(|s| ScreenSpec { machine, ..s })
                    .collect::<Vec<_>>();
                self.screens.splice(before..before, mine);
            }
            DeskPatch::Scenes(scenes) => self.scenes = scenes,
            DeskPatch::Shortcuts(shortcuts) => self.shortcuts = shortcuts,
            DeskPatch::Prefs(prefs) => self.prefs = prefs,
            DeskPatch::Module {
                machine,
                module,
                on,
            } => self.modules.set(machine, module.trim(), on),
        }
        Ok(())
    }

    /// Qué secciones cambian de `self` a `new`, para el aviso «Cambiado desde X» de la UI.
    /// `None` si no cambia nada que haya hecho una persona: anotar una máquina recién
    /// conocida (o su sistema) lo hace el agente solo.
    pub fn summary_of_changes(&self, new: &Desk) -> Option<String> {
        let machines =
            Machine::ALL
                .iter()
                .any(|&m| match (self.machines.get(m), new.machines.get(m)) {
                    (Some(old), Some(new)) => old.name != new.name || old.color != new.color,
                    (Some(_), None) => true,
                    _ => false,
                });
        let sections = [
            (machines, "máquinas"),
            (self.arrangement != new.arrangement, "acomodo"),
            (self.screens != new.screens, "pantallas"),
            (self.scenes != new.scenes, "escenas"),
            (self.shortcuts != new.shortcuts, "atajos"),
            (self.prefs != new.prefs, "teclado y mouse"),
            (self.modules != new.modules, "módulos"),
        ];
        let changed: Vec<&str> = sections
            .iter()
            .filter(|(changed, _)| *changed)
            .map(|(_, name)| *name)
            .collect();
        let (last, rest) = changed.split_last()?;
        let text = if rest.is_empty() {
            (*last).to_owned()
        } else {
            format!("{} y {last}", rest.join(", "))
        };
        // Con mayúscula inicial, como título del aviso.
        let mut chars = text.chars();
        Some(
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default(),
        )
    }
}

/// El sello de una versión del puesto: cuál es, cuándo y quién la hizo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeskStamp {
    /// Cada cambio sube la más alta conocida en uno. 0 = recién generado, nunca cambiado.
    pub rev: u64,
    /// Milisegundos desde la época Unix, según el reloj de quien lo hizo.
    pub at: u64,
    /// Quién lo hizo.
    pub by: Machine,
}

impl DeskStamp {
    /// El de un puesto recién generado en `me`.
    pub fn fresh(me: Machine) -> Self {
        DeskStamp {
            rev: 0,
            at: 0,
            by: me,
        }
    }

    /// Gana el último (D5): más versión; con la misma, más fecha; con la misma, `A`.
    pub fn wins_over(&self, other: &DeskStamp) -> bool {
        self.key() > other.key()
    }

    fn key(&self) -> (u64, u64, bool) {
        (self.rev, self.at, self.by == Machine::A)
    }

    /// El sello de un cambio hecho en `me` a las `now` sobre esta versión.
    pub fn next(&self, me: Machine, now: u64) -> Self {
        DeskStamp {
            rev: self.rev.saturating_add(1),
            at: now,
            by: me,
        }
    }

    /// El mismo sello con los lados cambiados.
    pub fn swapped(self) -> Self {
        DeskStamp {
            by: self.by.other(),
            ..self
        }
    }
}

/// El puesto con su sello: lo que guarda cada máquina y lo que se mandan
/// (`ControlMsg::Desk`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeskVersion {
    pub stamp: DeskStamp,
    pub desk: Desk,
}

impl DeskVersion {
    /// Qué hacer con la versión que mandó la otra máquina: `Some` es la que queda aquí.
    /// Dos puestos recién generados (versión 0) no se mezclan: cada máquina se queda con lo
    /// suyo hasta que alguien cambie algo. Un puesto inválido se ignora.
    pub fn receive(&self, theirs: &DeskVersion) -> Option<DeskVersion> {
        if self.stamp.rev == 0 && theirs.stamp.rev == 0 {
            return None;
        }
        (theirs.stamp.wins_over(&self.stamp) && theirs.desk.validate().is_ok())
            .then(|| theirs.clone())
    }

    /// `desk` como cambio hecho en `me` a las `now` sobre esta versión.
    pub fn change(&self, desk: Desk, me: Machine, now: u64) -> DeskVersion {
        DeskVersion {
            stamp: self.stamp.next(me, now),
            desk,
        }
    }

    /// `patch` aplicado aquí: la versión siguiente, o `None` si no cambia nada.
    pub fn edit(
        &self,
        patch: DeskPatch,
        me: Machine,
        now: u64,
    ) -> Result<Option<DeskVersion>, PatchError> {
        let mut desk = self.desk.clone();
        desk.apply(patch)?;
        Ok((desk != self.desk).then(|| self.change(desk, me, now)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UndoError {
    #[error("la versión {0} ya no está en el historial")]
    Unknown(u64),
}

/// Lo que se muestra de una versión del historial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub stamp: DeskStamp,
    /// Qué cambió respecto de la anterior del historial; vacío si no se sabe.
    pub summary: String,
}

/// Las últimas [`HISTORY_LEN`] versiones que tuvo esta máquina, de la más vieja a la más
/// nueva, para volver a cualquiera (`UiCommand::UndoDesk`). Es de cada máquina: guarda
/// también una versión propia que perdió contra un cambio simultáneo de la otra, que es
/// justo la que se quiere recuperar con «Deshacer».
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeskHistory {
    versions: VecDeque<DeskVersion>,
}

impl DeskHistory {
    pub fn new(first: DeskVersion) -> Self {
        let mut h = DeskHistory::default();
        h.record(first);
        h
    }

    /// Anota la versión que vale ahora. La misma dos veces seguidas cuenta una.
    pub fn record(&mut self, version: DeskVersion) {
        if self.versions.back() == Some(&version) {
            return;
        }
        self.versions.push_back(version);
        while self.versions.len() > HISTORY_LEN {
            self.versions.pop_front();
        }
    }

    pub fn len(&self) -> usize {
        self.versions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.versions.is_empty()
    }

    pub fn versions(&self) -> impl DoubleEndedIterator<Item = &DeskVersion> {
        self.versions.iter()
    }

    /// Las versiones para la UI, de la más nueva a la más vieja.
    pub fn entries(&self) -> Vec<HistoryEntry> {
        let mut out = Vec::with_capacity(self.versions.len());
        let mut prev: Option<&DeskVersion> = None;
        for v in &self.versions {
            let summary = prev
                .and_then(|p| p.desk.summary_of_changes(&v.desk))
                .unwrap_or_default();
            out.push(HistoryEntry {
                stamp: v.stamp,
                summary,
            });
            prev = Some(v);
        }
        out.reverse();
        out
    }

    /// La versión `rev` a la que se vuelve estando en `current`: la más nueva con ese número
    /// que no sea la actual (con dos cambios simultáneos hay dos con el mismo número, y la
    /// que interesa es la que perdió). Si la única es la actual, la actual.
    pub fn find(&self, rev: u64, current: &DeskStamp) -> Option<&DeskVersion> {
        let mut same = self.versions.iter().rev().filter(|v| v.stamp.rev == rev);
        let mut fallback = None;
        for v in same.by_ref() {
            if v.stamp != *current {
                return Some(v);
            }
            fallback = Some(v);
        }
        fallback
    }

    /// Volver a la versión `to_rev` estando en `current`: una versión nueva (hecha en `me` a
    /// las `now`) con exactamente el puesto de aquella. `None` si ya es ese puesto.
    pub fn undo(
        &self,
        current: &DeskVersion,
        to_rev: u64,
        me: Machine,
        now: u64,
    ) -> Result<Option<DeskVersion>, UndoError> {
        let target = self
            .find(to_rev, &current.stamp)
            .ok_or(UndoError::Unknown(to_rev))?;
        Ok((target.desk != current.desk).then(|| current.change(target.desk.clone(), me, now)))
    }

    /// Los lados cambiados en todo el historial ([`Desk::swap_sides`]).
    pub fn swapped(self) -> Self {
        DeskHistory {
            versions: self
                .versions
                .into_iter()
                .map(|v| DeskVersion {
                    stamp: v.stamp.swapped(),
                    desk: v.desk.swap_sides(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Align, MachineInfo, Os, Scale, Side};

    fn known() -> Desk {
        Desk {
            machines: crate::Machines {
                a: Some(MachineInfo::new(Machine::A, Os::MacOs, None)),
                b: Some(MachineInfo::new(Machine::B, Os::Windows, None)),
            },
            ..Desk::default()
        }
    }

    fn v(rev: u64, at: u64, by: Machine, desk: Desk) -> DeskVersion {
        DeskVersion {
            stamp: DeskStamp { rev, at, by },
            desk,
        }
    }

    fn screen(machine: Machine, id: &str) -> ScreenSpec {
        ScreenSpec {
            machine,
            id: id.into(),
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            scale: Scale::ONE,
        }
    }

    #[test]
    fn the_last_one_wins_by_version_then_clock_then_side_a() {
        let s = |rev, at, by| DeskStamp { rev, at, by };
        assert!(s(3, 0, Machine::B).wins_over(&s(2, 99, Machine::A)));
        assert!(s(3, 10, Machine::B).wins_over(&s(3, 9, Machine::A)));
        assert!(s(3, 10, Machine::A).wins_over(&s(3, 10, Machine::B)));
        assert!(!s(3, 10, Machine::B).wins_over(&s(3, 10, Machine::A)));
        assert!(!s(3, 10, Machine::A).wins_over(&s(3, 10, Machine::A)));
    }

    #[test]
    fn two_fresh_desks_are_never_joined_and_a_broken_one_is_ignored() {
        let mine = v(0, 0, Machine::A, Desk::default());
        let theirs = v(0, 5, Machine::B, known());
        assert_eq!(mine.receive(&theirs), None);
        assert_eq!(theirs.receive(&mine), None);
        let changed = v(1, 0, Machine::B, known());
        assert_eq!(mine.receive(&changed), Some(changed.clone()));
        let mut broken = known();
        broken.screens = vec![screen(Machine::A, "x"), screen(Machine::A, "x")];
        assert_eq!(mine.receive(&v(2, 0, Machine::B, broken)), None);
    }

    #[test]
    fn an_edit_bumps_the_version_and_a_no_op_does_not() {
        let cur = v(4, 100, Machine::B, known());
        let next = cur
            .edit(
                DeskPatch::Prefs(InputPrefs {
                    clipboard: false,
                    ..InputPrefs::default()
                }),
                Machine::A,
                200,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            next.stamp,
            DeskStamp {
                rev: 5,
                at: 200,
                by: Machine::A
            }
        );
        assert!(!next.desk.prefs.clipboard);
        assert_eq!(
            cur.edit(DeskPatch::Prefs(InputPrefs::default()), Machine::A, 300),
            Ok(None)
        );
    }

    #[test]
    fn machine_patches_check_name_and_color() {
        let mut d = known();
        d.apply(DeskPatch::Machine {
            side: Machine::A,
            name: Some("  Air ".into()),
            color: Some("#5FBFB0".into()),
        })
        .unwrap();
        let a = d.machines.get(Machine::A).unwrap();
        assert_eq!((a.name.as_str(), a.color.as_str()), ("Air", "#5fbfb0"));
        let before = d.clone();
        let taken = d.apply(DeskPatch::Machine {
            side: Machine::B,
            name: None,
            color: Some("#5fbfb0".into()),
        });
        assert_eq!(taken, Err(PatchError::ColorTaken("Air".into())));
        assert_eq!(
            d.apply(DeskPatch::Machine {
                side: Machine::B,
                name: None,
                color: Some("violeta".into()),
            }),
            Err(PatchError::BadColor("violeta".into()))
        );
        assert_eq!(
            d.apply(DeskPatch::Machine {
                side: Machine::B,
                name: Some(" ".into()),
                color: None,
            }),
            Err(PatchError::BadName)
        );
        assert_eq!(d, before, "un parche rechazado no cambia nada");
        let mut empty = Desk::default();
        assert_eq!(
            empty.apply(DeskPatch::Machine {
                side: Machine::B,
                name: Some("PC".into()),
                color: None,
            }),
            Err(PatchError::UnknownMachine(Machine::B))
        );
    }

    #[test]
    fn screen_patches_replace_only_that_machine() {
        let mut d = Desk {
            screens: vec![
                screen(Machine::B, "34"),
                screen(Machine::A, "laptop"),
                screen(Machine::B, "32"),
            ],
            ..Desk::default()
        };
        d.apply(DeskPatch::Screens {
            machine: Machine::A,
            screens: vec![screen(Machine::B, "x"), screen(Machine::B, "y")],
        })
        .unwrap();
        let ids: Vec<_> = d
            .screens
            .iter()
            .map(|s| (s.machine, s.id.as_str()))
            .collect();
        assert_eq!(
            ids,
            [
                (Machine::B, "34"),
                (Machine::A, "x"),
                (Machine::A, "y"),
                (Machine::B, "32")
            ]
        );
        let dup = d.apply(DeskPatch::Screens {
            machine: Machine::B,
            screens: vec![screen(Machine::B, "z"), screen(Machine::B, "z")],
        });
        assert!(matches!(dup, Err(PatchError::Invalid(_))));
    }

    #[test]
    fn other_patches_set_their_section() {
        let mut d = known();
        let arr = Arrangement {
            a_side: Side::Down,
            align: Align::Start,
        };
        d.apply(DeskPatch::Arrangement(Some(arr))).unwrap();
        d.apply(DeskPatch::Scenes(vec![Scene {
            id: "t".into(),
            name: "Trabajo".into(),
        }]))
        .unwrap();
        d.apply(DeskPatch::Shortcuts(Shortcuts { lock: vec![] }))
            .unwrap();
        d.apply(DeskPatch::Module {
            machine: Machine::B,
            module: " clipboard ".into(),
            on: false,
        })
        .unwrap();
        assert_eq!(d.arrangement, Some(arr));
        assert_eq!(d.scenes.len(), 1);
        assert!(d.shortcuts.lock.is_empty());
        assert_eq!(d.modules.get(Machine::B, "clipboard"), Some(false));
        assert!(
            d.apply(DeskPatch::Scenes(vec![Scene {
                id: "".into(),
                name: "x".into()
            }]))
            .is_err()
        );
    }

    #[test]
    fn the_summary_names_what_a_person_changed() {
        let old = known();
        let mut new = old.clone();
        assert_eq!(old.summary_of_changes(&new), None);
        new.arrangement = Some(Arrangement::default());
        assert_eq!(old.summary_of_changes(&new).as_deref(), Some("Acomodo"));
        new.prefs.clipboard = false;
        new.shortcuts.lock.clear();
        assert_eq!(
            old.summary_of_changes(&new).as_deref(),
            Some("Acomodo, atajos y teclado y mouse")
        );
        // Anotar una máquina que no se conocía no es un cambio de nadie.
        assert_eq!(Desk::default().summary_of_changes(&known()), None);
        let mut renamed = known();
        renamed.machines.b.as_mut().unwrap().name = "Torre".into();
        assert_eq!(
            known().summary_of_changes(&renamed).as_deref(),
            Some("Máquinas")
        );
    }

    #[test]
    fn colors_end_up_distinct() {
        let mut m = known().machines;
        assert!(!m.distinct_colors());
        m.b.as_mut().unwrap().color = m.a.as_ref().unwrap().color.to_uppercase();
        assert!(m.distinct_colors());
        assert_ne!(m.a.as_ref().unwrap().color, m.b.as_ref().unwrap().color);
        assert!(crate::PALETTE.contains(&m.b.as_ref().unwrap().color.as_str()));
        assert!(valid_color("#a1B2c3"));
        assert!(!valid_color("#a1B2c") && !valid_color("a1B2c3f") && !valid_color("#a1B2cz"));
    }

    #[test]
    fn history_keeps_the_last_twenty_and_finds_the_lost_version() {
        let mut h = DeskHistory::new(v(0, 0, Machine::A, Desk::default()));
        for rev in 1..=25u64 {
            let mut d = Desk::default();
            d.scenes.push(Scene {
                id: format!("s{rev}"),
                name: "x".into(),
            });
            h.record(v(rev, rev, Machine::A, d));
        }
        assert_eq!(h.len(), HISTORY_LEN);
        let current = h.versions().last().unwrap().clone();
        h.record(current.clone());
        assert_eq!(
            h.len(),
            HISTORY_LEN,
            "la misma versión seguida no se repite"
        );
        assert_eq!(
            h.undo(&current, 3, Machine::B, 99),
            Err(UndoError::Unknown(3))
        );
        let back = h.undo(&current, 10, Machine::B, 99).unwrap().unwrap();
        assert_eq!(back.desk.scenes[0].id, "s10");
        assert_eq!(
            back.stamp,
            DeskStamp {
                rev: 26,
                at: 99,
                by: Machine::B
            }
        );
        assert_eq!(h.undo(&current, 25, Machine::B, 99), Ok(None));
        let entries = h.entries();
        assert_eq!(entries[0].stamp.rev, 25);
        assert_eq!(entries[0].summary, "Escenas");

        // Dos cambios simultáneos con el mismo número: se vuelve al que perdió.
        let mine = v(26, 1, Machine::B, known());
        let theirs = v(26, 2, Machine::A, Desk::default());
        h.record(mine.clone());
        h.record(theirs.clone());
        assert_eq!(h.find(26, &theirs.stamp), Some(&mine));
        let undone = h.undo(&theirs, 26, Machine::B, 5).unwrap().unwrap();
        assert_eq!(undone.desk, mine.desk);
        assert_eq!(undone.stamp.rev, 27);
    }

    #[test]
    fn swapping_the_history_swaps_every_version() {
        let mut d = known();
        d.modules.set(Machine::A, "x", true);
        let h = DeskHistory::new(v(2, 3, Machine::A, d.clone())).swapped();
        let only = h.versions().next().unwrap();
        assert_eq!(only.stamp.by, Machine::B);
        assert_eq!(only.desk, d.swap_sides());
    }
}
