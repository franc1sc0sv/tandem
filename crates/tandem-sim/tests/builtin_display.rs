//! The Mac's built-in display mode (T-351) with two real agents: the mode is a desk setting, so
//! either machine's UI changes it, and the Mac (A, with a display double: its panel and the 34")
//! applies it. At exit the panel goes back to how the agent found it.

use std::sync::Arc;
use std::time::Duration;

use tandem_core::{BuiltinMode, DeskPatch, DisplayKey, Machine};
use tandem_input::displays::{BuiltinState, DisplayPower, FakeDisplayPower, PowerDisplay};
use tandem_proto::{Reply, UiCommand};
use tandem_sim::{Desk, Outcome};
use tandemd::displays::DisplayGuard;

const LIMIT: Duration = Duration::from_secs(600);
const PANEL: u32 = 1;
const EXTERNAL: u32 = 7;

fn key(product: u16) -> DisplayKey {
    DisplayKey {
        vendor: 0x4C2D,
        product,
        serial: 1,
    }
}

async fn panel_until(mac: &FakeDisplayPower, want: impl Fn(&FakeDisplayPower) -> bool) {
    tokio::time::timeout(LIMIT, async {
        while !want(mac) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the panel never reached the state: {:?}",
            mac.builtin_calls()
        )
    });
}

#[tokio::test(start_paused = true)]
async fn the_mode_chosen_on_either_machine_is_applied_by_the_mac() {
    let d = Desk::new().await.unwrap();
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    let mac = Arc::new(
        FakeDisplayPower::new([
            PowerDisplay {
                id: EXTERNAL,
                key: Some(key(0x7119)),
            },
            PowerDisplay {
                id: PANEL,
                key: Some(key(0xA061)),
            },
        ])
        .with_builtin(PANEL),
    );
    drop(
        d.agent(Machine::A)
            .start_displays(Arc::new(DisplayGuard::new(mac.clone(), None))),
    );

    // The default: external only.
    panel_until(&mac, |m| !m.is_on(PANEL)).await;
    assert!(mac.is_on(EXTERNAL));

    // The PC's UI picks "mirror" for the Mac.
    let reply = d.agent(Machine::B).handle(UiCommand::EditDesk {
        patch: DeskPatch::BuiltinDisplay {
            machine: Machine::A,
            mode: BuiltinMode::Mirror,
        },
    });
    assert!(matches!(reply, Reply::Desk(_)), "{reply:?}");
    panel_until(&mac, |m| m.mirror_of(PANEL) == Some(EXTERNAL)).await;
    assert!(mac.is_on(PANEL));

    // The external display is unplugged: the panel is on on its own, never no screen at all.
    mac.unplug(EXTERNAL, true);
    panel_until(&mac, |m| m.is_on(PANEL) && m.mirror_of(PANEL).is_none()).await;
    assert_eq!(mac.active().unwrap().len(), 1);
    mac.unplug(EXTERNAL, false);
    panel_until(&mac, |m| m.mirror_of(PANEL) == Some(EXTERNAL)).await;

    // At exit, back as the agent found it: on, on its own.
    d.agent(Machine::A).restore_displays();
    assert!(mac.is_on(PANEL));
    assert_eq!(mac.mirror_of(PANEL), None);
    assert_eq!(
        mac.builtin_calls().last(),
        Some(&(PANEL, BuiltinState::Own))
    );
}
