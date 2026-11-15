//! Invariantes del núcleo con pruebas de propiedades.
//!
//! El ejecutor de prueba lleva un modelo "físico" (qué máquinas están prendidas) que solo
//! cambia cuando un paso se ejecuta con éxito. Así se comprueba lo que de verdad le importa
//! al usuario, no solo lo que `State` cree. Los estados se generan desde las dos
//! perspectivas (DESIGN §3.7).

use std::future::Future;

use proptest::prelude::*;
use tandem_core::{
    Abilities, Arrangement, Config, Executor, Machine, Plan, Powered, Scale, ScreenSpec, Side,
    State, Step, TxnError, run,
};

/// Ejecutor que falla exactamente en la llamada número `fail_at` (si la hay).
struct Sim {
    on: Powered,
    calls: usize,
    fail_at: Option<usize>,
}

impl Executor for Sim {
    fn execute(&mut self, step: &Step) -> impl Future<Output = Result<(), String>> + Send {
        let n = self.calls;
        self.calls += 1;
        let r = if Some(n) == self.fail_at {
            Err(format!("fallo inyectado en la llamada {n}"))
        } else {
            match step {
                // El Wake-on-LAN sale de la otra, que tiene que estar prendida.
                Step::PowerOn { machine } if !self.on.is_on(machine.other()) => {
                    Err(format!("{} está apagada", machine.other()))
                }
                Step::PowerOn { machine } => {
                    self.on.set(*machine, true);
                    Ok(())
                }
                Step::PowerOff { machine } => {
                    self.on.set(*machine, false);
                    Ok(())
                }
            }
        };
        async move { r }
    }
}

fn machine() -> impl Strategy<Value = Machine> {
    prop_oneof![Just(Machine::Mac), Just(Machine::Pc)]
}

/// Quien orquesta está prendida; la otra, como sea.
fn state() -> impl Strategy<Value = (Machine, State)> {
    (machine(), any::<bool>()).prop_map(|(me, other_on)| {
        let mut s = State::initial(me);
        s.on.set(me.other(), other_on);
        (me, s)
    })
}

fn screen() -> impl Strategy<Value = ScreenSpec> {
    (
        machine(),
        "[a-z0-9]{1,6}",
        any::<i32>(),
        any::<i32>(),
        1u32..10_000,
        1u32..10_000,
        1u32..4000,
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

proptest! {
    /// Encender o apagar la otra deja el estado con lo que de verdad se hizo, falle donde
    /// falle.
    #[test]
    fn the_state_follows_what_really_happened(
        (me, start) in state(),
        on in any::<bool>(),
        fail in proptest::option::of(0usize..2),
    ) {
        let other = me.other();
        let plan = if on {
            Plan::power_on(&start, other, &Abilities::both_wake()).unwrap()
        } else {
            Plan::power_off(&start, other)
        };
        let mut sim = Sim { on: start.on, calls: 0, fail_at: fail };
        let mut s = start.clone();
        let r = pollster::block_on(run(&mut s, &plan, &mut sim));
        prop_assert_eq!(s.on, sim.on);
        prop_assert!(s.is_on(me), "quien orquesta nunca se apaga sola");
        match r {
            Ok(_) => prop_assert_eq!(s.is_on(other), on),
            Err(TxnError::Failed { .. }) => prop_assert_eq!(s, start),
        }
    }

    /// Lo que se declara en `tandem.toml` vuelve igual.
    #[test]
    fn the_declared_desk_round_trips(
        screens in proptest::collection::vec(screen(), 0..5),
        right in any::<bool>(),
        offset in any::<i32>(),
    ) {
        let cfg = Config {
            screens,
            arrangement: Arrangement {
                mac_side: if right { Side::Right } else { Side::Left },
                offset,
            },
        };
        let text = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        prop_assert_eq!(back, cfg);
    }
}
