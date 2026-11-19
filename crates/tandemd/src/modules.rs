//! Each machine's modules and their state (DECISIONS D3, DESIGN §4.3, T-098).
//!
//! A module exists only on the systems where it can work ([`Registry::for_os`]) and at
//! runtime says whether it is active, turned off by the user or unavailable, with a
//! concrete reason. Its state comes from what the agent already knows ([`ModuleContext`]): modules
//! keep nothing of their own, so they are tested with hand-built contexts.
//!
//! Which modules exist is decided by the machine's [`Os`] and not by a direct `#[cfg]`: on
//! Linux (tests and simulation) the registry of both systems must be buildable. What
//! does depend on the platform (the real probes) lives behind `#[cfg]` in each module.

use std::collections::{BTreeMap, BTreeSet};

use tandem_core::Os;
use tandem_proto::{InputPermission, InputStatus, ModuleId, ModuleInfo, ModuleState, PowerCaps};

/// Reasons for "Files" (T-158): the module exists and works; it only says it is
/// unavailable for one of these true causes.
pub const NO_FILE_DRAG: &str = "Could not open the system's file drag detector";
pub const NO_FILE_DRAG_OS: &str = "This system has no file drag between machines";
pub const NO_ENGINE: &str =
    "The keyboard and mouse engine did not start, and the drag goes with it";

/// The "Files" problem according to what the system gave: `supported` is whether this system
/// has a detector (Windows and macOS), `present` whether the engine received one.
pub fn file_drag_problem(supported: bool, present: bool) -> Option<&'static str> {
    match (present, supported) {
        (true, _) => None,
        (false, true) => Some(NO_FILE_DRAG),
        (false, false) => Some(NO_FILE_DRAG_OS),
    }
}

/// What the agent knows and the modules look at to state their status.
#[derive(Debug, Clone, Copy)]
pub struct ModuleContext<'a> {
    pub os: Os,
    pub input: &'a InputStatus,
    pub power: &'a PowerCaps,
    pub switches: &'a ModuleSwitches,
}

/// What the user turned off and the problems each running module reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleSwitches {
    off: BTreeSet<ModuleId>,
    problems: BTreeMap<ModuleId, String>,
}

impl ModuleSwitches {
    pub fn is_on(&self, id: ModuleId) -> bool {
        !self.off.contains(&id)
    }

    /// `true` if it changed.
    pub fn set(&mut self, id: ModuleId, on: bool) -> bool {
        if on {
            self.off.remove(&id)
        } else {
            self.off.insert(id)
        }
    }

    pub fn problem(&self, id: ModuleId) -> Option<&str> {
        self.problems.get(&id).map(String::as_str)
    }

    /// `true` if it changed.
    pub fn set_problem(&mut self, id: ModuleId, problem: Option<String>) -> bool {
        match problem {
            Some(p) => self.problems.insert(id, p.clone()).as_ref() != Some(&p),
            None => self.problems.remove(&id).is_some(),
        }
    }
}

/// A Tandem module.
pub trait Module: Send + Sync {
    fn id(&self) -> ModuleId;
    fn name(&self) -> &'static str;
    /// What it does on this machine (the UI line).
    fn detail(&self, ctx: &ModuleContext<'_>) -> String;
    /// Its state not counting the user's switch or the problems reported while
    /// running (the [`Registry`] adds that).
    fn state(&self, ctx: &ModuleContext<'_>) -> ModuleState;
    /// The user can turn it on and off. Only the ones the agent really knows how to stop.
    fn switchable(&self) -> bool {
        false
    }
}

/// The modules that exist on a machine.
pub struct Registry {
    modules: Vec<Box<dyn Module>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.modules.iter().map(|m| m.id()))
            .finish()
    }
}

impl Registry {
    /// Those of a machine with `os`, in the UI order.
    pub fn for_os(os: Os) -> Self {
        let mut modules: Vec<Box<dyn Module>> = vec![
            Box::new(Input),
            Box::new(Clipboard),
            Box::new(Files),
            Box::new(Tunnels),
            Box::new(Monitors),
            Box::new(Power),
        ];
        // The secure desktop (UAC, sign-in) is a Windows thing.
        if os == Os::Windows {
            modules.push(Box::new(SecureDesktop));
        }
        Registry { modules }
    }

    /// With tailor-made modules (tests).
    pub fn with(modules: Vec<Box<dyn Module>>) -> Self {
        Registry { modules }
    }

    pub fn ids(&self) -> Vec<ModuleId> {
        self.modules.iter().map(|m| m.id()).collect()
    }

    fn get(&self, id: ModuleId) -> Option<&dyn Module> {
        self.modules.iter().find(|m| m.id() == id).map(|m| &**m)
    }

    /// Each module's announcement. Turned off by the user overrides everything else: when
    /// turning it on the reason is seen if it cannot work.
    pub fn snapshot(&self, ctx: &ModuleContext<'_>) -> Vec<ModuleInfo> {
        self.modules
            .iter()
            .map(|m| {
                let id = m.id();
                let switchable = m.switchable();
                let state = if switchable && !ctx.switches.is_on(id) {
                    ModuleState::Off
                } else if let Some(p) = ctx.switches.problem(id) {
                    ModuleState::Unavailable { reason: p.into() }
                } else {
                    m.state(ctx)
                };
                ModuleInfo {
                    id,
                    name: m.name().into(),
                    detail: m.detail(ctx),
                    state,
                    switchable,
                }
            })
            .collect()
    }

    /// Whether the user can turn `id` on or off here; if not, the reason for the UI.
    pub fn check_switch(&self, id: ModuleId) -> Result<(), String> {
        match self.get(id) {
            None => Err(format!(
                "\"{}\" does not exist on this machine",
                default_name(id)
            )),
            Some(m) if !m.switchable() => {
                Err(format!("\"{}\" cannot be turned on or off", m.name()))
            }
            Some(_) => Ok(()),
        }
    }
}

/// A module's name even if it does not exist on this machine (for messages).
pub fn default_name(id: ModuleId) -> &'static str {
    match id {
        ModuleId::Input => Input.name(),
        ModuleId::Clipboard => Clipboard.name(),
        ModuleId::Files => Files.name(),
        ModuleId::Tunnels => Tunnels.name(),
        ModuleId::Monitors => Monitors.name(),
        ModuleId::Power => Power.name(),
        ModuleId::SecureDesktop => SecureDesktop.name(),
    }
}

/// Shared keyboard and mouse: it does not work without the macOS permissions or if its engine failed.
pub struct Input;

impl Module for Input {
    fn id(&self) -> ModuleId {
        ModuleId::Input
    }
    fn name(&self) -> &'static str {
        "Keyboard and mouse"
    }
    fn detail(&self, ctx: &ModuleContext<'_>) -> String {
        match ctx.os {
            Os::MacOs => "Capture with CGEventTap and injection with CGEventPost".into(),
            Os::Windows => "Capture and injection (low-level hooks, SendInput)".into(),
        }
    }
    fn state(&self, ctx: &ModuleContext<'_>) -> ModuleState {
        if let Some(reason) = missing_permissions(&ctx.input.missing) {
            return ModuleState::Unavailable { reason };
        }
        match &ctx.input.problem {
            Some(p) => ModuleState::Unavailable {
                reason: capitalized(p),
            },
            None => ModuleState::Active,
        }
    }
    fn switchable(&self) -> bool {
        true
    }
}

fn permission_name(p: InputPermission) -> &'static str {
    match p {
        InputPermission::Accessibility => "Accessibility",
        InputPermission::InputMonitoring => "Input Monitoring",
    }
}

/// "The Accessibility permission is missing" / "The permissions A and B are missing".
fn missing_permissions(missing: &[InputPermission]) -> Option<String> {
    match missing {
        [] => None,
        [one] => Some(format!(
            "The {} permission is missing",
            permission_name(*one)
        )),
        many => {
            let names: Vec<_> = many.iter().map(|p| permission_name(*p)).collect();
            let (last, rest) = names.split_last()?;
            Some(format!(
                "The {} and {last} permissions are missing",
                rest.join(", ")
            ))
        }
    }
}

fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The shared clipboard: it runs inside the keyboard and mouse engine, which says which
/// machine the focus goes to.
pub struct Clipboard;

impl Module for Clipboard {
    fn id(&self) -> ModuleId {
        ModuleId::Clipboard
    }
    fn name(&self) -> &'static str {
        "Clipboard"
    }
    fn detail(&self, _ctx: &ModuleContext<'_>) -> String {
        "Text, HTML and image".into()
    }
    fn state(&self, ctx: &ModuleContext<'_>) -> ModuleState {
        if ctx.switches.is_on(ModuleId::Input) {
            ModuleState::Active
        } else {
            ModuleState::Unavailable {
                reason: format!("Needs \"{}\" on", Input.name()),
            }
        }
    }
    fn switchable(&self) -> bool {
        true
    }
}

/// Drag files between machines (D14).
pub struct Files;

impl Module for Files {
    fn id(&self) -> ModuleId {
        ModuleId::Files
    }
    fn name(&self) -> &'static str {
        "Files"
    }
    fn detail(&self, _ctx: &ModuleContext<'_>) -> String {
        "Drag files when crossing the edge".into()
    }
    /// Active: the drag exists on both systems. If the system detector does not open,
    /// the input engine reports it as a problem (`ModuleSwitches::set_problem`).
    fn state(&self, _ctx: &ModuleContext<'_>) -> ModuleState {
        ModuleState::Active
    }
}

/// Claude tunnels (D8).
pub struct Tunnels;

impl Module for Tunnels {
    fn id(&self) -> ModuleId {
        ModuleId::Tunnels
    }
    fn name(&self) -> &'static str {
        "Tunnels"
    }
    fn detail(&self, _ctx: &ModuleContext<'_>) -> String {
        "Receives Claude tunnels".into()
    }
    /// Tunnels (processes, files, ports) are a service of the agent itself on both
    /// systems: they do not depend on a separate backend.
    fn state(&self, _ctx: &ModuleContext<'_>) -> ModuleState {
        ModuleState::Active
    }
}

/// Monitors over DDC (D11, T-110). Active with a bus; without it, the agent reports the problem
/// ([`crate::monitors::NO_DDC`]).
pub struct Monitors;

impl Module for Monitors {
    fn id(&self) -> ModuleId {
        ModuleId::Monitors
    }
    fn name(&self) -> &'static str {
        "Monitors"
    }
    fn detail(&self, _ctx: &ModuleContext<'_>) -> String {
        "Monitor input over DDC".into()
    }
    fn state(&self, _ctx: &ModuleContext<'_>) -> ModuleState {
        ModuleState::Active
    }
}

/// Wake-on-LAN and clean shutdown: active if it can do at least one of the three things.
pub struct Power;

impl Module for Power {
    fn id(&self) -> ModuleId {
        ModuleId::Power
    }
    fn name(&self) -> &'static str {
        "Power"
    }
    fn detail(&self, ctx: &ModuleContext<'_>) -> String {
        let p = ctx.power;
        let mut parts = Vec::new();
        parts.push(if p.power_off {
            "It shuts down on the other machine's request."
        } else {
            "It does not shut down on the other machine's request."
        });
        if p.wakeable {
            parts.push("It can be woken by Wake-on-LAN.");
        }
        if p.wake_peer {
            parts.push("It can wake the other by Wake-on-LAN.");
        }
        parts.join(" ")
    }
    fn state(&self, ctx: &ModuleContext<'_>) -> ModuleState {
        let p = ctx.power;
        if p.power_off || p.wakeable || p.wake_peer {
            ModuleState::Active
        } else {
            ModuleState::Unavailable {
                reason: "The other machine's MAC is missing for Wake-on-LAN: it is learned when pairing, or write it on its page"
                    .into(),
            }
        }
    }
}

/// Keyboard and mouse on the Windows secure desktop (UAC, sign-in): active while the
/// injector reaches the helper of T-027's system service ([`crate::secure`]).
pub struct SecureDesktop;

impl Module for SecureDesktop {
    fn id(&self) -> ModuleId {
        ModuleId::SecureDesktop
    }
    fn name(&self) -> &'static str {
        "Protected screens"
    }
    fn detail(&self, _ctx: &ModuleContext<'_>) -> String {
        "The mouse and keyboard also work on UAC prompts and at the sign-in screen.".into()
    }
    fn state(&self, _ctx: &ModuleContext<'_>) -> ModuleState {
        crate::secure::module_state(crate::secure::status())
    }
}

#[cfg(test)]
mod tests {
    use tandem_core::Machine;

    use super::*;

    fn power(wake_peer: bool, wakeable: bool, power_off: bool) -> PowerCaps {
        PowerCaps {
            wake_peer,
            wakeable,
            power_off,
            mac: None,
            wol_mac: None,
        }
    }

    fn state_of(reg: &Registry, ctx: &ModuleContext<'_>, id: ModuleId) -> ModuleState {
        reg.snapshot(ctx)
            .into_iter()
            .find(|m| m.id == id)
            .map(|m| m.state)
            .unwrap_or_else(|| panic!("missing {id:?}"))
    }

    /// The state of `id` on Windows, without anything power-related.
    fn on_pc(reg: &Registry, input: &InputStatus, s: &ModuleSwitches, id: ModuleId) -> ModuleState {
        let p = power(false, false, false);
        let ctx = ModuleContext {
            os: Os::Windows,
            input,
            power: &p,
            switches: s,
        };
        state_of(reg, &ctx, id)
    }

    /// A double: a module that says whatever it is told.
    struct Fake {
        id: ModuleId,
        state: ModuleState,
        switchable: bool,
    }

    impl Module for Fake {
        fn id(&self) -> ModuleId {
            self.id
        }
        fn name(&self) -> &'static str {
            "Fake"
        }
        fn detail(&self, _ctx: &ModuleContext<'_>) -> String {
            "double".into()
        }
        fn state(&self, _ctx: &ModuleContext<'_>) -> ModuleState {
            self.state.clone()
        }
        fn switchable(&self) -> bool {
            self.switchable
        }
    }

    /// T-158: Tunnels and Files exist; no reason says "not yet".
    #[test]
    fn tunnels_and_files_are_active_and_no_reason_says_not_yet() {
        for os in Os::ALL {
            let reg = Registry::for_os(os);
            let input = InputStatus::new(Machine::A);
            let switches = ModuleSwitches::default();
            let p = power(true, true, true);
            let ctx = ModuleContext {
                os,
                input: &input,
                power: &p,
                switches: &switches,
            };
            assert_eq!(state_of(&reg, &ctx, ModuleId::Tunnels), ModuleState::Active);
            assert_eq!(state_of(&reg, &ctx, ModuleId::Files), ModuleState::Active);
        }
    }

    /// Without a drag detector on the system, "Files" says so with a true reason.
    #[test]
    fn files_report_a_missing_drag_backend() {
        let reg = Registry::for_os(Os::Windows);
        let input = InputStatus::new(Machine::A);
        let mut switches = ModuleSwitches::default();
        switches.set_problem(ModuleId::Files, Some(NO_FILE_DRAG.into()));
        assert_eq!(
            on_pc(&reg, &input, &switches, ModuleId::Files),
            ModuleState::Unavailable {
                reason: NO_FILE_DRAG.into()
            }
        );
        switches.set_problem(ModuleId::Files, None);
        assert_eq!(
            on_pc(&reg, &input, &switches, ModuleId::Files),
            ModuleState::Active
        );
    }

    #[test]
    fn the_file_drag_problem_tells_the_true_cause() {
        assert_eq!(file_drag_problem(true, true), None);
        assert_eq!(file_drag_problem(false, true), None);
        assert_eq!(file_drag_problem(true, false), Some(NO_FILE_DRAG));
        assert_eq!(file_drag_problem(false, false), Some(NO_FILE_DRAG_OS));
        for r in [NO_FILE_DRAG, NO_FILE_DRAG_OS, NO_ENGINE] {
            assert!(!r.to_lowercase().contains("not yet"), "{r}");
        }
    }

    #[test]
    fn each_system_has_its_modules() {
        use ModuleId::*;
        assert_eq!(
            Registry::for_os(Os::Windows).ids(),
            [
                Input,
                Clipboard,
                Files,
                Tunnels,
                Monitors,
                Power,
                SecureDesktop
            ]
        );
        // Without a secure desktop on macOS: it does not exist, not even as "unavailable".
        assert_eq!(
            Registry::for_os(Os::MacOs).ids(),
            [Input, Clipboard, Files, Tunnels, Monitors, Power]
        );
    }

    #[test]
    fn every_module_is_announced_with_name_and_detail() {
        for os in Os::ALL {
            let input = InputStatus::new(Machine::A);
            let switches = ModuleSwitches::default();
            let p = power(true, true, true);
            let ctx = ModuleContext {
                os,
                input: &input,
                power: &p,
                switches: &switches,
            };
            for m in Registry::for_os(os).snapshot(&ctx) {
                assert!(!m.name.is_empty() && !m.detail.is_empty(), "{m:?}");
                assert_eq!(m.name, default_name(m.id));
                if let ModuleState::Unavailable { reason } = &m.state {
                    assert!(!reason.is_empty(), "{m:?}");
                }
            }
        }
    }

    /// The mockup's line for a module (`V3-Machine`), the text the demo also shows: the
    /// agent's wording cannot drift from it, which `pnpm shots` cannot catch in the demo.
    fn mockup_detail(name: &str) -> String {
        let html = include_str!("../../../mockups/v03/V3-Machine.dc.html");
        let tail = html
            .split_once(&format!(
                "<b style=\"font-weight:600\">{name}</b><span class=\"mut\">"
            ))
            .unwrap_or_else(|| panic!("V3-Machine has no row for {name}"))
            .1;
        tail.split_once("</span>")
            .expect("the row ends")
            .0
            .to_string()
    }

    #[test]
    fn protected_screens_reads_as_the_mockup() {
        let input = InputStatus::new(Machine::A);
        let switches = ModuleSwitches::default();
        let p = power(true, true, true);
        let ctx = ModuleContext {
            os: Os::Windows,
            input: &input,
            power: &p,
            switches: &switches,
        };
        let detail = SecureDesktop.detail(&ctx);
        assert_eq!(
            detail,
            "The mouse and keyboard also work on UAC prompts and at the sign-in screen."
        );
        assert_eq!(detail, mockup_detail("Protected screens"));
    }

    #[test]
    fn missing_permissions_are_named() {
        let reg = Registry::for_os(Os::MacOs);
        let mut input = InputStatus::new(Machine::A);
        input.missing = vec![InputPermission::Accessibility];
        let switches = ModuleSwitches::default();
        assert_eq!(
            on_pc(&reg, &input, &switches, ModuleId::Input),
            ModuleState::Unavailable {
                reason: "The Accessibility permission is missing".into()
            }
        );
        input.missing = vec![
            InputPermission::Accessibility,
            InputPermission::InputMonitoring,
        ];
        assert_eq!(
            on_pc(&reg, &input, &switches, ModuleId::Input),
            ModuleState::Unavailable {
                reason: "The Accessibility and Input Monitoring permissions are missing".into()
            }
        );
    }

    #[test]
    fn an_engine_problem_makes_input_unavailable() {
        let reg = Registry::for_os(Os::Windows);
        let mut input = InputStatus::new(Machine::B);
        input.problem = Some("could not install the keyboard hook".into());
        let switches = ModuleSwitches::default();
        let p = power(false, false, true);
        let ctx = ModuleContext {
            os: Os::Windows,
            input: &input,
            power: &p,
            switches: &switches,
        };
        assert_eq!(
            state_of(&reg, &ctx, ModuleId::Input),
            ModuleState::Unavailable {
                reason: "Could not install the keyboard hook".into()
            }
        );
    }

    #[test]
    fn power_says_what_it_can_do_or_what_is_missing() {
        let reg = Registry::for_os(Os::MacOs);
        let input = InputStatus::new(Machine::A);
        let switches = ModuleSwitches::default();
        let none = power(false, false, false);
        let ctx = ModuleContext {
            os: Os::MacOs,
            input: &input,
            power: &none,
            switches: &switches,
        };
        let ModuleState::Unavailable { reason } = state_of(&reg, &ctx, ModuleId::Power) else {
            panic!("with nothing to do, power is not available");
        };
        assert!(reason.contains("Wake-on-LAN"), "{reason}");
        let wol = power(true, false, false);
        let ctx = ModuleContext { power: &wol, ..ctx };
        assert_eq!(state_of(&reg, &ctx, ModuleId::Power), ModuleState::Active);
        let info = reg.snapshot(&ctx);
        let detail = &info
            .iter()
            .find(|m| m.id == ModuleId::Power)
            .unwrap()
            .detail;
        assert!(detail.contains("It can wake the other"), "{detail}");
    }

    #[test]
    fn the_user_switch_wins_and_problems_come_next() {
        let reg = Registry::with(vec![
            Box::new(Fake {
                id: ModuleId::Clipboard,
                state: ModuleState::Active,
                switchable: true,
            }),
            Box::new(Fake {
                id: ModuleId::Monitors,
                state: ModuleState::Active,
                switchable: false,
            }),
        ]);
        let input = InputStatus::new(Machine::A);
        let mut switches = ModuleSwitches::default();
        assert!(switches.set(ModuleId::Clipboard, false));
        assert!(!switches.set(ModuleId::Clipboard, false), "already was");
        assert!(switches.set_problem(ModuleId::Clipboard, Some("x".into())));
        assert!(switches.set_problem(ModuleId::Monitors, Some("no bus".into())));
        assert!(!switches.set_problem(ModuleId::Monitors, Some("no bus".into())));
        // A module that cannot be turned off is not left off even if requested.
        switches.set(ModuleId::Monitors, false);
        assert_eq!(
            on_pc(&reg, &input, &switches, ModuleId::Clipboard),
            ModuleState::Off
        );
        assert_eq!(
            on_pc(&reg, &input, &switches, ModuleId::Monitors),
            ModuleState::Unavailable {
                reason: "no bus".into()
            }
        );
        switches.set(ModuleId::Clipboard, true);
        assert_eq!(
            on_pc(&reg, &input, &switches, ModuleId::Clipboard),
            ModuleState::Unavailable { reason: "x".into() }
        );
        assert!(switches.set_problem(ModuleId::Clipboard, None));
        assert_eq!(
            on_pc(&reg, &input, &switches, ModuleId::Clipboard),
            ModuleState::Active
        );
    }

    #[test]
    fn clipboard_needs_the_input_engine() {
        let reg = Registry::for_os(Os::Windows);
        let input = InputStatus::new(Machine::B);
        let p = power(false, false, true);
        let mut switches = ModuleSwitches::default();
        switches.set(ModuleId::Input, false);
        let ctx = ModuleContext {
            os: Os::Windows,
            input: &input,
            power: &p,
            switches: &switches,
        };
        assert_eq!(state_of(&reg, &ctx, ModuleId::Input), ModuleState::Off);
        assert_eq!(
            state_of(&reg, &ctx, ModuleId::Clipboard),
            ModuleState::Unavailable {
                reason: "Needs \"Keyboard and mouse\" on".into()
            }
        );
    }

    #[test]
    fn only_existing_switchable_modules_can_be_switched() {
        let mac = Registry::for_os(Os::MacOs);
        assert_eq!(mac.check_switch(ModuleId::Input), Ok(()));
        assert_eq!(mac.check_switch(ModuleId::Clipboard), Ok(()));
        let e = mac.check_switch(ModuleId::SecureDesktop).unwrap_err();
        assert!(e.contains("does not exist"), "{e}");
        let e = mac.check_switch(ModuleId::Power).unwrap_err();
        assert!(e.contains("cannot"), "{e}");
        let pc = Registry::for_os(Os::Windows);
        assert!(pc.check_switch(ModuleId::SecureDesktop).is_err());
    }

    proptest::proptest! {
        /// What is off is exactly what was left off, in any order of changes.
        #[test]
        fn switches_keep_the_last_word(ops in proptest::collection::vec((0usize..7, proptest::bool::ANY), 0..40)) {
            let mut s = ModuleSwitches::default();
            let mut last = BTreeMap::new();
            for (i, on) in ops {
                let id = ModuleId::ALL[i];
                s.set(id, on);
                last.insert(id, on);
            }
            for id in ModuleId::ALL {
                proptest::prop_assert_eq!(s.is_on(id), *last.get(&id).unwrap_or(&true));
            }
        }
    }
}
