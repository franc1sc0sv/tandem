# Testing strategy

Goal: when the real hardware is connected, the only thing left to verify is what **physically** cannot be simulated.

## Layers

| Layer | What | Where it runs |
|---|---|---|
| Unit | Pure logic: plans, transactions, geometry, key map, protocol, link policy | Linux / CI x3 |
| Properties (`proptest`) | Invariants: framing decodes any partition; rollback restores the state; HID↔Mac↔Win key round trip | Linux / CI x3 |
| Integration | Two `tandemd` in the same process over loopback (real QUIC) with simulated screen, input, power and peripherals backends | Linux / CI x3 |
| Binary (`tandemd/tests/two_processes.rs`) | Two `tandemd` processes compiled with `--sim-backends`, paired through files and joined by QUIC over loopback; the test is the UI over the real IPC. Power on, state on both and killing a process halfway through a change | CI Linux |
| Desk simulation (`tandem-sim`) | End-to-end scenarios: power, link drops, hub change, power off, faults injected at each step | Linux / CI x3 |
| Platform | Real Windows and macOS code: it compiles and its device-free tests run on the runners | CI windows / macos |
| Hardware | `#[ignore = "hardware"]` and the `## Pending hardware verification` list of each task | Real machine, with the user |

## Rules

- Every platform backend has a test double that implements the same trait.
- Faults are injected with a `FaultPlan`: fail step N, timeout, link drop at instant T.
- Clocks are controlled with `tokio::time::pause()`; no test uses a real `sleep`.
- A bug found = first a failing test, then the fix.
- Coverage with `cargo llvm-cov` in CI (informational; target ≥ 80 % in the portable crates).

## Two real processes (`--sim-backends`)

`crates/tandemd/tests/two_processes.rs` launches the binary twice, with temporary configs and data directories, pre-provisioned identities and `trust.toml`, and a fixed loopback link. `tandemd`'s `sim-backends` feature (enabled by its own `[dev-dependencies]`, so it only exists when compiling the tests) adds the `--sim-backends` option: steps that always succeed, with no mDNS or input engine. Since each process is opaque, the steps leave a trace in the data directory (`sim-steps.jsonl`, one JSON line per step) and are held back while `sim-hold` exists: this way the test kills a process with a step half done without depending on timing. It uses real clocks with short times in the config (`step_timeout_secs`, `pc_lost_after_secs`). In the WSL sandbox, which does not allow creating Unix sockets, it is skipped; in CI never.

## Desk simulation (`tandem-sim`)

`crates/tandem-sim` brings up two real `tandemd` in the same process: each with its orchestrator, its link manager (`tandem-link`) and the real control protocol (`PeerClient` / `peer::serve`). The only simulated part is the outside:

- **Physical desk** (`world`): the PC's power (Wake-on-LAN, which only arrives over Ethernet; 45 s boot; power off), each input engine's geometry and the USB hub.
- **Network** (`net`): an in-memory link per USB4, Ethernet and Wi-Fi. Cutting a link is silence, like an unplugged cable; streams opened over it do not come back even if the cable does. With the PC off its agent does not answer on any link.
- **Faults** (`FaultPlan`): at any step of either machine, `Fail` (fails without being applied), `Hang` (never finishes: the timeout cuts it), `Delay`, `CutLink` (a link drops when the step starts) and `ApplyThenCut` (it is applied and the response is lost with the link). Also `cut_link_at` to cut a link at an instant. Each fault fires once unless `.times(n)`, so the step's inverse does not fail as well.

The checks compare physical truth with what the agent believes: `Desk::check` (power and no impossible step, such as one on the powered-off PC) and `Desk::pc_agrees` (the PC's agent has the same state).

### Adding a scenario

Scenarios go in `crates/tandem-sim/tests/scenarios.rs` (or in another file of `tests/`), always with the clock paused:

```rust
#[tokio::test(start_paused = true)]
async fn my_scenario() {
    // One scene monitor; scenes are added as in `tests/scenes.rs`.
    let d = Desk::builder()
        .scene_monitors(&[("34", Machine::A)])
        .build()
        .await
        .unwrap();

    // Reach the starting point the normal way: power off machine B.
    let run = d.shutdown_pc().await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed));

    // Arm the faults right before what is being tested.
    d.arm(FaultPlan::new().on(
        Machine::A,
        Step::Geometry,
        FaultAction::Fail("the engine did not respond".into()),
    ));
    let run = d.apply_scene(Machine::A, "work").await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Failed));

    // The desk matches the state and the PC agrees.
    d.check().await.unwrap();
    d.pc_agrees().await.unwrap();
}
```

- No `std::thread::sleep` and no real waits: to let time pass, `tokio::time::sleep` (with the clock paused it advances by itself) or `Desk::settle`.
- For a new `tandem_core::Step`, add its effect in `SimSteps::apply` (`steps.rs`) and whatever is needed in `World` and in `World::check`.
- For a new backend (for example a real one behind a trait), the double goes in the backend's crate; `tandem-sim` only models the physical part.
