//! Qué rutas tocar para activar o quitar un monitor.
//!
//! Es el modelo de Windows (`QueryDisplayConfig` con `QDC_ALL_PATHS`): una ruta une una
//! fuente del adaptador (una "superficie" del escritorio) con un target (la salida física
//! donde está el monitor). Para cada target conectado hay una ruta inactiva por cada fuente
//! posible; las activas son el escritorio actual. Activar un monitor = activar una de sus
//! rutas con una fuente que nadie use; quitarlo = desactivar sus rutas activas.

use std::collections::{BTreeMap, BTreeSet};

use super::{TargetId, TopologyError};

/// Una ruta fuente → target, reducida a lo que importa para decidir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathSlot {
    pub adapter: u64,
    pub source: u32,
    pub target: u32,
    pub active: bool,
    /// ¿Hay un monitor conectado en el target?
    pub available: bool,
}

impl PathSlot {
    pub fn target_id(&self) -> TargetId {
        TargetId {
            adapter: self.adapter,
            target: self.target,
        }
    }
}

/// Targets con monitor conectado (o activos) y si están en el escritorio, ordenados.
pub fn targets(paths: &[PathSlot]) -> Vec<(TargetId, bool)> {
    let mut out: BTreeMap<TargetId, bool> = BTreeMap::new();
    for p in paths.iter().filter(|p| p.available || p.active) {
        *out.entry(p.target_id()).or_default() |= p.active;
    }
    out.into_iter().collect()
}

/// Índices de las rutas a desactivar para quitar `target`. Vacío si ya estaba fuera.
pub fn plan_detach(paths: &[PathSlot], target: TargetId) -> Result<Vec<usize>, TopologyError> {
    let off: Vec<usize> = active(paths)
        .filter(|(_, p)| p.target_id() == target)
        .map(|(i, _)| i)
        .collect();
    if !off.is_empty() && active(paths).all(|(_, p)| p.target_id() == target) {
        return Err(TopologyError::LastDisplay(target.to_string()));
    }
    Ok(off)
}

/// Índice de la ruta a activar para poner `target` en el escritorio, o `None` si ya está.
///
/// Se elige la fuente libre más baja del adaptador: es la que Windows usaría y deja el
/// resultado determinista.
pub fn plan_attach(paths: &[PathSlot], target: TargetId) -> Result<Option<usize>, TopologyError> {
    if active(paths).any(|(_, p)| p.target_id() == target) {
        return Ok(None);
    }
    let used: BTreeSet<(u64, u32)> = active(paths).map(|(_, p)| (p.adapter, p.source)).collect();
    let mut connected = false;
    let mut best: Option<(u32, usize)> = None;
    for (i, p) in paths.iter().enumerate() {
        if p.target_id() != target || !p.available {
            continue;
        }
        connected = true;
        if used.contains(&(p.adapter, p.source)) {
            continue;
        }
        if best.is_none_or(|(source, _)| p.source < source) {
            best = Some((p.source, i));
        }
    }
    match best {
        Some((_, i)) => Ok(Some(i)),
        None if connected => Err(TopologyError::NoFreeSource(target.to_string())),
        None => Err(TopologyError::NotConnected(target.to_string())),
    }
}

/// Índice de modo "sin modo" (`DISPLAYCONFIG_PATH_MODE_IDX_INVALID`).
pub const MODE_IDX_INVALID: u32 = u32::MAX;

/// Deja en la tabla de modos solo los que usan las rutas y renumera sus referencias.
///
/// Al quitar una ruta, su modo queda huérfano; mandarlo igual a `SetDisplayConfig` es
/// pedirle que valide algo que no se usa. Devuelve los índices (en la tabla original) de
/// los modos que se conservan, en orden; las referencias fuera de rango pasan a inválidas.
pub fn compact_modes(refs: &mut [u32], modes: usize) -> Vec<usize> {
    let mut keep: Vec<usize> = Vec::new();
    let mut remap: BTreeMap<u32, u32> = BTreeMap::new();
    for r in refs.iter_mut() {
        if *r == MODE_IDX_INVALID || *r as usize >= modes {
            *r = MODE_IDX_INVALID;
            continue;
        }
        let next = keep.len() as u32;
        let new = *remap.entry(*r).or_insert_with(|| {
            keep.push(*r as usize);
            next
        });
        *r = new;
    }
    keep
}

fn active(paths: &[PathSlot]) -> impl Iterator<Item = (usize, &PathSlot)> {
    paths.iter().enumerate().filter(|(_, p)| p.active)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn slot(source: u32, target: u32, active: bool, available: bool) -> PathSlot {
        PathSlot {
            adapter: 7,
            source,
            target,
            active,
            available,
        }
    }

    fn tid(target: u32) -> TargetId {
        TargetId { adapter: 7, target }
    }

    /// Dos fuentes, tres targets: 1 y 2 activos (fuentes 0 y 1), 3 desconectado.
    fn sample() -> Vec<PathSlot> {
        let mut v = Vec::new();
        for target in 1..=3 {
            for source in 0..2 {
                let active = (target == 1 && source == 0) || (target == 2 && source == 1);
                v.push(slot(source, target, active, target != 3));
            }
        }
        v
    }

    #[test]
    fn lists_connected_targets_with_state() {
        let mut paths = sample();
        paths[0].active = false;
        assert_eq!(targets(&paths), [(tid(1), false), (tid(2), true)]);
    }

    #[test]
    fn detach_turns_off_only_the_target_paths() {
        assert_eq!(plan_detach(&sample(), tid(1)).unwrap(), [0]);
        assert!(plan_detach(&sample(), tid(3)).unwrap().is_empty());
    }

    #[test]
    fn detach_of_the_only_active_target_is_refused() {
        let mut paths = sample();
        paths[3].active = false;
        assert_eq!(
            plan_detach(&paths, tid(1)),
            Err(TopologyError::LastDisplay(tid(1).to_string()))
        );
        // Si ya estaba fuera y no queda nada activo, no hay nada que hacer.
        paths[0].active = false;
        assert_eq!(plan_detach(&paths, tid(1)), Ok(vec![]));
    }

    #[test]
    fn attach_picks_the_lowest_free_source() {
        let mut paths = sample();
        paths[0].active = false;
        assert_eq!(plan_attach(&paths, tid(1)), Ok(Some(0)));
        // Con la fuente 0 ocupada por el target 2, queda la 1.
        paths[3].active = false;
        paths[2].active = true;
        assert_eq!(plan_attach(&paths, tid(1)), Ok(Some(1)));
    }

    #[test]
    fn attach_without_free_source_fails() {
        let paths = vec![slot(0, 1, true, true), slot(0, 2, false, true)];
        assert_eq!(
            plan_attach(&paths, tid(2)),
            Err(TopologyError::NoFreeSource(tid(2).to_string()))
        );
    }

    #[test]
    fn attach_is_a_noop_when_active_and_fails_when_unplugged() {
        assert_eq!(plan_attach(&sample(), tid(1)), Ok(None));
        assert_eq!(
            plan_attach(&sample(), tid(3)),
            Err(TopologyError::NotConnected(tid(3).to_string()))
        );
        assert_eq!(
            plan_attach(&sample(), tid(9)),
            Err(TopologyError::NotConnected(tid(9).to_string()))
        );
    }

    #[test]
    fn sources_are_per_adapter() {
        // La fuente 0 del adaptador 7 está ocupada, pero la del adaptador 8 no.
        let mut other = slot(0, 1, false, true);
        other.adapter = 8;
        let paths = vec![slot(0, 2, true, true), other];
        let t = TargetId {
            adapter: 8,
            target: 1,
        };
        assert_eq!(plan_attach(&paths, t), Ok(Some(1)));
    }

    #[test]
    fn compact_modes_drops_orphans_and_renumbers() {
        const X: u32 = MODE_IDX_INVALID;
        // Rutas que usan los modos 4, 1, 4 (compartido), ninguno y uno fuera de rango.
        let mut refs = [4, 1, 4, X, 9];
        assert_eq!(compact_modes(&mut refs, 6), [4, 1]);
        assert_eq!(refs, [0, 1, 0, X, X]);
        let mut none: [u32; 0] = [];
        assert!(compact_modes(&mut none, 3).is_empty());
    }

    proptest! {
        #[test]
        fn compacted_refs_point_to_the_same_modes(
            original in proptest::collection::vec(
                prop_oneof![Just(MODE_IDX_INVALID), 0u32..12],
                0..10,
            ),
            modes in 0usize..10,
        ) {
            let mut refs = original.clone();
            let keep = compact_modes(&mut refs, modes);
            prop_assert!(keep.iter().all(|&k| k < modes));
            let unique: BTreeSet<_> = keep.iter().collect();
            prop_assert_eq!(unique.len(), keep.len());
            for (before, after) in original.iter().zip(&refs) {
                if *before == MODE_IDX_INVALID || *before as usize >= modes {
                    prop_assert_eq!(*after, MODE_IDX_INVALID);
                } else {
                    prop_assert_eq!(keep[*after as usize], *before as usize);
                }
            }
        }
    }

    /// Topología válida al azar: hasta 2 adaptadores × 3 fuentes × 4 targets, sin clones
    /// (cada fuente y cada target con a lo sumo una ruta activa), y algún monitor activo.
    fn topology() -> impl Strategy<Value = Vec<PathSlot>> {
        (
            1u64..=2,
            proptest::collection::vec(any::<bool>(), 8),
            proptest::collection::vec(proptest::option::of(0u32..3), 8),
        )
            .prop_map(|(adapters, available, wanted)| {
                let mut paths = Vec::new();
                let mut used = BTreeSet::new();
                for adapter in 1..=adapters {
                    for target in 0..4u32 {
                        let k = ((adapter - 1) * 4) as usize + target as usize;
                        let avail = available[k];
                        let source = wanted[k].filter(|s| avail && used.insert((adapter, *s)));
                        for s in 0..3 {
                            paths.push(PathSlot {
                                adapter,
                                source: s,
                                target,
                                active: source == Some(s),
                                available: avail,
                            });
                        }
                    }
                }
                paths
            })
            .prop_filter("al menos un monitor activo", |p| p.iter().any(|s| s.active))
    }

    fn valid(paths: &[PathSlot]) -> bool {
        let mut sources = BTreeSet::new();
        let mut tgts = BTreeSet::new();
        paths
            .iter()
            .filter(|p| p.active)
            .all(|p| sources.insert((p.adapter, p.source)) && tgts.insert(p.target_id()))
    }

    proptest! {
        #[test]
        fn attach_activates_the_target_without_conflicts(
            paths in topology(),
            adapter in 1u64..=2,
            target in 0u32..4,
        ) {
            let t = TargetId { adapter, target };
            let before = targets(&paths);
            match plan_attach(&paths, t) {
                Ok(None) => prop_assert!(before.contains(&(t, true))),
                Ok(Some(i)) => {
                    let mut after = paths.clone();
                    prop_assert!(!after[i].active && after[i].available);
                    after[i].active = true;
                    prop_assert!(valid(&after));
                    // Solo cambia el target pedido.
                    let expected: Vec<_> = before
                        .iter()
                        .map(|(id, a)| (*id, *a || *id == t))
                        .collect();
                    prop_assert_eq!(targets(&after), expected);
                }
                Err(TopologyError::NoFreeSource(_)) => {
                    // Sin fuente libre: todas las del adaptador están ocupadas.
                    let busy = paths.iter().filter(|p| p.active && p.adapter == adapter).count();
                    prop_assert_eq!(busy, 3);
                }
                Err(TopologyError::NotConnected(_)) => {
                    prop_assert!(!paths.iter().any(|p| p.target_id() == t && p.available));
                }
                Err(e) => prop_assert!(false, "error inesperado: {e}"),
            }
        }

        #[test]
        fn detach_never_leaves_an_empty_desktop(
            paths in topology(),
            adapter in 1u64..=2,
            target in 0u32..4,
        ) {
            let t = TargetId { adapter, target };
            let before = targets(&paths);
            match plan_detach(&paths, t) {
                Ok(off) => {
                    let mut after = paths.clone();
                    for i in off {
                        after[i].active = false;
                    }
                    prop_assert!(after.iter().any(|p| p.active));
                    let expected: Vec<_> = before
                        .iter()
                        .map(|(id, a)| (*id, *a && *id != t))
                        .collect();
                    prop_assert_eq!(targets(&after), expected);
                }
                Err(TopologyError::LastDisplay(_)) => {
                    let actives: Vec<_> = before.iter().filter(|(_, a)| *a).copied().collect();
                    prop_assert_eq!(actives, vec![(t, true)]);
                }
                Err(e) => prop_assert!(false, "error inesperado: {e}"),
            }
        }
    }
}
