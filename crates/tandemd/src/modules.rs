//! Los módulos de cada máquina y su estado (DECISIONS D3, DESIGN §4.3, T-098).
//!
//! Un módulo existe solo en los sistemas donde puede funcionar ([`Registry::for_os`]) y en
//! ejecución dice si está activo, apagado por el usuario o no disponible, con una razón
//! concreta. Su estado sale de lo que el agente ya sabe ([`ModuleContext`]): los módulos no
//! guardan nada propio, así que se prueban con contextos armados a mano.
//!
//! Qué módulos existen se decide por el [`Os`] de la máquina y no por `#[cfg]` directo: en
//! Linux (pruebas y simulación) hay que poder armar el registro de los dos sistemas. Lo que
//! sí depende de la plataforma (las sondas reales) vive detrás de `#[cfg]` en cada módulo.

use std::collections::{BTreeMap, BTreeSet};

use tandem_core::Os;
use tandem_proto::{InputPermission, InputStatus, ModuleId, ModuleInfo, ModuleState, PowerCaps};

/// Lo que dice un módulo que todavía no está implementado en esta versión.
pub const NOT_YET: &str = "Aún no está en esta versión de Tandem";

/// Lo que el agente sabe y los módulos miran para decir su estado.
#[derive(Debug, Clone, Copy)]
pub struct ModuleContext<'a> {
    pub os: Os,
    pub input: &'a InputStatus,
    pub power: &'a PowerCaps,
    pub switches: &'a ModuleSwitches,
}

/// Lo que el usuario apagó y los problemas que reportó cada módulo en marcha.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleSwitches {
    off: BTreeSet<ModuleId>,
    problems: BTreeMap<ModuleId, String>,
}

impl ModuleSwitches {
    pub fn is_on(&self, id: ModuleId) -> bool {
        !self.off.contains(&id)
    }

    /// `true` si cambió.
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

    /// `true` si cambió.
    pub fn set_problem(&mut self, id: ModuleId, problem: Option<String>) -> bool {
        match problem {
            Some(p) => self.problems.insert(id, p.clone()).as_ref() != Some(&p),
            None => self.problems.remove(&id).is_some(),
        }
    }
}

/// Un módulo de Tandem.
pub trait Module: Send + Sync {
    fn id(&self) -> ModuleId;
    fn name(&self) -> &'static str;
    /// Qué hace en esta máquina (la línea de la UI).
    fn detail(&self, ctx: &ModuleContext<'_>) -> String;
    /// Su estado sin contar el interruptor del usuario ni los problemas reportados en
    /// marcha (eso lo suma el [`Registry`]).
    fn state(&self, ctx: &ModuleContext<'_>) -> ModuleState;
    /// El usuario lo puede encender y apagar. Solo los que el agente sabe detener de verdad.
    fn switchable(&self) -> bool {
        false
    }
}

/// Los módulos que existen en una máquina.
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
    /// Los de una máquina con `os`, en el orden de la UI.
    pub fn for_os(os: Os) -> Self {
        let mut modules: Vec<Box<dyn Module>> = vec![
            Box::new(Input),
            Box::new(Clipboard),
            Box::new(Files),
            Box::new(Tunnels),
            Box::new(Monitors),
            Box::new(Power),
        ];
        // El escritorio seguro (UAC, inicio de sesión) es cosa de Windows.
        if os == Os::Windows {
            modules.push(Box::new(SecureDesktop));
        }
        Registry { modules }
    }

    /// Con módulos a medida (las pruebas).
    pub fn with(modules: Vec<Box<dyn Module>>) -> Self {
        Registry { modules }
    }

    pub fn ids(&self) -> Vec<ModuleId> {
        self.modules.iter().map(|m| m.id()).collect()
    }

    fn get(&self, id: ModuleId) -> Option<&dyn Module> {
        self.modules.iter().find(|m| m.id() == id).map(|m| &**m)
    }

    /// El anuncio de cada módulo. Apagado por el usuario manda sobre todo lo demás: al
    /// encenderlo se ve la razón si no puede funcionar.
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

    /// Si el usuario puede encender o apagar `id` aquí; si no, la razón para la UI.
    pub fn check_switch(&self, id: ModuleId) -> Result<(), String> {
        match self.get(id) {
            None => Err(format!("«{}» no existe en esta máquina", default_name(id))),
            Some(m) if !m.switchable() => {
                Err(format!("«{}» no se puede encender ni apagar", m.name()))
            }
            Some(_) => Ok(()),
        }
    }
}

/// El nombre de un módulo aunque no exista en esta máquina (para los mensajes).
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

/// Teclado y mouse compartidos: no funciona sin los permisos de macOS o si su motor falló.
pub struct Input;

impl Module for Input {
    fn id(&self) -> ModuleId {
        ModuleId::Input
    }
    fn name(&self) -> &'static str {
        "Teclado y mouse"
    }
    fn detail(&self, ctx: &ModuleContext<'_>) -> String {
        match ctx.os {
            Os::MacOs => "Captura con CGEventTap e inyección con CGEventPost".into(),
            Os::Windows => "Captura e inyección (hooks de bajo nivel, SendInput)".into(),
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
        InputPermission::Accessibility => "Accesibilidad",
        InputPermission::InputMonitoring => "Monitorización de entrada",
    }
}

/// «Falta el permiso de Accesibilidad» / «Faltan los permisos de A y B».
fn missing_permissions(missing: &[InputPermission]) -> Option<String> {
    match missing {
        [] => None,
        [one] => Some(format!("Falta el permiso de {}", permission_name(*one))),
        many => {
            let names: Vec<_> = many.iter().map(|p| permission_name(*p)).collect();
            let (last, rest) = names.split_last()?;
            Some(format!(
                "Faltan los permisos de {} y {last}",
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

/// El portapapeles compartido: corre dentro del motor de teclado y mouse, que dice a qué
/// máquina va el foco.
pub struct Clipboard;

impl Module for Clipboard {
    fn id(&self) -> ModuleId {
        ModuleId::Clipboard
    }
    fn name(&self) -> &'static str {
        "Portapapeles"
    }
    fn detail(&self, _ctx: &ModuleContext<'_>) -> String {
        "Texto, HTML e imagen".into()
    }
    fn state(&self, ctx: &ModuleContext<'_>) -> ModuleState {
        if ctx.switches.is_on(ModuleId::Input) {
            ModuleState::Active
        } else {
            ModuleState::Unavailable {
                reason: format!("Necesita «{}» encendido", Input.name()),
            }
        }
    }
    fn switchable(&self) -> bool {
        true
    }
}

/// Arrastrar archivos entre máquinas (D14).
pub struct Files;

impl Module for Files {
    fn id(&self) -> ModuleId {
        ModuleId::Files
    }
    fn name(&self) -> &'static str {
        "Archivos"
    }
    fn detail(&self, _ctx: &ModuleContext<'_>) -> String {
        "Arrastrar archivos al cruzar el borde".into()
    }
    fn state(&self, _ctx: &ModuleContext<'_>) -> ModuleState {
        ModuleState::Unavailable {
            reason: NOT_YET.into(),
        }
    }
}

/// Túneles de Claude (D8).
pub struct Tunnels;

impl Module for Tunnels {
    fn id(&self) -> ModuleId {
        ModuleId::Tunnels
    }
    fn name(&self) -> &'static str {
        "Túneles"
    }
    fn detail(&self, _ctx: &ModuleContext<'_>) -> String {
        "Recibe túneles de Claude".into()
    }
    fn state(&self, _ctx: &ModuleContext<'_>) -> ModuleState {
        ModuleState::Unavailable {
            reason: NOT_YET.into(),
        }
    }
}

/// Monitores por DDC (D11, T-110). Activo con un bus; sin él, el agente reporta el problema
/// ([`crate::monitors::NO_DDC`]).
pub struct Monitors;

impl Module for Monitors {
    fn id(&self) -> ModuleId {
        ModuleId::Monitors
    }
    fn name(&self) -> &'static str {
        "Monitores"
    }
    fn detail(&self, _ctx: &ModuleContext<'_>) -> String {
        "Entrada de los monitores por DDC".into()
    }
    fn state(&self, _ctx: &ModuleContext<'_>) -> ModuleState {
        ModuleState::Active
    }
}

/// Wake-on-LAN y apagado limpio: activo si sabe hacer al menos una de las tres cosas.
pub struct Power;

impl Module for Power {
    fn id(&self) -> ModuleId {
        ModuleId::Power
    }
    fn name(&self) -> &'static str {
        "Energía"
    }
    fn detail(&self, ctx: &ModuleContext<'_>) -> String {
        let p = ctx.power;
        let mut parts = Vec::new();
        parts.push(if p.power_off {
            "Se apaga a pedido de la otra máquina."
        } else {
            "No se apaga a pedido de la otra máquina."
        });
        if p.wakeable {
            parts.push("Se deja despertar por Wake-on-LAN.");
        }
        if p.wake_peer {
            parts.push("Puede despertar a la otra por Wake-on-LAN.");
        }
        parts.join(" ")
    }
    fn state(&self, ctx: &ModuleContext<'_>) -> ModuleState {
        let p = ctx.power;
        if p.power_off || p.wakeable || p.wake_peer {
            ModuleState::Active
        } else {
            ModuleState::Unavailable {
                reason: "Falta configurar Wake-on-LAN ([power.wol] con la MAC de la otra máquina)"
                    .into(),
            }
        }
    }
}

/// Teclado y mouse en el escritorio seguro de Windows (UAC, inicio de sesión). Necesita el
/// servicio de sistema de T-027, que todavía no existe.
pub struct SecureDesktop;

impl Module for SecureDesktop {
    fn id(&self) -> ModuleId {
        ModuleId::SecureDesktop
    }
    fn name(&self) -> &'static str {
        "Pantallas protegidas"
    }
    fn detail(&self, _ctx: &ModuleContext<'_>) -> String {
        "El mouse y el teclado también en avisos de UAC y en el inicio de sesión".into()
    }
    fn state(&self, _ctx: &ModuleContext<'_>) -> ModuleState {
        ModuleState::Unavailable {
            reason: "Necesita el servicio de sistema de Tandem, que aún no está en esta versión"
                .into(),
        }
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
        }
    }

    fn state_of(reg: &Registry, ctx: &ModuleContext<'_>, id: ModuleId) -> ModuleState {
        reg.snapshot(ctx)
            .into_iter()
            .find(|m| m.id == id)
            .map(|m| m.state)
            .unwrap_or_else(|| panic!("falta {id:?}"))
    }

    /// El estado de `id` en Windows, sin nada de energía.
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

    /// Un doble: un módulo que dice lo que se le pida.
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
            "Falso"
        }
        fn detail(&self, _ctx: &ModuleContext<'_>) -> String {
            "doble".into()
        }
        fn state(&self, _ctx: &ModuleContext<'_>) -> ModuleState {
            self.state.clone()
        }
        fn switchable(&self) -> bool {
            self.switchable
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
        // Sin escritorio seguro en macOS: no existe, ni siquiera como «no disponible».
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

    #[test]
    fn missing_permissions_are_named() {
        let reg = Registry::for_os(Os::MacOs);
        let mut input = InputStatus::new(Machine::A);
        input.missing = vec![InputPermission::Accessibility];
        let switches = ModuleSwitches::default();
        assert_eq!(
            on_pc(&reg, &input, &switches, ModuleId::Input),
            ModuleState::Unavailable {
                reason: "Falta el permiso de Accesibilidad".into()
            }
        );
        input.missing = vec![
            InputPermission::Accessibility,
            InputPermission::InputMonitoring,
        ];
        assert_eq!(
            on_pc(&reg, &input, &switches, ModuleId::Input),
            ModuleState::Unavailable {
                reason: "Faltan los permisos de Accesibilidad y Monitorización de entrada".into()
            }
        );
    }

    #[test]
    fn an_engine_problem_makes_input_unavailable() {
        let reg = Registry::for_os(Os::Windows);
        let mut input = InputStatus::new(Machine::B);
        input.problem = Some("no se pudo instalar el hook del teclado".into());
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
                reason: "No se pudo instalar el hook del teclado".into()
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
            panic!("sin nada que hacer, energía no está disponible");
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
        assert!(detail.contains("Puede despertar a la otra"), "{detail}");
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
        assert!(!switches.set(ModuleId::Clipboard, false), "ya estaba");
        assert!(switches.set_problem(ModuleId::Clipboard, Some("x".into())));
        assert!(switches.set_problem(ModuleId::Monitors, Some("sin bus".into())));
        assert!(!switches.set_problem(ModuleId::Monitors, Some("sin bus".into())));
        // Un módulo que no se puede apagar no queda apagado aunque se pida.
        switches.set(ModuleId::Monitors, false);
        assert_eq!(
            on_pc(&reg, &input, &switches, ModuleId::Clipboard),
            ModuleState::Off
        );
        assert_eq!(
            on_pc(&reg, &input, &switches, ModuleId::Monitors),
            ModuleState::Unavailable {
                reason: "sin bus".into()
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
                reason: "Necesita «Teclado y mouse» encendido".into()
            }
        );
    }

    #[test]
    fn only_existing_switchable_modules_can_be_switched() {
        let mac = Registry::for_os(Os::MacOs);
        assert_eq!(mac.check_switch(ModuleId::Input), Ok(()));
        assert_eq!(mac.check_switch(ModuleId::Clipboard), Ok(()));
        let e = mac.check_switch(ModuleId::SecureDesktop).unwrap_err();
        assert!(e.contains("no existe"), "{e}");
        let e = mac.check_switch(ModuleId::Power).unwrap_err();
        assert!(e.contains("no se puede"), "{e}");
        let pc = Registry::for_os(Os::Windows);
        assert!(pc.check_switch(ModuleId::SecureDesktop).is_err());
    }

    proptest::proptest! {
        /// Lo apagado es exactamente lo que quedó apagado, en cualquier orden de cambios.
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
