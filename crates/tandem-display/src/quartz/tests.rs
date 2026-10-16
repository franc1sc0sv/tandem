use proptest::prelude::*;
use tandem_core::{BuiltinDisplay, MonitorConfig};

use super::fake::{Call, FakeQuartz, builtin, external};
use super::*;
use crate::topology::{self, BuiltinApplied, monitor_by_model};

fn monitor(id: &str, model: &str) -> MonitorConfig {
    monitor_by_model(id, model)
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

type T = MacTopology<FakeQuartz>;

fn find(t: &mut T, m: &MonitorConfig) -> Result<Display<DisplayId>, TopologyError> {
    let all = t.displays()?;
    topology::find_monitor(&all, m).cloned()
}

fn detach(t: &mut T, m: &MonitorConfig) -> Result<(), TopologyError> {
    topology::detach(t, m).map(drop)
}

fn attach(t: &mut T, m: &MonitorConfig) -> Result<(), TopologyError> {
    topology::attach(t, m).map(drop)
}

fn set_builtin(t: &mut T, mode: BuiltinDisplay) -> Result<BuiltinApplied, TopologyError> {
    topology::set_builtin(t, mode)
}

const B: DisplayId = DisplayId(1);
const D34: DisplayId = DisplayId(2);
const D32: DisplayId = DisplayId(3);

#[test]
fn finds_monitor_by_edid_model_ignoring_case() {
    let mut t = topo();
    assert_eq!(find(&mut t, &monitor("x", " lc34g55t ")).unwrap().id, D34);
    assert_eq!(find(&mut t, &m32()).unwrap().id, D32);
    assert_eq!(
        find(&mut t, &monitor("x", "DELL")),
        Err(TopologyError::NoMatch("DELL".into()))
    );
}

#[test]
fn finds_monitor_by_pnp_id() {
    let mut t = topo();
    // `external(2, ..)`: fabricante 0x4C2D (SAM), producto 0x7002.
    assert_eq!(find(&mut t, &monitor("x", "sam7002")).unwrap().id, D34);
    // Un trozo del id no basta.
    assert!(find(&mut t, &monitor("x", "SAM70")).is_err());
}

#[test]
fn monitor_without_edid_model_never_matches() {
    let mut cfg = m34();
    cfg.edid_model = None;
    assert_eq!(find(&mut topo(), &cfg), Err(TopologyError::MissingModel));
}

#[test]
fn builtin_never_matches_a_monitor() {
    let mut t = MacTopology::new(FakeQuartz::new(vec![builtin()]));
    assert!(find(&mut t, &monitor("x", "Color LCD")).is_err());
}

#[test]
fn two_identical_monitors_are_ambiguous() {
    let mut t = MacTopology::new(FakeQuartz::new(vec![
        builtin(),
        external(2, "LC34G55T"),
        external(3, "LC34G55T"),
    ]));
    assert!(matches!(
        find(&mut t, &m34()),
        Err(TopologyError::Ambiguous { candidates, .. }) if candidates.len() == 2
    ));
}

#[test]
fn detach_and_attach_round_trip_even_if_macos_hides_disabled() {
    let mut t = topo();
    detach(&mut t, &m34()).unwrap();
    assert!(!t.api().get(D34).unwrap().enabled);
    // macOS ya no la lista, pero la seguimos encontrando.
    assert!(t.api_mut().online().unwrap().iter().all(|d| d.id != D34));
    let found = find(&mut t, &m34()).unwrap();
    assert!(!found.active);
    assert_eq!(t.disabled_ids(), vec![D34]);

    attach(&mut t, &m34()).unwrap();
    assert!(t.api().get(D34).unwrap().enabled);
    assert!(t.disabled_ids().is_empty());
}

#[test]
fn detach_and_attach_are_idempotent() {
    let mut t = topo();
    detach(&mut t, &m34()).unwrap();
    detach(&mut t, &m34()).unwrap();
    attach(&mut t, &m34()).unwrap();
    attach(&mut t, &m34()).unwrap();
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
    detach(&mut t, &m32()).unwrap();
    let all = t.displays().unwrap();
    assert_eq!(all.len(), 3);
    assert!(!all.iter().find(|d| d.id == D32).unwrap().active);
    attach(&mut t, &m32()).unwrap();
    assert_eq!(t.api().enabled_count(), 3);
}

#[test]
fn never_disables_the_last_display() {
    let mut t = topo();
    set_builtin(&mut t, BuiltinDisplay::Off).unwrap();
    detach(&mut t, &m34()).unwrap();
    assert_eq!(
        detach(&mut t, &m32()),
        Err(TopologyError::LastDisplay(D32.to_string()))
    );
    assert!(t.api().get(D32).unwrap().enabled);
}

#[test]
fn builtin_off_needs_an_external_display() {
    let mut t = MacTopology::new(FakeQuartz::new(vec![builtin()]));
    assert_eq!(
        set_builtin(&mut t, BuiltinDisplay::Off),
        Err(TopologyError::LastDisplay(B.to_string()))
    );
    assert!(t.api().calls.is_empty());
}

#[test]
fn builtin_off_then_extend() {
    let mut t = topo();
    assert_eq!(
        set_builtin(&mut t, BuiltinDisplay::Off),
        Ok(BuiltinApplied::Exact(BuiltinDisplay::Off))
    );
    assert!(!t.api().get(B).unwrap().enabled);
    // La principal pasa a un externo.
    assert!(t.api().get(D34).unwrap().main);
    assert_eq!(
        set_builtin(&mut t, BuiltinDisplay::Extend),
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
    set_builtin(&mut t, BuiltinDisplay::Mirror).unwrap();
    assert_eq!(t.api().get(B).unwrap().mirror_of, Some(D32));
}

#[test]
fn mirror_uses_first_external_when_builtin_is_main() {
    let mut t = topo();
    set_builtin(&mut t, BuiltinDisplay::Mirror).unwrap();
    assert_eq!(t.api().get(B).unwrap().mirror_of, Some(D34));
    // Repetirlo no vuelve a configurar nada.
    let calls = t.api().calls.len();
    set_builtin(&mut t, BuiltinDisplay::Mirror).unwrap();
    assert_eq!(t.api().calls.len(), calls);
}

#[test]
fn mirror_skips_detached_monitors() {
    let mut t = topo();
    detach(&mut t, &m34()).unwrap();
    set_builtin(&mut t, BuiltinDisplay::Mirror).unwrap();
    assert_eq!(t.api().get(B).unwrap().mirror_of, Some(D32));
}

#[test]
fn mirror_without_externals_fails() {
    let mut t = MacTopology::new(FakeQuartz::new(vec![builtin()]));
    assert_eq!(
        set_builtin(&mut t, BuiltinDisplay::Mirror),
        Err(TopologyError::NoMirrorTarget)
    );
}

#[test]
fn extend_undoes_mirror() {
    let mut t = topo();
    set_builtin(&mut t, BuiltinDisplay::Mirror).unwrap();
    set_builtin(&mut t, BuiltinDisplay::Extend).unwrap();
    assert_eq!(t.api().get(B).unwrap().mirror_of, None);
}

#[test]
fn off_from_mirror_unmirrors_first() {
    let mut t = topo();
    set_builtin(&mut t, BuiltinDisplay::Mirror).unwrap();
    t.api_mut().calls.clear();
    set_builtin(&mut t, BuiltinDisplay::Off).unwrap();
    assert_eq!(
        t.api().calls,
        vec![Call::SetMirror(B, None), Call::SetEnabled(B, false)]
    );
}

#[test]
fn mirror_from_off_enables_builtin_first() {
    let mut t = topo();
    set_builtin(&mut t, BuiltinDisplay::Off).unwrap();
    t.api_mut().calls.clear();
    set_builtin(&mut t, BuiltinDisplay::Mirror).unwrap();
    assert_eq!(
        t.api().calls,
        vec![Call::SetEnabled(B, true), Call::SetMirror(B, Some(D34))]
    );
    assert!(t.disabled_ids().is_empty());
}

#[test]
fn detaching_the_mirror_master_frees_the_builtin() {
    let mut t = topo();
    set_builtin(&mut t, BuiltinDisplay::Mirror).unwrap();
    detach(&mut t, &m34()).unwrap();
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
        set_builtin(&mut t, BuiltinDisplay::Off),
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
    assert_eq!(
        detach(&mut t, &m34()),
        Err(TopologyError::PrivateApiUnavailable)
    );
}

#[test]
fn os_failure_does_not_mark_display_as_disabled() {
    let mut t = topo();
    t.api_mut().script.push_back(false);
    assert!(matches!(
        detach(&mut t, &m34()),
        Err(TopologyError::Os { .. })
    ));
    assert!(t.disabled_ids().is_empty());
    assert!(t.api().get(D34).unwrap().enabled);
}

#[test]
fn restore_all_tries_every_display_and_reports_first_error() {
    let mut t = topo();
    detach(&mut t, &m34()).unwrap();
    set_builtin(&mut t, BuiltinDisplay::Off).unwrap();
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
        detach(&mut g, &m34()).unwrap();
        set_builtin(&mut g, BuiltinDisplay::Off).unwrap();
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
        set_builtin(&mut t, BuiltinDisplay::Extend),
        Err(TopologyError::NoBuiltin)
    );
}

#[test]
fn coregraphics_fields_become_the_common_display() {
    let mut d = external(2, "LC34G55T");
    d.mirror_of = Some(B);
    let common = Display::from(d);
    let edid = common.edid.as_ref().unwrap();
    assert_eq!(edid.pnp_id(), "SAM7002");
    assert_eq!(edid.serial_number, 2);
    assert_eq!(common.model(), "LC34G55T");
    assert_eq!(
        common.to_string(),
        "LC34G55T (SAM7002) en #2 [espejo de #1] [activo] 3440x1440 en (3024, 0)"
    );

    let mut b = builtin();
    b.name = None;
    b.enabled = false;
    let common = Display::from(b);
    assert!(common.builtin && !common.active && common.bounds.is_none());
    assert_eq!(
        common.to_string(),
        "pantalla integrada (APPA050) en #1 (integrada) (principal) [inactivo]"
    );

    let mut anon = external(3, "x");
    anon.name = None;
    anon.vendor = 0;
    let common = Display::from(anon);
    assert_eq!(common.model(), "0000:7003");
    assert!(common.edid.is_none());
}

#[test]
fn mac_displays_are_ordered_and_include_forgotten_ones() {
    let mut t = topo();
    detach(&mut t, &m34()).unwrap();
    let ids: Vec<_> = t.displays().unwrap().iter().map(|d| d.id).collect();
    assert_eq!(ids, [B, D34, D32]);
    // Lo que macOS ya no lista sale sin rectángulo ni espejo.
    let gone = find(&mut t, &m34()).unwrap();
    assert!(!gone.active && !gone.main && gone.bounds.is_none());
}

#[test]
fn unknown_id_is_not_connected() {
    let mut t = topo();
    assert_eq!(
        t.set_active(DisplayId(99), false),
        Err(TopologyError::NotConnected("#99".into()))
    );
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
                Op::Detach(big) => detach(&mut t, &if big { m34() } else { m32() }),
                Op::Attach(big) => attach(&mut t, &if big { m34() } else { m32() }),
                Op::Builtin(mode) => set_builtin(&mut t, mode).map(drop),
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
