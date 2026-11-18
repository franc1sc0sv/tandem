//! End-to-end scenarios of the desk's power (docs/TESTING.md, "Desk
//! simulation"): power on and off from either machine, faults and maximum times in
//! each step, power cuts and link drops.
//!
//! All with the tokio clock paused: the PC's startup (45 s), the links' pings and
//! the steps' maximum times run in virtual time.

use std::time::Duration;

use proptest::prelude::*;
use tandem_core::{Machine, Os, Step};
use tandem_link::manager::LinkKind;
use tandem_proto::PowerEvent;
use tandem_sim::{Desk, FaultAction, FaultPlan, Outcome, Power, TxnRun};

const PC_ON: Step = Step::PowerOn {
    machine: Machine::B,
};
const PC_OFF: Step = Step::PowerOff {
    machine: Machine::B,
};

async fn committed(d: &Desk, run: &TxnRun) {
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    d.check().await.unwrap();
}

/// The Mac with the PC on and both agents seeing each other.
async fn both_on() -> Desk {
    let d = Desk::new().await.unwrap();
    committed(&d, &d.power_on(Machine::B).await.unwrap()).await;
    d.pc_agrees().await.unwrap();
    d
}

#[tokio::test(start_paused = true)]
async fn power_the_pc_on_and_off() {
    let d = Desk::new().await.unwrap();
    d.check().await.unwrap();
    assert_eq!(d.world.power_of(Machine::B), Power::Off);

    // Wake-on-LAN and wait for the PC's agent.
    let start = tokio::time::Instant::now();
    let run = d.power_on(Machine::B).await.unwrap();
    committed(&d, &run).await;
    assert!(
        start.elapsed() >= Duration::from_secs(45),
        "waited for the startup"
    );
    assert_eq!(d.active_link(Machine::A), Some(LinkKind::Usb4));
    d.pc_agrees().await.unwrap();

    // Shutdown: the step runs on the PC, which answers and shuts down.
    let run = d.shutdown_pc().await.unwrap();
    committed(&d, &run).await;
    assert_eq!(d.world.power_of(Machine::B), Power::Off);
    assert_eq!(
        d.world.calls().last(),
        Some(&(Machine::B, PC_OFF)),
        "{:?}",
        d.world.calls()
    );
    // Without the PC no link with its agent remains.
    assert_eq!(d.active_link(Machine::A), None);
    assert!(d.world.violations().is_empty());
}

/// The mirror (DESIGN §3.7): with the PC as main, it wakes the Mac.
#[tokio::test(start_paused = true)]
async fn the_pc_also_orchestrates() {
    let d = Desk::builder().primary(Machine::B).build().await.unwrap();
    let run = d.power_on(Machine::A).await.unwrap();
    committed(&d, &run).await;
    assert_eq!(d.world.power_of(Machine::A), Power::On);
    d.agrees(Machine::A).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_failing_power_on_leaves_everything_as_it_was() {
    let d = Desk::new().await.unwrap();
    d.arm(FaultPlan::new().on(Machine::A, PC_ON, FaultAction::Fail("no network".into())));
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    assert_eq!(run.failure().map(|f| f.0), Some(0));
    assert!(!d.mac.state().on.b);
    d.check().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_hung_power_on_times_out_and_does_not_change_the_state() {
    let d = Desk::new().await.unwrap();
    d.arm(FaultPlan::new().on(Machine::A, PC_ON, FaultAction::Hang));
    let start = tokio::time::Instant::now();
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    assert!(start.elapsed() >= Duration::from_secs(120));
    assert!(!d.mac.state().on.b);
    d.check().await.unwrap();
}

/// The magic packet only goes out over Ethernet (DESIGN §6): without that cable the PC does not wake.
#[tokio::test(start_paused = true)]
async fn without_ethernet_the_pc_does_not_wake() {
    let d = Desk::builder()
        .links(&[LinkKind::Usb4, LinkKind::Wifi])
        .build()
        .await
        .unwrap();
    let run = d.power_on(Machine::B).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    assert_eq!(d.world.power_of(Machine::B), Power::Off);
    d.check().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_failing_shutdown_leaves_the_pc_on() {
    let d = both_on().await;
    d.arm(FaultPlan::new().on(
        Machine::B,
        PC_OFF,
        FaultAction::Fail("access denied".into()),
    ));
    let run = d.shutdown_pc().await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed), "{:#?}", run.events);
    assert!(d.mac.state().on.b);
    d.check().await.unwrap();
}

/// The PC shuts down but the answer is lost with the link: the Mac takes it as done and the
/// desk matches.
#[tokio::test(start_paused = true)]
async fn a_shutdown_with_a_lost_reply_is_taken_as_done() {
    let d = both_on().await;
    d.arm(FaultPlan::new().on(
        Machine::B,
        PC_OFF,
        FaultAction::ApplyThenCut(LinkKind::Usb4),
    ));
    let run = d.shutdown_pc().await.unwrap();
    committed(&d, &run).await;
    assert_eq!(d.world.power_of(Machine::B), Power::Off);
}

/// A shutdown that does not answer is taken as done (most likely the PC is
/// shutting down). If it actually stayed on, the next power-on repairs it: the
/// Wake-on-LAN does nothing to it and its agent already answers.
#[tokio::test(start_paused = true)]
async fn a_hung_shutdown_is_taken_as_done_and_the_next_power_on_repairs_it() {
    let d = both_on().await;
    d.arm(FaultPlan::new().on(Machine::B, PC_OFF, FaultAction::Hang));
    let run = d.shutdown_pc().await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert!(!d.mac.state().on.b);
    assert_eq!(d.world.power_of(Machine::B), Power::On);
    let run = d.power_on(Machine::B).await.unwrap();
    committed(&d, &run).await;
}

/// Power cut: after the threshold the Mac considers the PC off and notifies the UI; once
/// powered on by hand, it counts again.
#[tokio::test(start_paused = true)]
async fn power_cut_and_manual_return() {
    let d = both_on().await;
    let mut power = d.mac.subscribe_power();
    let lost_after = d.mac.config().power.peer_lost_after();
    d.pull_pc_plug();
    tokio::time::sleep(lost_after - Duration::from_secs(1)).await;
    assert!(d.mac.state().on.b, "inside the threshold nothing changes");
    let PowerEvent::PeerLost { machine, reason } =
        tokio::time::timeout(Duration::from_secs(10), power.recv())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(machine, Machine::B);
    assert!(reason.contains("corte de corriente"), "{reason}");
    assert!(!d.mac.state().on.b);
    d.check().await.unwrap();

    d.press_pc_power_button();
    let mut rx = d.mac.subscribe();
    tokio::time::timeout(Duration::from_secs(120), rx.wait_for(|s| s.on.b))
        .await
        .unwrap()
        .unwrap();
    d.check().await.unwrap();
}

/// Without USB4 or Ethernet the control plane continues over Wi-Fi, and when USB4 returns the manager
/// goes back to it.
#[tokio::test(start_paused = true)]
async fn with_only_wifi_control_goes_on() {
    let d = both_on().await;
    d.arm(
        FaultPlan::new()
            .cut_link_at(Duration::ZERO, LinkKind::Usb4)
            .cut_link_at(Duration::from_secs(1), LinkKind::Ethernet),
    );
    d.wait_link(LinkKind::Wifi).await.unwrap();
    d.net.restore(LinkKind::Usb4);
    d.wait_link(LinkKind::Usb4).await.unwrap();
    let run = d.shutdown_pc().await.unwrap();
    committed(&d, &run).await;
}

/// What the desk is asked on each round of the property.
#[derive(Debug, Clone)]
struct Op {
    from: Machine,
    on: bool,
    fail: bool,
}

fn op() -> impl Strategy<Value = Op> {
    (
        prop_oneof![Just(Machine::A), Just(Machine::B)],
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(from, on, fail)| Op { from, on, fail })
}

async fn run_ops(ops: Vec<Op>) -> Result<(), TestCaseError> {
    let d = Desk::new()
        .await
        .map_err(|e| TestCaseError::fail(e.to_string()))?;
    for (i, op) in ops.iter().enumerate() {
        let case = format!("step {i}: {op:?}");
        // Only a machine that is on can ask for something.
        if d.world.power_of(op.from) != Power::On {
            continue;
        }
        let other = op.from.other();
        if op.fail {
            let (machine, step) = if op.on {
                (op.from, Step::PowerOn { machine: other })
            } else {
                (other, Step::PowerOff { machine: other })
            };
            d.arm(FaultPlan::new().on(machine, step, FaultAction::Fail("fails".into())));
        } else {
            d.arm(FaultPlan::new());
        }
        let run = if op.on {
            d.set_power(op.from, other, true).await
        } else {
            d.shutdown(op.from, other).await
        };
        let run = run.map_err(|e| TestCaseError::fail(format!("{case}: {e}")))?;
        prop_assert!(run.outcome().is_some(), "{case}: {:#?}", run.events);
        d.check_from(op.from)
            .await
            .map_err(|e| TestCaseError::fail(format!("{case}: {e}")))?;
        prop_assert!(d.world.violations().is_empty(), "{case}");
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    /// Any sequence of power-ons and power-offs, requested from either machine and
    /// with injected faults, leaves power as the one who requested it believes.
    #[test]
    fn any_sequence_leaves_power_coherent(ops in proptest::collection::vec(op(), 1..6)) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap();
        rt.block_on(run_ops(ops))?;
    }
}

/// DECISIONS D2: the desk works the same with any system on each side. With macOS +
/// Windows, Windows + Windows and macOS + macOS: power B on from A and off again, each
/// announces its system and it is recorded in both desks.
#[tokio::test(start_paused = true)]
async fn any_system_on_each_side() {
    for (a, b) in [
        (Os::MacOs, Os::Windows),
        (Os::Windows, Os::Windows),
        (Os::MacOs, Os::MacOs),
    ] {
        let d = Desk::builder().os(a, b).build().await.unwrap();
        let run = d.power_on(Machine::B).await.unwrap();
        committed(&d, &run).await;
        d.pc_agrees().await.unwrap();
        // Each sees the other's system, and both desks store it.
        let caps = d.agent(Machine::A).capabilities();
        assert_eq!(caps.here.info.os, a, "{a:?}+{b:?}");
        assert_eq!(caps.peer.map(|p| p.info.os), Some(b), "{a:?}+{b:?}");
        for m in Machine::ALL {
            let desk = d.agent(m).desk();
            assert_eq!(desk.machines.get(Machine::A).map(|i| i.os), Some(a));
            assert_eq!(desk.machines.get(Machine::B).map(|i| i.os), Some(b));
        }
        // Only Windows knows how to shut itself down: it announces it according to its system, not its side.
        let b_caps = d.agent(Machine::B).capabilities().here;
        assert_eq!(b_caps.power.power_off, b == Os::Windows, "{a:?}+{b:?}");
        let run = d.shutdown_pc().await.unwrap();
        committed(&d, &run).await;
        assert_eq!(d.world.power_of(Machine::B), Power::Off, "{a:?}+{b:?}");
        assert!(d.world.violations().is_empty());
    }
}
