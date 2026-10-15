//! Formas de cable de los tipos de `tandem-core` que `postcard` no sabe leer.
//!
//! `Step` y `Mode` usan enums etiquetados de serde (pensados para TOML/JSON), que
//! necesitan un formato autodescriptivo; `postcard` no lo es. Aquí se espejan como
//! enums simples y se usan con `#[serde(with = ...)]`, así los mensajes guardan los
//! tipos del núcleo. Las conversiones son `match` exhaustivos: si el núcleo cambia,
//! esto deja de compilar en vez de desincronizarse.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tandem_core::{BuiltinDisplay, Input, Machine, Mode, MonitorId, Step};

#[derive(Serialize, Deserialize)]
enum StepWire {
    PowerOnPc,
    PowerOffPc,
    SwitchInput {
        monitor: MonitorId,
        from: Machine,
        to: Machine,
        input: Input,
    },
    Attach {
        machine: Machine,
        monitor: MonitorId,
    },
    Detach {
        machine: Machine,
        monitor: MonitorId,
    },
    SetBuiltin {
        from: BuiltinDisplay,
        to: BuiltinDisplay,
    },
    UpdateInputGeometry,
}

impl From<&Step> for StepWire {
    fn from(s: &Step) -> Self {
        match s.clone() {
            Step::PowerOnPc => StepWire::PowerOnPc,
            Step::PowerOffPc => StepWire::PowerOffPc,
            Step::SwitchInput {
                monitor,
                from,
                to,
                input,
            } => StepWire::SwitchInput {
                monitor,
                from,
                to,
                input,
            },
            Step::Attach { machine, monitor } => StepWire::Attach { machine, monitor },
            Step::Detach { machine, monitor } => StepWire::Detach { machine, monitor },
            Step::SetBuiltin { from, to } => StepWire::SetBuiltin { from, to },
            Step::UpdateInputGeometry => StepWire::UpdateInputGeometry,
        }
    }
}

impl From<StepWire> for Step {
    fn from(s: StepWire) -> Self {
        match s {
            StepWire::PowerOnPc => Step::PowerOnPc,
            StepWire::PowerOffPc => Step::PowerOffPc,
            StepWire::SwitchInput {
                monitor,
                from,
                to,
                input,
            } => Step::SwitchInput {
                monitor,
                from,
                to,
                input,
            },
            StepWire::Attach { machine, monitor } => Step::Attach { machine, monitor },
            StepWire::Detach { machine, monitor } => Step::Detach { machine, monitor },
            StepWire::SetBuiltin { from, to } => Step::SetBuiltin { from, to },
            StepWire::UpdateInputGeometry => Step::UpdateInputGeometry,
        }
    }
}

#[derive(Serialize, Deserialize)]
enum ModeWire {
    SoloMac,
    Shared(BTreeMap<MonitorId, Machine>),
    SoloPc,
}

impl From<&Mode> for ModeWire {
    fn from(m: &Mode) -> Self {
        match m {
            Mode::SoloMac => ModeWire::SoloMac,
            Mode::Shared(a) => ModeWire::Shared(a.clone()),
            Mode::SoloPc => ModeWire::SoloPc,
        }
    }
}

impl From<ModeWire> for Mode {
    fn from(m: ModeWire) -> Self {
        match m {
            ModeWire::SoloMac => Mode::SoloMac,
            ModeWire::Shared(a) => Mode::Shared(a),
            ModeWire::SoloPc => Mode::SoloPc,
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

pub(crate) mod mode {
    use super::*;

    pub fn serialize<S: Serializer>(mode: &Mode, s: S) -> Result<S::Ok, S::Error> {
        ModeWire::from(mode).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Mode, D::Error> {
        ModeWire::deserialize(d).map(Mode::from)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use proptest::test_runner::{Config, RngSeed};
    use tandem_core::State;

    use super::*;
    use crate::msg::{ControlMsg, UiCommand};
    use crate::{decode, encode};

    fn machine() -> impl Strategy<Value = Machine> {
        prop_oneof![Just(Machine::Mac), Just(Machine::Pc)]
    }

    fn builtin() -> impl Strategy<Value = BuiltinDisplay> {
        prop_oneof![
            Just(BuiltinDisplay::Off),
            Just(BuiltinDisplay::Mirror),
            Just(BuiltinDisplay::Extend),
        ]
    }

    fn monitor() -> impl Strategy<Value = MonitorId> {
        ".{0,20}".prop_map(MonitorId)
    }

    fn step() -> impl Strategy<Value = Step> {
        let input = prop_oneof![Just(Input::Hdmi), Just(Input::DisplayPort)];
        prop_oneof![
            Just(Step::PowerOnPc),
            Just(Step::PowerOffPc),
            (monitor(), machine(), machine(), input).prop_map(|(monitor, from, to, input)| {
                Step::SwitchInput {
                    monitor,
                    from,
                    to,
                    input,
                }
            }),
            (machine(), monitor()).prop_map(|(machine, monitor)| Step::Attach { machine, monitor }),
            (machine(), monitor()).prop_map(|(machine, monitor)| Step::Detach { machine, monitor }),
            (builtin(), builtin()).prop_map(|(from, to)| Step::SetBuiltin { from, to }),
            Just(Step::UpdateInputGeometry),
        ]
    }

    fn assignment() -> impl Strategy<Value = BTreeMap<MonitorId, Machine>> {
        proptest::collection::btree_map(monitor(), machine(), 0..6)
    }

    fn mode() -> impl Strategy<Value = Mode> {
        prop_oneof![
            Just(Mode::SoloMac),
            assignment().prop_map(Mode::Shared),
            Just(Mode::SoloPc),
        ]
    }

    fn state() -> impl Strategy<Value = State> {
        (assignment(), any::<bool>(), builtin()).prop_map(|(owner, pc_on, builtin_pref)| State {
            owner,
            pc_on,
            builtin_pref,
        })
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
        fn any_mode_round_trips(mode in mode()) {
            let cmd = UiCommand::SetMode(mode);
            prop_assert_eq!(decode::<UiCommand>(&encode(&cmd).unwrap()).unwrap(), cmd);
        }

        #[test]
        fn any_state_round_trips(state in state()) {
            let msg = ControlMsg::State(state);
            prop_assert_eq!(decode::<ControlMsg>(&encode(&msg).unwrap()).unwrap(), msg);
        }
    }
}
