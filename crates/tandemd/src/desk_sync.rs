//! El puesto compartido entre las dos máquinas (T-084).
//!
//! El acomodo de pantallas (`[[screens]]`, `[arrangement]`) y el atajo de bloqueo son del
//! puesto, no de una máquina: un cambio hecho en cualquiera vale para las dos. Cada agente
//! guarda su copia en `tandem.toml` con un reloj lógico (`desk_rev`, `desk_origin`) y se la
//! manda al otro al conectarse y con cada cambio ([`tandem_proto::ControlMsg::Desk`]).
//!
//! - Un cambio local sube la versión a la más alta conocida + 1, con esta máquina de origen.
//! - Al recibir, gana la versión más nueva; dos iguales las desempata el origen (así las
//!   dos máquinas eligen la misma sin hablar más).
//! - Las dos en la versión 0 (recién generadas) no se mezclan: cada una se queda con lo
//!   suyo hasta que alguien cambie algo.
//! - Un `tandem.toml` editado a mano no coincide con la huella que escribió el agente
//!   ([`fingerprint`]): al arrancar cuenta como un cambio de esta máquina.
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

/// Huella estable del puesto y el atajo (FNV-1a de 64 bits sobre su JSON), en hexadecimal:
/// no depende de la versión de Rust ni de la del agente, así que no cambia sola entre
/// arranques.
pub fn fingerprint(desk: &Config, lock_shortcut: &[String]) -> String {
    // Serializar estos tipos a JSON no falla: no hay mapas con claves no textuales.
    let json = serde_json::to_vec(&(desk, lock_shortcut)).unwrap_or_default();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in json {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Si el puesto de `tandem.toml` lo cambió alguien a mano desde que el agente lo escribió.
/// Sin huella (un archivo generado o de una versión anterior), solo si no es el de
/// fábrica.
pub fn edited_by_hand(
    desk: &Config,
    lock_shortcut: &[String],
    sum: Option<&str>,
    default_lock: &[String],
) -> bool {
    match sum {
        Some(sum) => fingerprint(desk, lock_shortcut) != sum,
        None => *desk != Config::default() || lock_shortcut != default_lock,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tandem_core::{Scale, ScreenSpec};

    use super::*;

    fn screen(id: &str) -> ScreenSpec {
        ScreenSpec {
            machine: Machine::B,
            id: id.to_owned(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: Scale::ONE,
        }
    }

    fn sync(rev: u64, origin: Machine, ids: &[&str]) -> DeskSync {
        DeskSync {
            rev,
            origin,
            desk: Config {
                screens: ids.iter().map(|id| screen(id)).collect(),
                ..Config::default()
            },
            lock_shortcut: vec!["ScrollLock".into()],
        }
    }

    #[test]
    fn the_newer_version_wins() {
        let mine = sync(2, Machine::A, &["a"]);
        let theirs = sync(3, Machine::B, &["a", "b"]);
        assert_eq!(receive(&mine, &theirs), Some(theirs.clone()));
        assert_eq!(receive(&theirs, &mine), None);
    }

    #[test]
    fn a_tie_is_broken_by_origin_the_same_way_on_both_sides() {
        let mac = sync(4, Machine::A, &["a"]);
        let pc = sync(4, Machine::B, &["b"]);
        assert_eq!(receive(&mac, &pc), Some(pc.clone()));
        assert_eq!(receive(&pc, &mac), None);
    }

    #[test]
    fn two_fresh_desks_are_never_joined() {
        let mac = sync(0, Machine::A, &["a"]);
        let pc = sync(0, Machine::B, &["a", "b"]);
        assert_eq!(receive(&mac, &pc), None);
        assert_eq!(receive(&pc, &mac), None);
    }

    #[test]
    fn a_changed_desk_wins_over_a_fresh_one() {
        let fresh = sync(0, Machine::A, &[]);
        let built = sync(1, Machine::B, &["a", "b"]);
        assert_eq!(receive(&fresh, &built), Some(built.clone()));
        assert_eq!(receive(&built, &fresh), None);
    }

    #[test]
    fn a_broken_desk_is_ignored() {
        let mine = sync(1, Machine::A, &["a"]);
        let theirs = sync(2, Machine::B, &["a", "a"]);
        assert_eq!(receive(&mine, &theirs), None);
    }

    #[test]
    fn a_local_change_is_newer_than_anything_known() {
        let mine = sync(7, Machine::B, &["a"]);
        let next = bump(mine.rev, Machine::A, mine.desk.clone(), vec![]);
        assert_eq!((next.rev, next.origin), (8, Machine::A));
        assert_eq!(receive(&mine, &next), Some(next.clone()));
    }

    #[test]
    fn a_hand_edit_is_noticed_by_the_fingerprint() {
        let lock = vec!["ScrollLock".to_owned()];
        let desk = sync(1, Machine::B, &["a"]).desk;
        let sum = fingerprint(&desk, &lock);
        assert_eq!(sum.len(), 16);
        assert!(!edited_by_hand(&desk, &lock, Some(&sum), &lock));
        let other = sync(1, Machine::B, &["b"]).desk;
        assert!(edited_by_hand(&other, &lock, Some(&sum), &lock));
        assert!(edited_by_hand(&desk, &[], Some(&sum), &lock));
        // Sin huella: lo de fábrica no es una edición; cualquier otra cosa, sí.
        assert!(!edited_by_hand(&Config::default(), &lock, None, &lock));
        assert!(edited_by_hand(&desk, &lock, None, &lock));
        assert!(edited_by_hand(&Config::default(), &[], None, &lock));
        // La huella es estable: la misma entrada da lo mismo.
        assert_eq!(fingerprint(&desk, &lock), sum);
    }

    fn any_sync() -> impl Strategy<Value = DeskSync> {
        let ids = proptest::sample::subsequence(vec!["a", "b", "c", "d"], 0..=4);
        (
            0u64..4,
            prop_oneof![Just(Machine::A), Just(Machine::B)],
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
        /// de la otra máquina, que alguien cambió a propósito.
        #[test]
        fn sync_never_mixes_desks(a in any_sync(), b in any_sync()) {
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
