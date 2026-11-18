//! The menu bar (macOS) and the tray (Windows): what the icon menu shows and where
//! the mini scene opens. The former modes no longer exist: scenes replace them
//! (D6), which the menu does not offer yet.
//!
//! Everything is pure over the [`Snapshot`] to test it without Tauri; `app` only draws it.

use crate::session::{PeerView, Snapshot};

/// What the icon menu shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuView {
    /// Text when hovering the mouse over the icon.
    pub tooltip: String,
    /// Status line, disabled, at the top of the menu.
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
                // This machine is on (it is the one showing the menu): how the other one
                // is is told, whichever it is (DESIGN §3.7), with its name: the one it announced
                // or the desk's (T-097).
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
                    status.push_str(" · no paired peer");
                }
                MenuView {
                    tooltip: format!("Tandem · {power}"),
                    status,
                }
            }
            Snapshot::Connecting => MenuView {
                tooltip: "Tandem · conectando…".into(),
                status: "Connecting to tandemd…".into(),
            },
            Snapshot::Unavailable { .. } => MenuView {
                tooltip: "Tandem · no agent".into(),
                status: "No agent".into(),
            },
        }
    }
}

/// A rectangle in physical desktop pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Gap between the mini scene and the bar (or the screen edge).
pub const POPOVER_GAP: f64 = 8.0;

/// Where to open the mini scene (`w`×`h`) after a click at `click`, within the usable area of
/// the screen (without the menu bar or the taskbar).
///
/// Centered under the icon if the bar is at the top (macOS) and above if it is at the bottom (the
/// Windows tray); never outside the usable area.
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

    /// The test desk: A is called "Mac" and B "PC".
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
            monitors: vec![],
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

        // Without a peer, the menu says so: the PC is not going to respond.
        let alone = MenuView::of(&connected(A, State::initial(A), PeerView::Unpaired));
        assert_eq!(alone.status, "PC off · no paired peer");

        // On the PC: the one that is said to be off is the Mac, never the PC itself.
        let on_pc = MenuView::of(&connected(B, State::initial(B), paired()));
        assert_eq!(on_pc.status, "Mac apagada");
        // With nothing saved about the other, "machine A", never an invented "Mac".
        let mut bare = connected(B, State::initial(B), paired());
        if let Snapshot::Connected { desk, .. } = &mut bare {
            *desk = Config::default();
        }
        assert_eq!(MenuView::of(&bare).status, "machine A off");
    }

    #[test]
    fn the_menu_without_agent_says_so() {
        assert_eq!(
            MenuView::of(&Snapshot::Connecting).status,
            "Connecting to tandemd…"
        );
        let v = MenuView::of(&Snapshot::Unavailable {
            endpoint: None,
            reason: "x".into(),
        });
        assert_eq!(v.status, "No agent");
    }

    const SCREEN: Area = Area {
        x: 0.0,
        y: 50.0,
        w: 2000.0,
        h: 1000.0,
    };

    #[test]
    fn opens_under_a_menu_bar_icon() {
        // macOS: icon at the top right; the usable area starts under the bar.
        let (x, y) = popover_origin((1500.0, 20.0), 400.0, 300.0, SCREEN);
        assert_eq!((x, y), (1300.0, 50.0 + POPOVER_GAP));
    }

    #[test]
    fn opens_above_a_tray_icon() {
        // Windows: tray at the bottom right, flush with the edge.
        let (x, y) = popover_origin((1990.0, 1070.0), 400.0, 300.0, SCREEN);
        assert_eq!(x, 2000.0 - 400.0 - POPOVER_GAP);
        assert_eq!(y, 50.0 + 1000.0 - 300.0 - POPOVER_GAP);
    }

    proptest! {
        /// Wherever the click is, the mini scene stays entirely inside the usable area.
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
