//! The desk document ([`Desk`]) as it is edited and synchronized (DESIGN §4.4).
//!
//! - The sections that are not the arrangement: scenes, shortcuts, keyboard and mouse preferences
//!   and enabled modules.
//! - [`DeskPatch`]: the typed changes the UI requests; the agent applies them.
//! - [`DeskVersion`]: the desk with its stamp (version, date and author). Between the two
//!   machines **the last one wins** (D5): the highest version; with the same version, the newest
//!   date; with the same date, side `A`. This way both choose the same without talking.
//! - [`DeskHistory`]: the last [`HISTORY_LEN`] versions, for "Undo".

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::data::DataPrefs;
use crate::model::{Arrangement, ConfigError, Desk, Machine, ScreenSpec};
use crate::monitor::MonitorCable;
use crate::scene::{Scene, rename_machine, scene_id};

/// How many versions the history keeps (DESIGN §4.4).
pub const HISTORY_LEN: usize = 20;

/// Maximum length of a machine name, in characters.
pub const MAX_NAME: usize = 40;

/// The desk's shortcuts. Keys use the `KeyboardEvent.code` names.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Shortcuts {
    /// Locks the cursor on the machine where it is (all at once). Empty = no shortcut.
    #[serde(default = "default_lock")]
    pub lock: Vec<String>,
}

/// ScrollLock, as in Synergy/Deskflow. On the Mac it arrives as F14 and counts the same.
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

/// Keyboard and mouse preferences. Sharing the clipboard is not here: it is a module of
/// each machine (`modules`, "clipboard"), a single source (D3, D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct InputPrefs {
    /// Cmd↔Ctrl between macOS and Windows (the table for each direction is local, `[input.remap]`).
    #[serde(default = "yes")]
    pub cmd_ctrl: bool,
    /// Drag files when crossing the edge (D14).
    #[serde(default = "yes")]
    pub drag_files: bool,
}

fn yes() -> bool {
    true
}

impl Default for InputPrefs {
    fn default() -> Self {
        InputPrefs {
            cmd_ctrl: true,
            drag_files: true,
        }
    }
}

/// Which modules the user turned on or off on each machine (D3). A module that does not appear
/// stays as its machine leaves it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, PartialOrd, Ord)]
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

/// What a Mac does with its built-in display while an external display is active (T-351).
/// Without any active external display the built-in display always stays on, whatever the mode.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, PartialOrd, Ord,
)]
#[serde(rename_all = "kebab-case")]
pub enum BuiltinMode {
    /// Off: for a Mac used with its lid closed, only the external monitors count.
    #[default]
    ExternalOnly,
    /// It mirrors the main external display.
    Mirror,
    /// A screen of its own, as macOS leaves it.
    Extended,
}

impl BuiltinMode {
    pub const ALL: [BuiltinMode; 3] = [
        BuiltinMode::ExternalOnly,
        BuiltinMode::Mirror,
        BuiltinMode::Extended,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            BuiltinMode::ExternalOnly => "external-only",
            BuiltinMode::Mirror => "mirror",
            BuiltinMode::Extended => "extended",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == s)
    }
}

/// The built-in display mode of each machine (`[builtin_display]`, T-351). A machine without one
/// has the default ([`BuiltinMode::ExternalOnly`]); a machine without a built-in display ignores it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, PartialOrd, Ord,
)]
#[serde(deny_unknown_fields)]
pub struct BuiltinDisplays {
    #[serde(default)]
    pub a: Option<BuiltinMode>,
    #[serde(default)]
    pub b: Option<BuiltinMode>,
}

impl BuiltinDisplays {
    /// The mode of `side`, with the default when it was never chosen.
    pub fn of(&self, side: Machine) -> BuiltinMode {
        match side {
            Machine::A => self.a,
            Machine::B => self.b,
        }
        .unwrap_or_default()
    }

    pub fn set(&mut self, side: Machine, mode: BuiltinMode) {
        match side {
            Machine::A => self.a = Some(mode),
            Machine::B => self.b = Some(mode),
        }
    }

    pub fn swapped(self) -> Self {
        BuiltinDisplays {
            a: self.b,
            b: self.a,
        }
    }
}

/// `#rrggbb`.
pub fn valid_color(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// A desk change requested from the UI (`UiCommand::EditDesk`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeskPatch {
    /// The desk name (T-159); empty goes back to the default.
    Name(String),
    /// Name and color of an already known machine; whatever comes as `None` does not change.
    Machine {
        side: Machine,
        name: Option<String>,
        color: Option<String>,
    },
    /// The arrangement (D18); `None` goes back to the default.
    Arrangement(Option<Arrangement>),
    /// The declared screens of `machine` (replace all of its own; empty = the ones
    /// it reads itself). The `machine` of each one is ignored.
    Screens {
        machine: Machine,
        screens: Vec<ScreenSpec>,
    },
    /// All the scenes (to reorder them).
    Scenes(Vec<Scene>),
    /// A new scene, at the end. Without an id, one is derived from the name.
    AddScene(Scene),
    /// Replaces the scene with the same id.
    EditScene(Scene),
    DeleteScene {
        id: String,
    },
    Shortcuts(Shortcuts),
    Prefs(InputPrefs),
    /// The cap on data over Wi-Fi (`[data]`, T-101).
    Data(DataPrefs),
    /// Turn a machine's module on or off.
    Module {
        machine: Machine,
        module: String,
        on: bool,
    },
    /// A monitor's cabling (replaces any it had; T-111).
    Cable(MonitorCable),
    /// What `machine` does with its built-in display (T-351).
    BuiltinDisplay {
        machine: Machine,
        mode: BuiltinMode,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatchError {
    #[error("machine {0} is not known yet")]
    UnknownMachine(Machine),
    #[error("the name cannot be empty or exceed {MAX_NAME} characters")]
    BadName,
    #[error("\"{0}\" is not a #rrggbb color")]
    BadColor(String),
    #[error("that color already belongs to {0}")]
    ColorTaken(String),
    #[error("the scene `{0}` does not exist")]
    UnknownScene(String),
    #[error("{0}")]
    Invalid(String),
}

impl From<ConfigError> for PatchError {
    fn from(e: ConfigError) -> Self {
        PatchError::Invalid(e.to_string())
    }
}

impl Desk {
    /// Applies `patch`. If the result is not a valid desk, nothing changes.
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
                    let old = std::mem::replace(&mut info.name, name.to_owned());
                    rename_machine(&mut self.scenes, side, &old, name);
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
            DeskPatch::Name(name) => {
                let name = name.trim();
                if name.chars().count() > MAX_NAME {
                    return Err(PatchError::BadName);
                }
                self.name = (!name.is_empty()).then(|| name.to_owned());
            }
            DeskPatch::Arrangement(arrangement) => self.arrangement = arrangement,
            DeskPatch::Screens { machine, screens } => {
                // The other machine's stay where they were; this one's, where the
                // first one was (or at the end).
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
            DeskPatch::AddScene(mut scene) => {
                if scene.id.trim().is_empty() {
                    let taken: Vec<&str> = self.scenes.iter().map(|s| s.id.as_str()).collect();
                    scene.id = scene_id(&scene.name, &taken);
                }
                self.scenes.push(scene);
            }
            DeskPatch::EditScene(scene) => {
                let at = self
                    .scenes
                    .iter()
                    .position(|s| s.id == scene.id)
                    .ok_or_else(|| PatchError::UnknownScene(scene.id.clone()))?;
                self.scenes[at] = scene;
            }
            DeskPatch::DeleteScene { id } => {
                let before = self.scenes.len();
                self.scenes.retain(|s| s.id != id);
                if self.scenes.len() == before {
                    return Err(PatchError::UnknownScene(id));
                }
            }
            DeskPatch::Shortcuts(shortcuts) => self.shortcuts = shortcuts,
            DeskPatch::Prefs(prefs) => self.prefs = prefs,
            DeskPatch::Data(data) => self.data = data,
            DeskPatch::Module {
                machine,
                module,
                on,
            } => self.modules.set(machine, module.trim(), on),
            DeskPatch::BuiltinDisplay { machine, mode } => {
                self.builtin_display.set(machine, mode);
            }
            DeskPatch::Cable(cable) => {
                match self.cables.iter_mut().find(|c| c.monitor == cable.monitor) {
                    Some(c) => *c = cable,
                    None => self.cables.push(cable),
                }
            }
        }
        Ok(())
    }

    /// Which sections change from `self` to `new`, for the UI's "Changed from X" notice.
    /// `None` if nothing a person did changes: recording a newly
    /// known machine (or its system) is done by the agent alone, and so is setting the default
    /// scenes when pairing.
    pub fn summary_of_changes(&self, new: &Desk) -> Option<String> {
        let defaults = self.scenes.is_empty() && crate::scene::are_defaults(&new.scenes, new);
        // The screens the agents learn from the EDID (T-340) are not a person's change.
        let learned = Desk {
            cables: crate::monitor::without_learned_screens(&self.cables, &new.cables),
            ..new.clone()
        };
        let new = &learned;
        // A monitor the agent introduced on its own (its cable and the default scenes).
        let introduced = crate::scene::introduced_by_agent(self, new);
        let machines =
            Machine::ALL
                .iter()
                .any(|&m| match (self.machines.get(m), new.machines.get(m)) {
                    (Some(old), Some(new)) => old.name != new.name || old.color != new.color,
                    (Some(_), None) => true,
                    _ => false,
                });
        let sections = [
            (self.name != new.name, "desk name"),
            (machines, "machines"),
            (self.arrangement != new.arrangement, "arrangement"),
            (self.screens != new.screens, "screens"),
            (
                self.scenes != new.scenes && !defaults && !introduced,
                "scenes",
            ),
            (self.cables != new.cables && !introduced, "monitors"),
            (self.shortcuts != new.shortcuts, "shortcuts"),
            (self.prefs != new.prefs, "keyboard and mouse"),
            (self.modules != new.modules, "modules"),
            (self.data != new.data, "data over Wi-Fi"),
            (
                self.builtin_display != new.builtin_display,
                "built-in display",
            ),
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
            format!("{} and {last}", rest.join(", "))
        };
        // With an initial capital, as the notice title.
        let mut chars = text.chars();
        Some(
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default(),
        )
    }
}

/// The stamp of a desk version: which one it is, when and who made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeskStamp {
    /// Each change raises the highest known by one. 0 = just generated, never changed.
    pub rev: u64,
    /// Milliseconds since the Unix epoch, according to the clock of whoever made it.
    pub at: u64,
    /// Who made it.
    pub by: Machine,
}

impl DeskStamp {
    /// That of a desk just generated on `me`.
    pub fn fresh(me: Machine) -> Self {
        DeskStamp {
            rev: 0,
            at: 0,
            by: me,
        }
    }

    /// The last one wins (D5): higher version; with the same one, later date; with the same, `A`.
    pub fn wins_over(&self, other: &DeskStamp) -> bool {
        self.key() > other.key()
    }

    fn key(&self) -> (u64, u64, bool) {
        (self.rev, self.at, self.by == Machine::A)
    }

    /// The stamp of a change made on `me` at `now` on this version.
    pub fn next(&self, me: Machine, now: u64) -> Self {
        DeskStamp {
            rev: self.rev.saturating_add(1),
            at: now,
            by: me,
        }
    }

    /// The same stamp with the sides swapped.
    pub fn swapped(self) -> Self {
        DeskStamp {
            by: self.by.other(),
            ..self
        }
    }
}

/// The desk with its stamp: what each machine stores and what they send each
/// (`ControlMsg::Desk`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeskVersion {
    pub stamp: DeskStamp,
    pub desk: Desk,
}

impl DeskVersion {
    /// What to do with the version the other machine sent: `Some` is the one that stays here.
    /// Two just-generated desks (version 0) are not merged: each machine keeps its
    /// own until someone changes something. An invalid desk is ignored.
    ///
    /// With the same stamp and another desk (it should not happen: a machine never repeats a version,
    /// but a copied file or a migration can leave it so) the greater desk wins, in the
    /// same order on both: they never stay different forever.
    pub fn receive(&self, theirs: &DeskVersion) -> Option<DeskVersion> {
        if self.stamp.rev == 0 && theirs.stamp.rev == 0 {
            return None;
        }
        let wins = theirs.stamp.wins_over(&self.stamp)
            || (theirs.stamp == self.stamp && theirs.desk > self.desk);
        (wins && theirs.desk.validate().is_ok()).then(|| theirs.clone())
    }

    /// `desk` as a change made on `me` at `now` on this version.
    pub fn change(&self, desk: Desk, me: Machine, now: u64) -> DeskVersion {
        DeskVersion {
            stamp: self.stamp.next(me, now),
            desk,
        }
    }

    /// `patch` applied here: the next version, or `None` if nothing changes.
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
    #[error("version {0} is no longer in the history")]
    Unknown(u64),
}

/// What is shown of a history version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub stamp: DeskStamp,
    /// What changed with respect to the previous one in the history; empty if unknown.
    pub summary: String,
}

/// The last [`HISTORY_LEN`] versions this machine had, from the oldest to the
/// newest, to go back to any of them (`UiCommand::UndoDesk`). It is per machine: it also keeps
/// an own version that lost against a simultaneous change by the other, which is
/// precisely the one to recover with "Undo".
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

    /// Records the version that holds now. The same one twice in a row counts once.
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

    /// The versions for the UI, from newest to oldest.
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

    /// The version `rev` to go back to while at `current`: the newest with that number
    /// that is not the current one (with two simultaneous changes there are two with the same number, and the
    /// one that matters is the one that lost). If the only one is the current, the current.
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

    /// Go back to version `to_rev` while at `current`: a new version (made on `me` at
    /// `now`) with exactly the desk of that one. `None` if it is already that desk.
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

    /// The sides swapped across the whole history ([`Desk::swap_sides`]).
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

    #[test]
    fn the_desk_name_is_edited_trimmed_and_cleared() {
        let mut d = known();
        d.apply(DeskPatch::Name("  Office ".into())).unwrap();
        assert_eq!(d.name.as_deref(), Some("Office"));
        assert_eq!(known().summary_of_changes(&d).as_deref(), Some("Desk name"));
        d.apply(DeskPatch::Name("  ".into())).unwrap();
        assert_eq!(d.name, None);
        let long = "x".repeat(MAX_NAME + 1);
        assert_eq!(d.apply(DeskPatch::Name(long)), Err(PatchError::BadName));
        assert_eq!(d.name, None, "a rejection changes nothing");
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
        // The same stamp with another desk: both choose the same one.
        let mut other = known();
        other.prefs.drag_files = false;
        let x = v(4, 9, Machine::A, known());
        let y = v(4, 9, Machine::A, other);
        let (on_x, on_y) = (
            x.receive(&y).unwrap_or_else(|| x.clone()),
            y.receive(&x).unwrap_or_else(|| y.clone()),
        );
        assert_eq!(on_x, on_y);
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
                    drag_files: false,
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
        assert!(!next.desk.prefs.drag_files);
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
                color: Some("purple".into()),
            }),
            Err(PatchError::BadColor("purple".into()))
        );
        assert_eq!(
            d.apply(DeskPatch::Machine {
                side: Machine::B,
                name: Some(" ".into()),
                color: None,
            }),
            Err(PatchError::BadName)
        );
        assert_eq!(d, before, "a rejected patch changes nothing");
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
        d.apply(DeskPatch::Scenes(vec![Scene::new("t", "Work")]))
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
            d.apply(DeskPatch::Scenes(vec![Scene::new("", "x")]))
                .is_err()
        );
    }

    #[test]
    fn the_built_in_display_mode_defaults_to_external_only_and_is_set_per_machine() {
        let mut d = known();
        assert_eq!(d.builtin_display.of(Machine::A), BuiltinMode::ExternalOnly);
        d.apply(DeskPatch::BuiltinDisplay {
            machine: Machine::A,
            mode: BuiltinMode::Mirror,
        })
        .unwrap();
        assert_eq!(d.builtin_display.of(Machine::A), BuiltinMode::Mirror);
        assert_eq!(d.builtin_display.of(Machine::B), BuiltinMode::ExternalOnly);
        assert_eq!(
            known().summary_of_changes(&d).as_deref(),
            Some("Built-in display")
        );
        let swapped = d.clone().swap_sides();
        assert_eq!(swapped.builtin_display.of(Machine::B), BuiltinMode::Mirror);
        for m in BuiltinMode::ALL {
            assert_eq!(BuiltinMode::parse(m.as_str()), Some(m));
        }
        let toml: Desk = toml::from_str("[builtin_display]\nb = \"extended\"\n").unwrap();
        assert_eq!(toml.builtin_display.of(Machine::B), BuiltinMode::Extended);
        assert!(toml::from_str::<Desk>("[builtin_display]\nb = \"off\"\n").is_err());
    }

    #[test]
    fn the_summary_names_what_a_person_changed() {
        let old = known();
        let mut new = old.clone();
        assert_eq!(old.summary_of_changes(&new), None);
        // The default scenes are set by the agent when pairing; others, by a person.
        new.scenes = crate::default_scenes(&new.machines, None);
        assert_eq!(old.summary_of_changes(&new), None);
        new.scenes.pop();
        assert_eq!(old.summary_of_changes(&new).as_deref(), Some("Scenes"));
        new.scenes.clear();
        new.arrangement = Some(Arrangement::default());
        assert_eq!(old.summary_of_changes(&new).as_deref(), Some("Arrangement"));
        new.prefs.drag_files = false;
        new.shortcuts.lock.clear();
        assert_eq!(
            old.summary_of_changes(&new).as_deref(),
            Some("Arrangement, shortcuts and keyboard and mouse")
        );
        // Recording a machine that was not known is not anyone's change.
        assert_eq!(Desk::default().summary_of_changes(&known()), None);
        let mut renamed = known();
        renamed.machines.b.as_mut().unwrap().name = "Tower".into();
        assert_eq!(
            known().summary_of_changes(&renamed).as_deref(),
            Some("Machines")
        );
    }

    /// T-340: a screen the agent learned from the EDID is not a person's change; one changed by
    /// hand still is.
    #[test]
    fn a_learned_screen_is_not_a_change_of_monitors() {
        let mut old = known();
        old.cables.push(MonitorCable::new("lc34"));
        let mut new = old.clone();
        new.cables[0].b.screen = Some("screen-2".into());
        assert_eq!(old.summary_of_changes(&new), None);
        let mut by_hand = new.clone();
        by_hand.cables[0].b.screen = Some("34".into());
        assert_eq!(
            new.summary_of_changes(&by_hand).as_deref(),
            Some("Monitors")
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
            d.scenes.push(Scene::new(format!("s{rev}"), "x"));
            h.record(v(rev, rev, Machine::A, d));
        }
        assert_eq!(h.len(), HISTORY_LEN);
        let current = h.versions().last().unwrap().clone();
        h.record(current.clone());
        assert_eq!(
            h.len(),
            HISTORY_LEN,
            "the same version in a row is not repeated"
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
        assert_eq!(entries[0].summary, "Scenes");

        // Two simultaneous changes with the same number: it goes back to the one that lost.
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

    #[test]
    fn scenes_are_created_edited_and_deleted_by_patches() {
        let mut d = known();
        d.scenes = crate::default_scenes(&d.machines, None);
        // Without an id, it comes from the name and does not clash with existing ones.
        let mut game = Scene::new("", "Game");
        game.shortcut = vec!["ControlLeft".into(), "AltLeft".into(), "Digit4".into()];
        d.apply(DeskPatch::AddScene(game.clone())).unwrap();
        d.apply(DeskPatch::AddScene(Scene::new("", "Game")))
            .unwrap();
        let ids: Vec<&str> = d.scenes.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["work", "only-a", "only-b", "game", "game-2"]);
        // An id that already exists is not added twice.
        assert!(
            d.apply(DeskPatch::AddScene(Scene::new("game", "Other")))
                .is_err()
        );

        let mut edited = d.scenes[3].clone();
        edited.monitors = vec![("m34".into(), Machine::B)];
        edited.icon = "play".into();
        d.apply(DeskPatch::EditScene(edited.clone())).unwrap();
        assert_eq!(d.scenes[3], edited);
        assert_eq!(
            d.apply(DeskPatch::EditScene(Scene::new("nothing", "Nothing"))),
            Err(PatchError::UnknownScene("nothing".into()))
        );

        d.apply(DeskPatch::DeleteScene {
            id: "game-2".into(),
        })
        .unwrap();
        assert_eq!(d.scenes.len(), 4);
        assert!(
            d.apply(DeskPatch::DeleteScene {
                id: "game-2".into()
            })
            .is_err()
        );
    }

    #[test]
    fn a_shortcut_does_one_thing_only() {
        let mut d = known();
        d.scenes = crate::default_scenes(&d.machines, None);
        let mut copy = Scene::new("", "Copy");
        // The same shortcut as "Work", in another order.
        copy.shortcut = vec!["Digit1".into(), "AltLeft".into(), "ControlLeft".into()];
        let e = d.apply(DeskPatch::AddScene(copy.clone())).unwrap_err();
        assert!(
            e.to_string().contains("already belongs to another scene"),
            "{e}"
        );
        copy.shortcut = d.shortcuts.lock.clone();
        assert!(d.apply(DeskPatch::AddScene(copy.clone())).is_err());
        copy.shortcut.clear();
        d.apply(DeskPatch::AddScene(copy)).unwrap();
    }

    #[test]
    fn renaming_a_machine_renames_its_only_scene_too() {
        let mut d = known();
        d.scenes = crate::default_scenes(&d.machines, None);
        let before = d.clone();
        d.apply(DeskPatch::Machine {
            side: Machine::B,
            name: Some(" Tower ".into()),
            color: None,
        })
        .unwrap();
        assert_eq!(d.scenes[2].name, "Only Tower");
        assert_eq!(d.scenes[1].name, "Only Mac");
        assert_eq!(
            before.summary_of_changes(&d).as_deref(),
            Some("Machines and scenes")
        );
    }

    #[test]
    fn swapping_sides_swaps_the_scenes() {
        let mut d = known();
        d.scenes = crate::default_scenes(&d.machines, None);
        let swapped = d.clone().swap_sides();
        assert_eq!(swapped.scenes[1].require_on, Some(Machine::B));
        assert_eq!(swapped.scenes[1].name, "Only Mac");
        assert_eq!(swapped.swap_sides(), d);
    }
}
