//! La barra de menú (macOS) y la bandeja (Windows): qué muestra el menú del ícono y dónde
//! se abre la mini escena. Los modos de monitores están en pausa hasta el firmware propio
//! (DESIGN §8): el menú ya no los ofrece.
//!
//! Todo es puro sobre el [`Snapshot`] para probarlo sin Tauri; `app` solo lo dibuja.

use crate::session::{PeerView, Snapshot};

/// Lo que muestra el menú del ícono.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuView {
    /// Texto al pasar el mouse por el ícono.
    pub tooltip: String,
    /// Línea de estado, deshabilitada, arriba del menú.
    pub status: String,
}

impl MenuView {
    pub fn of(snapshot: &Snapshot) -> Self {
        match snapshot {
            Snapshot::Connected {
                agent,
                state,
                peer,
                caps,
                desk,
                ..
            } => {
                // Esta máquina está prendida (es la que muestra el menú): se dice cómo
                // está la otra, sea cual sea (DESIGN §3.7), con su nombre: el que anunció
                // o el del puesto (T-097).
                let other = agent.other();
                let name = caps
                    .peer
                    .as_ref()
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| desk.machines.name(other));
                let power = if state.is_on(other) {
                    format!("{name} encendida")
                } else {
                    format!("{name} apagada")
                };
                let mut status = power.clone();
                if *peer == PeerView::Unpaired {
                    status.push_str(" · sin par emparejado");
                }
                MenuView {
                    tooltip: format!("Tandem · {power}"),
                    status,
                }
            }
            Snapshot::Connecting => MenuView {
                tooltip: "Tandem · conectando…".into(),
                status: "Conectando con tandemd…".into(),
            },
            Snapshot::Unavailable { .. } => MenuView {
                tooltip: "Tandem · sin agente".into(),
                status: "Sin agente".into(),
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
    use tandem_core::{Config, Machine, State};
    use tandem_proto::InputStatus;

    use super::*;
    use crate::session::PairingView;

    use Machine::{A, B};

    fn paired() -> PeerView {
        PeerView::Paired {
            name: "PC".into(),
            link: None,
        }
    }

    /// El puesto de las pruebas: A se llama «Mac» y B «PC».
    fn desk() -> Config {
        let mut d = Config::default();
        d.machines.a = Some(tandem_core::MachineInfo::new(
            A,
            tandem_core::Os::MacOs,
            None,
        ));
        d.machines.b = Some(tandem_core::MachineInfo::new(
            B,
            tandem_core::Os::Windows,
            None,
        ));
        d
    }

    fn connected(agent: Machine, state: State, peer: PeerView) -> Snapshot {
        Snapshot::Connected {
            agent,
            desk: desk(),
            state,
            peer,
            pairing: PairingView::Idle,
            input: InputStatus::new(agent).into(),
            caps: crate::session::test_caps(agent),
        }
    }

    #[test]
    fn the_menu_says_how_the_other_machine_is() {
        let mut on = State::initial(A);
        on.on.b = true;
        let v = MenuView::of(&connected(A, on, paired()));
        assert_eq!(v.tooltip, "Tandem · PC encendida");
        assert_eq!(v.status, "PC encendida");

        let off = MenuView::of(&connected(A, State::initial(A), paired()));
        assert_eq!(off.status, "PC apagada");

        // Sin par, el menú lo dice: la PC no va a responder.
        let alone = MenuView::of(&connected(A, State::initial(A), PeerView::Unpaired));
        assert_eq!(alone.status, "PC apagada · sin par emparejado");

        // En la PC: la que se dice apagada es la Mac, nunca la PC misma.
        let on_pc = MenuView::of(&connected(B, State::initial(B), paired()));
        assert_eq!(on_pc.status, "Mac apagada");
        // Sin nada guardado de la otra, «máquina A», nunca un «Mac» inventado.
        let mut bare = connected(B, State::initial(B), paired());
        if let Snapshot::Connected { desk, .. } = &mut bare {
            *desk = Config::default();
        }
        assert_eq!(MenuView::of(&bare).status, "máquina A apagada");
    }

    #[test]
    fn the_menu_without_agent_says_so() {
        assert_eq!(
            MenuView::of(&Snapshot::Connecting).status,
            "Conectando con tandemd…"
        );
        let v = MenuView::of(&Snapshot::Unavailable {
            endpoint: None,
            reason: "x".into(),
        });
        assert_eq!(v.status, "Sin agente");
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
