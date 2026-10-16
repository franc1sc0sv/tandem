//! Invariantes de la topología sobre el trait, con los dobles de los dos sistemas: con
//! cualquier secuencia de pasos (incluidos fallos del sistema), la máquina nunca se queda
//! sin pantalla y todo se puede devolver a como estaba.

use proptest::prelude::*;
use tandem_core::{BuiltinDisplay, MonitorConfig};

use super::fake::FakeTopology;
use super::*;
use crate::quartz::MacTopology;
use crate::quartz::fake::FakeQuartz;

/// Un doble con sus dos monitores de la configuración y una forma de hacer fallar al
/// sistema en el próximo cambio.
trait Double: DisplayTopology {
    fn monitors(&self) -> [MonitorConfig; 2];
    fn fail_next(&mut self);
    fn heal(&mut self);
}

impl Double for FakeTopology {
    fn monitors(&self) -> [MonitorConfig; 2] {
        [
            monitor_by_model("m34", "LC34G55T"),
            monitor_by_model("m32", "S32BG85"),
        ]
    }
    fn fail_next(&mut self) {
        self.fail_next = true;
    }
    fn heal(&mut self) {
        self.fail_next = false;
    }
}

impl Double for MacTopology<FakeQuartz> {
    fn monitors(&self) -> [MonitorConfig; 2] {
        [
            monitor_by_model("m34", "LC34G55T"),
            monitor_by_model("m32", "Odyssey G5"),
        ]
    }
    fn fail_next(&mut self) {
        self.api_mut().script.push_back(false);
    }
    fn heal(&mut self) {
        self.api_mut().script.clear();
    }
}

#[derive(Debug, Clone)]
enum Op {
    Detach(usize),
    Attach(usize),
    Builtin(BuiltinDisplay),
    Fail,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0usize..2).prop_map(Op::Detach),
        (0usize..2).prop_map(Op::Attach),
        prop_oneof![
            Just(BuiltinDisplay::Off),
            Just(BuiltinDisplay::Mirror),
            Just(BuiltinDisplay::Extend),
        ]
        .prop_map(Op::Builtin),
        Just(Op::Fail),
    ]
}

fn run<T: Double>(t: &mut T, ops: Vec<Op>) -> Result<(), TestCaseError> {
    let total = t.displays().map_err(fail)?.len();
    let monitors = t.monitors();
    for op in ops {
        // Los errores son parte del juego (última pantalla, sin integrada, fallos).
        let _ = match op {
            Op::Detach(i) => detach(t, &monitors[i]).map(drop),
            Op::Attach(i) => attach(t, &monitors[i]).map(drop),
            Op::Builtin(mode) => set_builtin(t, mode).map(drop),
            Op::Fail => {
                t.fail_next();
                Ok(())
            }
        };
        let now = t.displays().map_err(fail)?;
        prop_assert!(now.iter().any(|d| d.active), "sin pantalla: {now:?}");
        prop_assert_eq!(now.len(), total, "se perdió una pantalla: {:?}", now);
    }
    // Vuelta a la normalidad con las mismas operaciones de siempre.
    t.heal();
    for m in &monitors {
        attach(t, m).map_err(fail)?;
    }
    match set_builtin(t, BuiltinDisplay::Extend) {
        Ok(_) | Err(TopologyError::NoBuiltin) => {}
        Err(e) => return Err(fail(e)),
    }
    let after = t.displays().map_err(fail)?;
    prop_assert!(
        after.iter().all(|d| d.active && d.mirror_of.is_none()),
        "{after:?}"
    );
    Ok(())
}

fn fail(e: TopologyError) -> TestCaseError {
    TestCaseError::fail(e.to_string())
}

proptest! {
    #[test]
    fn windows_never_blind_and_always_restorable(ops in prop::collection::vec(op(), 0..40)) {
        run(&mut FakeTopology::two_monitors(), ops)?;
    }

    #[test]
    fn mac_never_blind_and_always_restorable(
        ops in prop::collection::vec(op(), 0..40),
        hide in any::<bool>(),
        private in any::<bool>(),
    ) {
        let mut fake = FakeQuartz::desk();
        fake.hide_disabled = hide;
        fake.private_api = private;
        run(&mut MacTopology::new(fake), ops)?;
    }
}
