//! El puesto compartido entre las dos máquinas (T-084).
//!
//! Los monitores, sus perfiles DDC, el cableado y el atajo de bloqueo son del puesto, no
//! de una máquina: un cambio hecho en la UI de cualquiera vale para las dos. Cada agente
//! guarda su copia en `tandem.toml` con un reloj lógico (`desk_rev`, `desk_origin`) y se la
//! manda al otro al conectarse y con cada cambio ([`tandem_proto::ControlMsg::Desk`]).
//!
//! - Un cambio local sube la versión a la más alta conocida + 1, con esta máquina de origen.
//! - Al recibir, gana la versión más nueva; dos iguales las desempata el origen (así las
//!   dos máquinas eligen la misma sin hablar más).
//! - Las dos en la versión 0 (recién generadas) no se mezclan: **la sincronización nunca
//!   crea monitores** (T-087). Los monitores salen solo del asistente o de «Agregar
//!   monitor», que suben la versión; antes (T-083) se unían y por eso un mismo monitor
//!   quedaba duplicado al verse con un id PnP distinto en cada máquina.
//!
//! Lo solo-local (máquina, identidad, red, puertos, rutas, energía) nunca viaja.

use tandem_core::{Config, Machine};
use tandem_proto::DeskSync;

/// Qué hacer con el puesto que mandó la otra máquina: `Some` es el que queda aquí.
pub fn receive(mine: &DeskSync, theirs: &DeskSync) -> Option<DeskSync> {
    if mine.rev == 0 && theirs.rev == 0 {
        return None;
    }
    let newer = (theirs.rev, theirs.origin) > (mine.rev, mine.origin);
    (newer && theirs.desk.validate().is_ok()).then(|| theirs.clone())
}

/// Un cambio hecho aquí sobre la versión `rev`: la siguiente, con `me` de origen.
pub fn bump(rev: u64, me: Machine, desk: Config, lock_shortcut: Vec<String>) -> DeskSync {
    DeskSync {
        rev: rev + 1,
        origin: me,
        desk,
        lock_shortcut,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tandem_core::{Input, MonitorConfig};

    use super::*;

    fn monitor(id: &str) -> MonitorConfig {
        MonitorConfig {
            id: id.into(),
            label: id.to_owned(),
            edid_model: Some(id.to_ascii_uppercase()),
            edid_aliases: Vec::new(),
            wiring: [
                (Machine::Pc, Input::DisplayPort),
                (Machine::Mac, Input::Hdmi),
            ]
            .into(),
            ddc: None,
        }
    }

    fn sync(rev: u64, origin: Machine, ids: &[&str]) -> DeskSync {
        DeskSync {
            rev,
            origin,
            desk: Config {
                monitors: ids.iter().map(|id| monitor(id)).collect(),
            },
            lock_shortcut: vec!["ScrollLock".into()],
        }
    }

    #[test]
    fn the_newer_version_wins() {
        let mine = sync(2, Machine::Mac, &["a"]);
        let theirs = sync(3, Machine::Pc, &["a", "b"]);
        assert_eq!(receive(&mine, &theirs), Some(theirs.clone()));
        assert_eq!(receive(&theirs, &mine), None);
    }

    #[test]
    fn a_tie_is_broken_by_origin_the_same_way_on_both_sides() {
        let mac = sync(4, Machine::Mac, &["a"]);
        let pc = sync(4, Machine::Pc, &["b"]);
        assert_eq!(receive(&mac, &pc), Some(pc.clone()));
        assert_eq!(receive(&pc, &mac), None);
    }

    #[test]
    fn two_fresh_desks_are_never_joined() {
        // Ninguna sincronización crea monitores (T-087): ni con puestos distintos, ni con
        // uno vacío y otro con monitores de una versión anterior.
        let mac = sync(0, Machine::Mac, &["sam711a"]);
        let pc = sync(0, Machine::Pc, &["sam711a", "sam7489"]);
        assert_eq!(receive(&mac, &pc), None);
        assert_eq!(receive(&pc, &mac), None);
        let empty = sync(0, Machine::Mac, &[]);
        assert_eq!(receive(&empty, &pc), None);
        assert_eq!(receive(&pc, &empty), None);
    }

    #[test]
    fn a_desk_with_monitors_wins_over_a_fresh_one_only_if_it_was_changed() {
        let fresh = sync(0, Machine::Mac, &[]);
        let built = sync(1, Machine::Pc, &["sam711a", "sam7489"]);
        assert_eq!(receive(&fresh, &built), Some(built.clone()));
        assert_eq!(receive(&built, &fresh), None);
    }

    #[test]
    fn a_broken_desk_is_ignored() {
        let mine = sync(1, Machine::Mac, &["a"]);
        let mut theirs = sync(2, Machine::Pc, &["a"]);
        theirs.desk.monitors.push(monitor("a"));
        assert_eq!(receive(&mine, &theirs), None);
    }

    #[test]
    fn a_local_change_is_newer_than_anything_known() {
        let mine = sync(7, Machine::Pc, &["a"]);
        let next = bump(mine.rev, Machine::Mac, mine.desk.clone(), vec![]);
        assert_eq!((next.rev, next.origin), (8, Machine::Mac));
        assert_eq!(receive(&mine, &next), Some(next.clone()));
    }

    fn any_sync() -> impl Strategy<Value = DeskSync> {
        let ids = proptest::sample::subsequence(vec!["a", "b", "c", "d"], 0..=4);
        (
            0u64..4,
            prop_oneof![Just(Machine::Mac), Just(Machine::Pc)],
            ids,
        )
            .prop_map(|(rev, origin, ids)| sync(rev, origin, &ids))
    }

    proptest! {
        /// Tras mandarse cada una lo suyo, las dos máquinas quedan con el mismo puesto.
        #[test]
        fn both_sides_converge(a in any_sync(), b in any_sync()) {
            prop_assume!(a.origin != b.origin);
            // Dos puestos sin cambiar nunca se mezclan: cada uno queda como estaba.
            prop_assume!(a.rev != 0 || b.rev != 0);
            let on_a = receive(&a, &b).unwrap_or_else(|| a.clone());
            let on_b = receive(&b, &a).unwrap_or_else(|| b.clone());
            prop_assert_eq!(&on_a.desk, &on_b.desk);
            prop_assert_eq!(on_a.rev, on_b.rev);
            // Y volver a mandarse lo que quedó no cambia nada.
            prop_assert_eq!(receive(&on_a, &on_b), None);
        }

        /// La sincronización nunca inventa un puesto: o deja el que hay o adopta entero el
        /// de la otra máquina, que alguien cambió a propósito. Sin cambios, no hace nada.
        #[test]
        fn sync_never_creates_monitors(a in any_sync(), b in any_sync()) {
            match receive(&a, &b) {
                None => {}
                Some(next) => {
                    prop_assert_eq!(&next, &b);
                    prop_assert!(b.rev > 0, "un puesto sin cambiar no se adopta");
                }
            }
        }
    }
}
