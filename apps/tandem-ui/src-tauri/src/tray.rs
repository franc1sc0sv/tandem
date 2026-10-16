//! La barra de menú (macOS) y la bandeja (Windows): qué muestra el menú del ícono, qué
//! modo pide cada entrada (y cada atajo global) y dónde se abre la mini escena.
//!
//! Todo es puro sobre el [`Snapshot`] para probarlo sin Tauri; `app` solo lo dibuja.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tandem_core::{Config, Machine, Mode, MonitorId, State};

use crate::session::{PeerView, Snapshot};
use crate::shortcuts::ShortcutAction;

/// Las tres tarjetas de modo, como las llama la escena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeKey {
    SoloMac,
    Shared,
    SoloPc,
}

impl ModeKey {
    pub const ALL: [ModeKey; 3] = [ModeKey::SoloMac, ModeKey::Shared, ModeKey::SoloPc];

    pub fn label(self) -> &'static str {
        match self {
            ModeKey::SoloMac => "Solo Mac",
            ModeKey::Shared => "Compartido",
            ModeKey::SoloPc => "Solo Windows",
        }
    }

    pub fn of_action(a: ShortcutAction) -> Option<ModeKey> {
        match a {
            ShortcutAction::SoloMac => Some(ModeKey::SoloMac),
            ShortcutAction::Shared => Some(ModeKey::Shared),
            ShortcutAction::SoloPc => Some(ModeKey::SoloPc),
            ShortcutAction::ToggleWindow => None,
        }
    }
}

/// Un monitor sin dueño guardado está en la Mac: el estado seguro de partida.
fn owner_of(state: &State, id: &MonitorId) -> Machine {
    state.owner.get(id).copied().unwrap_or(Machine::Mac)
}

/// El modo que describe el reparto actual (igual que `modeOf` de la escena).
pub fn mode_of(desk: &Config, state: &State) -> Option<ModeKey> {
    if desk.monitors.is_empty() {
        return None;
    }
    let owners: Vec<Machine> = desk
        .monitors
        .iter()
        .map(|m| owner_of(state, &m.id))
        .collect();
    Some(if owners.iter().all(|&o| o == Machine::Mac) {
        ModeKey::SoloMac
    } else if owners.iter().all(|&o| o == Machine::Pc) {
        ModeKey::SoloPc
    } else {
        ModeKey::Shared
    })
}

/// Qué pedir al elegir un modo, con la misma regla que las tarjetas de la escena
/// (`modeRequest`): Compartido desde un modo «solo» manda el primer monitor a la PC y deja
/// el resto en la Mac; si ya es compartido, conserva el reparto.
pub fn mode_request(key: ModeKey, desk: &Config, state: &State) -> Mode {
    match key {
        ModeKey::SoloMac => Mode::SoloMac,
        ModeKey::SoloPc => Mode::SoloPc,
        ModeKey::Shared => {
            let keep = mode_of(desk, state) == Some(ModeKey::Shared);
            let assign: BTreeMap<MonitorId, Machine> = desk
                .monitors
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let owner = if keep {
                        owner_of(state, &m.id)
                    } else if i == 0 {
                        Machine::Pc
                    } else {
                        Machine::Mac
                    };
                    (m.id.clone(), owner)
                })
                .collect();
            Mode::Shared(assign)
        }
    }
}

/// Por qué no se puede pedir un modo ahora.
pub fn mode_for(key: ModeKey, snapshot: &Snapshot) -> Result<Mode, String> {
    match snapshot {
        Snapshot::Connected { desk, state, .. } => Ok(mode_request(key, desk, state)),
        Snapshot::Connecting => Err("todavía conectando con tandemd".into()),
        Snapshot::Unavailable { reason, .. } => Err(format!("sin agente: {reason}")),
    }
}

/// Lo que muestra el menú del ícono.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuView {
    /// Texto al pasar el mouse por el ícono.
    pub tooltip: String,
    /// Línea de estado, deshabilitada, arriba del menú.
    pub status: String,
    /// Modo actual (marcado), si hay agente.
    pub current: Option<ModeKey>,
    /// Los modos solo se pueden pedir con agente y con monitores.
    pub modes_enabled: bool,
}

impl MenuView {
    pub fn of(snapshot: &Snapshot) -> Self {
        match snapshot {
            Snapshot::Connected {
                desk, state, peer, ..
            } => {
                let current = mode_of(desk, state);
                let mode = current.map_or("sin monitores", ModeKey::label);
                let pc = if state.pc_on {
                    "PC encendida"
                } else {
                    "PC apagada"
                };
                let mut status = format!("{mode} · {pc}");
                if *peer == PeerView::Unpaired {
                    status.push_str(" · sin par emparejado");
                }
                MenuView {
                    tooltip: format!("Tandem · {mode}"),
                    status,
                    current,
                    modes_enabled: current.is_some(),
                }
            }
            Snapshot::Connecting => MenuView {
                tooltip: "Tandem · conectando…".into(),
                status: "Conectando con tandemd…".into(),
                current: None,
                modes_enabled: false,
            },
            Snapshot::Unavailable { .. } => MenuView {
                tooltip: "Tandem · sin agente".into(),
                status: "Sin agente".into(),
                current: None,
                modes_enabled: false,
            },
        }
    }
}

/// Un rectángulo en píxeles físicos del escritorio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Separación entre la mini escena y la barra (o el borde de la pantalla).
pub const POPOVER_GAP: f64 = 8.0;

/// Dónde abrir la mini escena (`w`×`h`) tras un clic en `click`, dentro del área útil de
/// la pantalla (sin la barra de menú ni la barra de tareas).
///
/// Centrada bajo el ícono si la barra está arriba (macOS) y encima si está abajo (la
/// bandeja de Windows); nunca fuera del área útil.
pub fn popover_origin(click: (f64, f64), w: f64, h: f64, work: Area) -> (f64, f64) {
    let (cx, cy) = click;
    let clamp = |v: f64, lo: f64, hi: f64| v.max(lo).min(hi.max(lo));
    let x = clamp(
        cx - w / 2.0,
        work.x + POPOVER_GAP,
        work.x + work.w - w - POPOVER_GAP,
    );
    let top_half = cy < work.y + work.h / 2.0;
    let y = if top_half {
        work.y + POPOVER_GAP
    } else {
        work.y + work.h - h - POPOVER_GAP
    };
    (x, clamp(y, work.y, work.y + work.h - h))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tandem_core::BuiltinDisplay;
    use tandemd::AgentConfig;

    use super::*;
    use crate::session::PairingView;

    fn paired() -> PeerView {
        PeerView::Paired {
            name: "PC".into(),
            link: None,
        }
    }

    const CONFIG: &str = r#"
machine = "mac"

[[monitors]]
id = "m34"
label = '34"'
wiring = { pc = "displayport", mac = "hdmi" }

[[monitors]]
id = "m32"
label = '32"'
wiring = { pc = "displayport", mac = "hdmi" }
"#;

    fn desk() -> Config {
        AgentConfig::from_toml(CONFIG).unwrap().desk
    }

    fn state(m34: Machine, m32: Machine) -> State {
        State {
            owner: [("m34".into(), m34), ("m32".into(), m32)].into(),
            pc_on: m34 == Machine::Pc || m32 == Machine::Pc,
            builtin_pref: BuiltinDisplay::Off,
        }
    }

    use Machine::{Mac, Pc};

    #[test]
    fn mode_follows_the_owners() {
        let d = desk();
        assert_eq!(mode_of(&d, &state(Mac, Mac)), Some(ModeKey::SoloMac));
        assert_eq!(mode_of(&d, &state(Pc, Pc)), Some(ModeKey::SoloPc));
        assert_eq!(mode_of(&d, &state(Pc, Mac)), Some(ModeKey::Shared));
        assert_eq!(mode_of(&d, &State::initial(&d)), Some(ModeKey::SoloMac));
        let mut unknown = state(Mac, Mac);
        unknown.owner.clear();
        assert_eq!(mode_of(&d, &unknown), Some(ModeKey::SoloMac));
        let empty = Config { monitors: vec![] };
        assert_eq!(mode_of(&empty, &state(Mac, Mac)), None);
    }

    #[test]
    fn requests_match_the_scene_cards() {
        let d = desk();
        assert_eq!(
            mode_request(ModeKey::SoloMac, &d, &state(Pc, Pc)),
            Mode::SoloMac
        );
        assert_eq!(
            mode_request(ModeKey::SoloPc, &d, &state(Mac, Mac)),
            Mode::SoloPc
        );
        // Desde un modo «solo», el primero (el de la izquierda) a la PC.
        let first_to_pc = Mode::Shared([("m34".into(), Pc), ("m32".into(), Mac)].into());
        assert_eq!(
            mode_request(ModeKey::Shared, &d, &state(Mac, Mac)),
            first_to_pc
        );
        assert_eq!(
            mode_request(ModeKey::Shared, &d, &state(Pc, Pc)),
            first_to_pc
        );
        // Ya compartido: conserva el reparto.
        assert_eq!(
            mode_request(ModeKey::Shared, &d, &state(Mac, Pc)),
            Mode::Shared([("m34".into(), Mac), ("m32".into(), Pc)].into())
        );
    }

    #[test]
    fn modes_need_an_agent() {
        let d = desk();
        let s = state(Mac, Mac);
        let ok = Snapshot::Connected {
            agent: Mac,
            desk: d,
            state: s,
            peer: PeerView::Unpaired,
            pairing: PairingView::Idle,
        };
        assert_eq!(mode_for(ModeKey::SoloPc, &ok), Ok(Mode::SoloPc));
        let down = Snapshot::Unavailable {
            endpoint: None,
            reason: "no está corriendo".into(),
        };
        assert_eq!(
            mode_for(ModeKey::SoloPc, &down),
            Err("sin agente: no está corriendo".into())
        );
        assert!(mode_for(ModeKey::SoloPc, &Snapshot::Connecting).is_err());
    }

    #[test]
    fn shortcut_actions_map_to_modes() {
        assert_eq!(
            ModeKey::of_action(ShortcutAction::SoloMac),
            Some(ModeKey::SoloMac)
        );
        assert_eq!(
            ModeKey::of_action(ShortcutAction::Shared),
            Some(ModeKey::Shared)
        );
        assert_eq!(
            ModeKey::of_action(ShortcutAction::SoloPc),
            Some(ModeKey::SoloPc)
        );
        assert_eq!(ModeKey::of_action(ShortcutAction::ToggleWindow), None);
    }

    #[test]
    fn menu_marks_the_current_mode() {
        let v = MenuView::of(&Snapshot::Connected {
            agent: Mac,
            desk: desk(),
            state: state(Pc, Mac),
            peer: paired(),
            pairing: PairingView::Idle,
        });
        assert_eq!(v.current, Some(ModeKey::Shared));
        assert!(v.modes_enabled);
        assert_eq!(v.tooltip, "Tandem · Compartido");
        assert_eq!(v.status, "Compartido · PC encendida");

        let off = MenuView::of(&Snapshot::Connected {
            agent: Mac,
            desk: desk(),
            state: State::initial(&desk()),
            peer: paired(),
            pairing: PairingView::Idle,
        });
        assert_eq!(off.status, "Solo Mac · PC apagada");

        // Sin par, el menú lo dice: la PC no va a responder.
        let alone = MenuView::of(&Snapshot::Connected {
            agent: Mac,
            desk: desk(),
            state: State::initial(&desk()),
            peer: PeerView::Unpaired,
            pairing: PairingView::Idle,
        });
        assert_eq!(alone.status, "Solo Mac · PC apagada · sin par emparejado");
    }

    #[test]
    fn menu_without_agent_disables_the_modes() {
        for s in [
            Snapshot::Connecting,
            Snapshot::Unavailable {
                endpoint: None,
                reason: "x".into(),
            },
        ] {
            let v = MenuView::of(&s);
            assert_eq!(v.current, None);
            assert!(!v.modes_enabled);
        }
        let empty = Snapshot::Connected {
            agent: Mac,
            desk: Config { monitors: vec![] },
            state: state(Mac, Mac),
            peer: paired(),
            pairing: PairingView::Idle,
        };
        let v = MenuView::of(&empty);
        assert!(!v.modes_enabled);
        assert_eq!(v.status, "sin monitores · PC apagada");
    }

    const SCREEN: Area = Area {
        x: 0.0,
        y: 50.0,
        w: 2000.0,
        h: 1000.0,
    };

    #[test]
    fn opens_under_a_menu_bar_icon() {
        // macOS: ícono arriba a la derecha; el área útil empieza bajo la barra.
        let (x, y) = popover_origin((1500.0, 20.0), 400.0, 300.0, SCREEN);
        assert_eq!((x, y), (1300.0, 50.0 + POPOVER_GAP));
    }

    #[test]
    fn opens_above_a_tray_icon() {
        // Windows: bandeja abajo a la derecha, pegada al borde.
        let (x, y) = popover_origin((1990.0, 1070.0), 400.0, 300.0, SCREEN);
        assert_eq!(x, 2000.0 - 400.0 - POPOVER_GAP);
        assert_eq!(y, 50.0 + 1000.0 - 300.0 - POPOVER_GAP);
    }

    proptest! {
        /// Donde sea el clic, la mini escena queda entera dentro del área útil.
        #[test]
        fn always_inside_the_work_area(
            cx in -500.0f64..3000.0,
            cy in -500.0f64..2000.0,
            w in 100.0f64..600.0,
            h in 100.0f64..600.0,
        ) {
            let (x, y) = popover_origin((cx, cy), w, h, SCREEN);
            prop_assert!(x >= SCREEN.x && x + w <= SCREEN.x + SCREEN.w);
            prop_assert!(y >= SCREEN.y && y + h <= SCREEN.y + SCREEN.h);
        }
    }
}
