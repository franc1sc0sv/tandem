//! Formas de cable de los tipos de `tandem-core` que `postcard` no sabe leer.
//!
//! `Step` usa un enum etiquetado de serde (pensado para TOML/JSON), que necesita un
//! formato autodescriptivo; `postcard` no lo es. Aquí se espeja como enum simple y se usa
//! con `#[serde(with = ...)]`, así los mensajes guardan los tipos del núcleo. Las
//! conversiones son `match` exhaustivos: si el núcleo cambia, esto deja de compilar en vez
//! de desincronizarse.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tandem_core::{Machine, Step};

#[derive(Serialize, Deserialize)]
enum StepWire {
    PowerOn { machine: Machine },
    PowerOff { machine: Machine },
}

impl From<&Step> for StepWire {
    fn from(s: &Step) -> Self {
        match *s {
            Step::PowerOn { machine } => StepWire::PowerOn { machine },
            Step::PowerOff { machine } => StepWire::PowerOff { machine },
        }
    }
}

impl From<StepWire> for Step {
    fn from(s: StepWire) -> Self {
        match s {
            StepWire::PowerOn { machine } => Step::PowerOn { machine },
            StepWire::PowerOff { machine } => Step::PowerOff { machine },
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
    use tandem_core::{Arrangement, Scale, ScreenSpec, Side, State};

    use super::*;
    use crate::msg::{ControlMsg, DeskSync};
    use crate::{decode, encode};

    fn machine() -> impl Strategy<Value = Machine> {
        prop_oneof![Just(Machine::A), Just(Machine::B)]
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            machine().prop_map(|machine| Step::PowerOn { machine }),
            machine().prop_map(|machine| Step::PowerOff { machine }),
        ]
    }

    fn state() -> impl Strategy<Value = State> {
        (any::<bool>(), any::<bool>()).prop_map(|(mac, pc)| State {
            on: tandem_core::Powered { a: mac, b: pc },
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

    fn desk() -> impl Strategy<Value = DeskSync> {
        (
            any::<u64>(),
            machine(),
            proptest::collection::vec(screen(), 0..5),
            any::<bool>(),
            any::<i32>(),
            proptest::collection::vec("[A-Za-z]{1,10}", 0..3),
        )
            .prop_map(
                |(rev, origin, screens, right, offset, lock_shortcut)| DeskSync {
                    rev,
                    origin,
                    desk: tandem_core::Config {
                        machines: tandem_core::Machines {
                            a: Some(tandem_core::MachineInfo::new(
                                Machine::A,
                                tandem_core::Os::Windows,
                                None,
                            )),
                            b: None,
                        },
                        screens,
                        arrangement: Arrangement {
                            a_side: if right { Side::Right } else { Side::Left },
                            offset,
                        },
                    },
                    lock_shortcut,
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

        /// El acomodo viaja exacto, escala incluida.
        #[test]
        fn any_desk_round_trips(sync in desk()) {
            let msg = ControlMsg::Desk(sync);
            prop_assert_eq!(decode::<ControlMsg>(&encode(&msg).unwrap()).unwrap(), msg);
        }
    }
}
