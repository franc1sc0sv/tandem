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
//! - Las dos en la versión 0 (recién generadas, T-082) se unen: la Mac no ve el EDID de
//!   los monitores que solo están en la PC, y la PC puede no ver alguno de la Mac. La
//!   unión es la misma desde los dos lados: los de la PC en su orden y después los que
//!   solo tiene la Mac.
//!
//! Lo solo-local (máquina, identidad, red, puertos, rutas, energía) nunca viaja.

use tandem_core::{Config, Machine};
use tandem_proto::DeskSync;

/// Qué hacer con el puesto que mandó la otra máquina: `Some` es el que queda aquí.
pub fn receive(mine: &DeskSync, theirs: &DeskSync) -> Option<DeskSync> {
    if mine.rev == 0 && theirs.rev == 0 {
        let merged = merge(mine, theirs);
        let same = merged.desk == mine.desk && merged.lock_shortcut == mine.lock_shortcut;
        return (!same).then_some(merged);
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

/// La unión de dos puestos en la versión 0, igual desde los dos lados.
fn merge(a: &DeskSync, b: &DeskSync) -> DeskSync {
    let (pc, mac) = if a.origin == Machine::Pc {
        (a, b)
    } else {
        (b, a)
    };
    let mut desk = pc.desk.clone();
    for m in &mac.desk.monitors {
        if desk.monitor(&m.id).is_none() {
            desk.monitors.push(m.clone());
        }
    }
    DeskSync {
        rev: 0,
        origin: Machine::Pc,
        desk,
        lock_shortcut: pc.lock_shortcut.clone(),
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
    fn two_fresh_desks_are_joined() {
        // La Mac solo ve el 34"; la PC ve los dos.
        let mac = sync(0, Machine::Mac, &["sam711a"]);
        let pc = sync(0, Machine::Pc, &["sam711a", "sam7489"]);
        let on_mac = receive(&mac, &pc).unwrap();
        assert_eq!(on_mac.desk, pc.desk);
        // La PC no cambia: ya tiene todo.
        assert_eq!(receive(&pc, &mac), None);
        // Uno que solo tiene la Mac se agrega al final, en las dos.
        let mac = sync(0, Machine::Mac, &["dell"]);
        let a = receive(&mac, &pc).unwrap();
        let b = receive(&pc, &mac).unwrap();
        assert_eq!(a, b);
        let ids: Vec<&str> = a.desk.monitors.iter().map(|m| m.id.0.as_str()).collect();
        assert_eq!(ids, ["sam711a", "sam7489", "dell"]);
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
            let on_a = receive(&a, &b).unwrap_or_else(|| a.clone());
            let on_b = receive(&b, &a).unwrap_or_else(|| b.clone());
            prop_assert_eq!(&on_a.desk, &on_b.desk);
            prop_assert_eq!(on_a.rev, on_b.rev);
            // Y volver a mandarse lo que quedó no cambia nada.
            prop_assert_eq!(receive(&on_a, &on_b), None);
        }
    }
}
