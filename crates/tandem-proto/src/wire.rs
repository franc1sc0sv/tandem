//! Wire shapes of the `tandem-core` types that `postcard` cannot read.
//!
//! `Step` uses a serde tagged enum (meant for TOML/JSON), which needs a
//! self-describing format; `postcard` is not one. Here it is mirrored as a simple enum and used
//! with `#[serde(with = ...)]`, so the messages keep the core's types. The
//! conversions are exhaustive `match`es: if the core changes, this stops compiling instead of
//! going out of sync.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tandem_core::{Arrangement, Machine, MonitorInput, Step};

#[derive(Serialize, Deserialize)]
enum StepWire {
    PowerOn {
        machine: Machine,
    },
    PowerOff {
        machine: Machine,
    },
    SwitchMonitor {
        monitor: String,
        input: MonitorInput,
    },
    ShowMachine {
        monitor: String,
        machine: Machine,
    },
    Arrange {
        layout: Option<Arrangement>,
    },
    Geometry,
    DisplayOn {
        monitor: String,
        machine: Machine,
    },
}

impl From<&Step> for StepWire {
    fn from(s: &Step) -> Self {
        match s {
            Step::PowerOn { machine } => StepWire::PowerOn { machine: *machine },
            Step::PowerOff { machine } => StepWire::PowerOff { machine: *machine },
            Step::SwitchMonitor { monitor, input } => StepWire::SwitchMonitor {
                monitor: monitor.clone(),
                input: *input,
            },
            Step::ShowMachine { monitor, machine } => StepWire::ShowMachine {
                monitor: monitor.clone(),
                machine: *machine,
            },
            Step::Arrange { layout } => StepWire::Arrange { layout: *layout },
            Step::Geometry => StepWire::Geometry,
            Step::DisplayOn { monitor, machine } => StepWire::DisplayOn {
                monitor: monitor.clone(),
                machine: *machine,
            },
        }
    }
}

impl From<StepWire> for Step {
    fn from(s: StepWire) -> Self {
        match s {
            StepWire::PowerOn { machine } => Step::PowerOn { machine },
            StepWire::PowerOff { machine } => Step::PowerOff { machine },
            StepWire::SwitchMonitor { monitor, input } => Step::SwitchMonitor { monitor, input },
            StepWire::ShowMachine { monitor, machine } => Step::ShowMachine { monitor, machine },
            StepWire::Arrange { layout } => Step::Arrange { layout },
            StepWire::Geometry => Step::Geometry,
            StepWire::DisplayOn { monitor, machine } => Step::DisplayOn { monitor, machine },
        }
    }
}

pub(crate) mod step {
    use super::*;

    pub fn serialize<S: Serializer>(step: &Step, s: S) -> Result<S::Ok, S::Error> {
        StepWire::from(step).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Step, D::Error> {
        StepWire::deserialize(d).map(Step::from)
    }
}

pub(crate) mod steps {
    use super::*;

    pub fn serialize<S: Serializer>(steps: &[Step], s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(steps.iter().map(StepWire::from))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Step>, D::Error> {
        Vec::<StepWire>::deserialize(d).map(|v| v.into_iter().map(Step::from).collect())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use proptest::test_runner::{Config, RngSeed};
    use tandem_core::{
        Align, Arrangement, DeskStamp, DeskVersion, InputPrefs, Scale, Scene, ScreenSpec,
        Shortcuts, Side, State,
    };

    use super::*;
    use crate::msg::ControlMsg;
    use crate::{decode, encode};

    fn machine() -> impl Strategy<Value = Machine> {
        prop_oneof![Just(Machine::A), Just(Machine::B)]
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            machine().prop_map(|machine| Step::PowerOn { machine }),
            machine().prop_map(|machine| Step::PowerOff { machine }),
            (
                ".{0,24}",
                prop_oneof![Just(MonitorInput::DisplayPort), Just(MonitorInput::Hdmi)]
            )
                .prop_map(|(monitor, input)| Step::SwitchMonitor { monitor, input }),
            (".{0,24}", machine())
                .prop_map(|(monitor, machine)| Step::ShowMachine { monitor, machine }),
            arrangement().prop_map(|layout| Step::Arrange { layout }),
            Just(Step::Geometry),
            (".{0,24}", machine())
                .prop_map(|(monitor, machine)| Step::DisplayOn { monitor, machine }),
        ]
    }

    fn state() -> impl Strategy<Value = State> {
        (
            any::<bool>(),
            any::<bool>(),
            proptest::option::of("[a-z-]{1,8}"),
        )
            .prop_map(|(mac, pc, scene)| State {
                on: tandem_core::Powered { a: mac, b: pc },
                scene,
            })
    }

    fn screen() -> impl Strategy<Value = ScreenSpec> {
        (
            machine(),
            ".{0,12}",
            any::<i32>(),
            any::<i32>(),
            any::<u32>(),
            any::<u32>(),
            1u32..100_000,
        )
            .prop_map(|(machine, id, x, y, width, height, milli)| ScreenSpec {
                machine,
                id,
                x,
                y,
                width,
                height,
                scale: Scale::from_milli(milli),
            })
    }

    fn arrangement() -> impl Strategy<Value = Option<Arrangement>> {
        proptest::option::of(
            (
                proptest::sample::select(Side::ALL.to_vec()),
                proptest::sample::select(Align::ALL.to_vec()),
            )
                .prop_map(|(a_side, align)| Arrangement { a_side, align }),
        )
    }

    fn desk() -> impl Strategy<Value = DeskVersion> {
        (
            (any::<u64>(), any::<u64>(), machine()),
            proptest::collection::vec(screen(), 0..5),
            arrangement(),
            proptest::collection::vec("[A-Za-z]{1,10}", 0..3),
            proptest::collection::vec("[a-z]{1,6}", 0..3),
            (any::<bool>(), any::<bool>(), any::<bool>()),
            proptest::collection::btree_map("[a-z]{1,6}", any::<bool>(), 0..3),
            (1u8..=100, proptest::option::of(1u32..10_000)),
            proptest::collection::vec(("[a-z0-9-]{1,8}", any::<bool>()), 0..3),
        )
            .prop_map(
                |(
                    (rev, at, by),
                    screens,
                    arrangement,
                    lock,
                    scenes,
                    prefs,
                    modules,
                    data,
                    cables,
                )| {
                    DeskVersion {
                        stamp: DeskStamp { rev, at, by },
                        desk: tandem_core::Config {
                            name: Some("Office".into()),
                            machines: tandem_core::Machines {
                                a: Some(tandem_core::MachineInfo::new(
                                    Machine::A,
                                    tandem_core::Os::Windows,
                                    None,
                                )),
                                b: None,
                            },
                            screens,
                            arrangement,
                            scenes: scenes
                                .into_iter()
                                .map(|id| Scene::new(id.clone(), id))
                                .collect(),
                            shortcuts: Shortcuts { lock },
                            prefs: InputPrefs {
                                cmd_ctrl: prefs.0,
                                drag_files: prefs.1,
                            },
                            modules: tandem_core::Modules {
                                a: modules,
                                b: Default::default(),
                            },
                            data: tandem_core::DataPrefs {
                                wifi_cap_percent: data.0,
                                wifi_cap_mbps: data.1,
                            },
                            cables: cables
                                .into_iter()
                                .map(|(monitor, hdmi)| {
                                    let mut c = tandem_core::MonitorCable::new(monitor);
                                    c.a.input = Some(if hdmi {
                                        MonitorInput::Hdmi
                                    } else {
                                        MonitorInput::DisplayPort
                                    });
                                    c.b.screen = Some("screen-1".into());
                                    c
                                })
                                .collect(),
                            builtin_display: tandem_core::BuiltinDisplays {
                                a: None,
                                b: tandem_core::BuiltinMode::ALL
                                    .get(usize::from(data.0 % 4))
                                    .copied(),
                            },
                        },
                    }
                },
            )
    }

    proptest! {
        #![proptest_config(Config {
            rng_seed: RngSeed::Fixed(0x7A4D_0002),
            failure_persistence: None,
            ..Config::default()
        })]

        #[test]
        fn any_step_round_trips(id in any::<u64>(), step in step()) {
            let msg = ControlMsg::ExecuteStep { id, step };
            prop_assert_eq!(decode::<ControlMsg>(&encode(&msg).unwrap()).unwrap(), msg);
        }

        #[test]
        fn any_state_round_trips(state in state()) {
            let msg = ControlMsg::State(state);
            prop_assert_eq!(decode::<ControlMsg>(&encode(&msg).unwrap()).unwrap(), msg);
        }

        /// The arrangement travels exactly, scale included.
        #[test]
        fn any_desk_round_trips(sync in desk()) {
            let msg = ControlMsg::Desk(Box::new(sync));
            prop_assert_eq!(decode::<ControlMsg>(&encode(&msg).unwrap()).unwrap(), msg);
        }
    }
}
