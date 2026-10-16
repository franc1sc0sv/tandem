use std::collections::BTreeMap;

use proptest::prelude::*;
use tandem_core::{BuiltinDisplay, Input, Machine, MonitorConfig};

use super::fake::{Call, FakeQuartz, builtin, external};
use super::*;

fn monitor(id: &str, model: &str) -> MonitorConfig {
    MonitorConfig {
        id: id.into(),
        label: id.into(),
        edid_model: Some(model.into()),
        wiring: BTreeMap::from([
            (Machine::Mac, Input::Hdmi),
            (Machine::Pc, Input::DisplayPort),
        ]),
        ddc: None,
    }
}

fn m34() -> MonitorConfig {
    monitor("34", "LC34G55T")
}

fn m32() -> MonitorConfig {
    monitor("32", "Odyssey G5")
}

fn topo() -> MacTopology<FakeQuartz> {
    MacTopology::new(FakeQuartz::desk())
}

const B: DisplayId = DisplayId(1);
const D34: DisplayId = DisplayId(2);
const D32: DisplayId = DisplayId(3);

#[test]
fn finds_monitor_by_edid_model_ignoring_case() {
    let mut t = topo();
    assert_eq!(t.find(&monitor("x", " lc34g55t ")).unwrap().id, D34);
    assert_eq!(t.find(&m32()).unwrap().id, D32);
    assert_eq!(
        t.find(&monitor("x", "DELL")),
        Err(TopologyError::NotFound("x".into()))
    );
}

#[test]
fn monitor_without_edid_model_never_matches() {
    let mut cfg = m34();
    cfg.edid_model = None;
    assert!(matches!(topo().find(&cfg), Err(TopologyError::NotFound(_))));
}

#[test]
fn builtin_never_matches_a_monitor() {
    let mut t = MacTopology::new(FakeQuartz::new(vec![builtin()]));
    assert!(t.find(&monitor("x", "Color LCD")).is_err());
}

#[test]
fn two_identical_monitors_are_ambiguous() {
    let mut t = MacTopology::new(FakeQuartz::new(vec![
        builtin(),
        external(2, "LC34G55T"),
        external(3, "LC34G55T"),
    ]));
    assert_eq!(t.find(&m34()), Err(TopologyError::Ambiguous("34".into())));
}

#[test]
fn detach_and_attach_round_trip_even_if_macos_hides_disabled() {
    let mut t = topo();
    t.detach(&m34()).unwrap();
    assert!(!t.api().get(D34).unwrap().enabled);
    // macOS ya no la lista, pero la seguimos encontrando.
    assert!(t.api_mut().online().unwrap().iter().all(|d| d.id != D34));
    let found = t.find(&m34()).unwrap();
    assert!(!found.enabled);
    assert_eq!(t.disabled_ids(), vec![D34]);

    t.attach(&m34()).unwrap();
    assert!(t.api().get(D34).unwrap().enabled);
    assert!(t.disabled_ids().is_empty());
}

#[test]
fn detach_and_attach_are_idempotent() {
    let mut t = topo();
    t.detach(&m34()).unwrap();
    t.detach(&m34()).unwrap();
    t.attach(&m34()).unwrap();
    t.attach(&m34()).unwrap();
    assert_eq!(
        t.api().calls,
        vec![Call::SetEnabled(D34, false), Call::SetEnabled(D34, true)]
    );
}

#[test]
fn works_when_macos_keeps_listing_disabled_displays() {
    let mut fake = FakeQuartz::desk();
    fake.hide_disabled = false;
    let mut t = MacTopology::new(fake);
    t.detach(&m32()).unwrap();
    let all = t.displays().unwrap();
    assert_eq!(all.len(), 3);
    assert!(!all.iter().find(|d| d.id == D32).unwrap().enabled);
    t.attach(&m32()).unwrap();
    assert_eq!(t.api().enabled_count(), 3);
}

#[test]
fn never_disables_the_last_display() {
    let mut t = topo();
    t.set_builtin(BuiltinDisplay::Off).unwrap();
    t.detach(&m34()).unwrap();
    assert_eq!(t.detach(&m32()), Err(TopologyError::LastDisplay(D32)));
    assert!(t.api().get(D32).unwrap().enabled);
}

#[test]
fn builtin_off_needs_an_external_display() {
    let mut t = MacTopology::new(FakeQuartz::new(vec![builtin()]));
    assert_eq!(
        t.set_builtin(BuiltinDisplay::Off),
        Err(TopologyError::LastDisplay(B))
    );
    assert!(t.api().calls.is_empty());
}

#[test]
fn builtin_off_then_extend() {
    let mut t = topo();
    assert_eq!(
        t.set_builtin(BuiltinDisplay::Off),
        Ok(BuiltinApplied::Exact(BuiltinDisplay::Off))
    );
    assert!(!t.api().get(B).unwrap().enabled);
    // La principal pasa a un externo.
    assert!(t.api().get(D34).unwrap().main);
    assert_eq!(
        t.set_builtin(BuiltinDisplay::Extend),
        Ok(BuiltinApplied::Exact(BuiltinDisplay::Extend))
    );
    assert!(t.api().get(B).unwrap().enabled);
}

#[test]
fn mirror_prefers_the_main_external_display() {
    let mut fake = FakeQuartz::desk();
    for d in &mut fake.displays {
        d.main = d.id == D32;
    }
    let mut t = MacTopology::new(fake);
    t.set_builtin(BuiltinDisplay::Mirror).unwrap();
    assert_eq!(t.api().get(B).unwrap().mirror_of, Some(D32));
}

#[test]
fn mirror_uses_first_external_when_builtin_is_main() {
    let mut t = topo();
    t.set_builtin(BuiltinDisplay::Mirror).unwrap();
    assert_eq!(t.api().get(B).unwrap().mirror_of, Some(D34));
    // Repetirlo no vuelve a configurar nada.
    let calls = t.api().calls.len();
    t.set_builtin(BuiltinDisplay::Mirror).unwrap();
    assert_eq!(t.api().calls.len(), calls);
}

#[test]
fn mirror_skips_detached_monitors() {
    let mut t = topo();
    t.detach(&m34()).unwrap();
    t.set_builtin(BuiltinDisplay::Mirror).unwrap();
    assert_eq!(t.api().get(B).unwrap().mirror_of, Some(D32));
}

#[test]
fn mirror_without_externals_fails() {
    let mut t = MacTopology::new(FakeQuartz::new(vec![builtin()]));
    assert_eq!(
        t.set_builtin(BuiltinDisplay::Mirror),
        Err(TopologyError::NoMirrorTarget)
    );
}

#[test]
fn extend_undoes_mirror() {
    let mut t = topo();
    t.set_builtin(BuiltinDisplay::Mirror).unwrap();
    t.set_builtin(BuiltinDisplay::Extend).unwrap();
    assert_eq!(t.api().get(B).unwrap().mirror_of, None);
}

#[test]
fn off_from_mirror_unmirrors_first() {
    let mut t = topo();
    t.set_builtin(BuiltinDisplay::Mirror).unwrap();
    t.api_mut().calls.clear();
    t.set_builtin(BuiltinDisplay::Off).unwrap();
    assert_eq!(
        t.api().calls,
        vec![Call::SetMirror(B, None), Call::SetEnabled(B, false)]
    );
}

#[test]
fn mirror_from_off_enables_builtin_first() {
    let mut t = topo();
    t.set_builtin(BuiltinDisplay::Off).unwrap();
    t.api_mut().calls.clear();
    t.set_builtin(BuiltinDisplay::Mirror).unwrap();
    assert_eq!(
        t.api().calls,
        vec![Call::SetEnabled(B, true), Call::SetMirror(B, Some(D34))]
    );
    assert!(t.disabled_ids().is_empty());
}

#[test]
fn detaching_the_mirror_master_frees_the_builtin() {
    let mut t = topo();
    t.set_builtin(BuiltinDisplay::Mirror).unwrap();
    t.detach(&m34()).unwrap();
    let b = t.api().get(B).unwrap();
    assert!(b.enabled);
    assert_eq!(b.mirror_of, None);
}

#[test]
fn off_falls_back_to_mirror_without_private_api() {
    let mut fake = FakeQuartz::desk();
    fake.private_api = false;
    let mut t = MacTopology::new(fake);
    assert_eq!(
        t.set_builtin(BuiltinDisplay::Off),
        Ok(BuiltinApplied::MirrorFallback)
    );
    let b = t.api().get(B).unwrap();
    assert!(b.enabled);
    assert_eq!(b.mirror_of, Some(D34));
    assert!(t.disabled_ids().is_empty());
}

#[test]
fn detach_without_private_api_reports_it() {
    let mut fake = FakeQuartz::desk();
    fake.private_api = false;
    let mut t = MacTopology::new(fake);
    assert_eq!(t.detach(&m34()), Err(TopologyError::PrivateApiUnavailable));
}

#[test]
fn os_failure_does_not_mark_display_as_disabled() {
    let mut t = topo();
    t.api_mut().script.push_back(false);
    assert!(matches!(t.detach(&m34()), Err(TopologyError::Os { .. })));
    assert!(t.disabled_ids().is_empty());
    assert!(t.api().get(D34).unwrap().enabled);
}

#[test]
fn restore_all_tries_every_display_and_reports_first_error() {
    let mut t = topo();
    t.detach(&m34()).unwrap();
    t.set_builtin(BuiltinDisplay::Off).unwrap();
    t.api_mut().script.push_back(false);
    assert!(t.restore_all().is_err());
    // La que falló sigue pendiente; la otra volvió.
    assert_eq!(t.disabled_ids().len(), 1);
    t.restore_all().unwrap();
    assert_eq!(t.api().enabled_count(), 3);
}

#[test]
fn restore_guard_brings_displays_back_on_panic() {
    let mut t = topo();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut g = RestoreGuard(&mut t);
        g.detach(&m34()).unwrap();
        g.set_builtin(BuiltinDisplay::Off).unwrap();
        panic!("prueba interrumpida");
    }));
    assert!(result.is_err());
    assert_eq!(t.api().enabled_count(), 3);
    assert!(t.disabled_ids().is_empty());
}

#[test]
fn missing_builtin_is_reported() {
    let mut t = MacTopology::new(FakeQuartz::new(vec![external(2, "LC34G55T")]));
    assert_eq!(
        t.set_builtin(BuiltinDisplay::Extend),
        Err(TopologyError::NoBuiltin)
    );
}

#[test]
fn display_text_is_readable() {
    let mut d = external(2, "LC34G55T");
    d.mirror_of = Some(B);
    assert_eq!(
        d.to_string(),
        "#2 LC34G55T [espejo de #1] 3440x1440 en (3024, 0)"
    );
    let mut b = builtin();
    b.name = None;
    b.enabled = false;
    assert_eq!(
        b.to_string(),
        "#1 pantalla integrada (integrada) (principal) [desactivada] 1512x982 en (0, 0)"
    );
    let mut anon = external(3, "x");
    anon.name = None;
    assert_eq!(anon.label(), "4c2d:7003");
}

#[derive(Debug, Clone)]
enum Op {
    Detach(bool),
    Attach(bool),
    Builtin(BuiltinDisplay),
    Fail,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        any::<bool>().prop_map(Op::Detach),
        any::<bool>().prop_map(Op::Attach),
        prop_oneof![
            Just(BuiltinDisplay::Off),
            Just(BuiltinDisplay::Mirror),
            Just(BuiltinDisplay::Extend),
        ]
        .prop_map(Op::Builtin),
        Just(Op::Fail),
    ]
}

proptest! {
    /// Con cualquier secuencia (incluidos fallos del sistema), la Mac nunca se queda sin
    /// pantalla, nada refleja a una pantalla desactivada y `restore_all` lo devuelve todo.
    #[test]
    fn never_blind_and_always_restorable(
        ops in prop::collection::vec(op(), 0..40),
        hide in any::<bool>(),
        private in any::<bool>(),
    ) {
        let mut fake = FakeQuartz::desk();
        fake.hide_disabled = hide;
        fake.private_api = private;
        let mut t = MacTopology::new(fake);
        for op in ops {
            let _ = match op {
                Op::Detach(big) => t.detach(&if big { m34() } else { m32() }),
                Op::Attach(big) => t.attach(&if big { m34() } else { m32() }),
                Op::Builtin(mode) => t.set_builtin(mode).map(|_| ()),
                Op::Fail => {
                    t.api_mut().script.push_back(false);
                    Ok(())
                }
            };
            let api = t.api();
            prop_assert!(api.enabled_count() >= 1);
            for d in &api.displays {
                if let Some(m) = d.mirror_of {
                    prop_assert!(api.get(m).is_some_and(|m| m.enabled));
                }
            }
            for id in t.disabled_ids() {
                prop_assert!(!api.get(id).is_some_and(|d| d.enabled));
            }
        }
        t.api_mut().script.clear();
        t.restore_all().unwrap();
        prop_assert_eq!(t.api().enabled_count(), 3);
    }
}
