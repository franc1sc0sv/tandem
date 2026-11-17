//! El puesto compartido entre las dos máquinas (T-084, T-099).
//!
//! El puesto ([`tandem_core::Desk`]: máquinas, acomodo, escenas, atajos, preferencias y
//! módulos) es de las dos, no de una máquina: un cambio hecho en cualquiera vale para las
//! dos. Cada agente guarda su copia en `tandem.toml` con su sello (`desk_rev`, `desk_at`,
//! `desk_origin`) y se la manda al otro al conectarse y con cada cambio
//! ([`tandem_proto::ControlMsg::Desk`]). Gana el último ([`tandem_core::DeskVersion`]).
//!
//! Un `tandem.toml` editado a mano no coincide con la huella que escribió el agente
//! ([`fingerprint`]): al arrancar cuenta como un cambio de esta máquina.
//!
//! Lo solo-local (máquina, identidad, red, puertos, rutas, energía) nunca viaja.

use tandem_core::Desk;

/// Huella estable del puesto (FNV-1a de 64 bits sobre su JSON), en hexadecimal: no depende
/// de la versión de Rust ni de la del agente, así que no cambia sola entre arranques.
pub fn fingerprint(desk: &Desk) -> String {
    // Serializar estos tipos a JSON no falla: los mapas tienen claves de texto.
    let json = serde_json::to_vec(desk).unwrap_or_default();
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
pub fn edited_by_hand(desk: &Desk, sum: Option<&str>) -> bool {
    match sum {
        Some(sum) => fingerprint(desk) != sum,
        None => *desk != Desk::default(),
    }
}

#[cfg(test)]
mod tests {
    use tandem_core::{Machine, Scale, ScreenSpec};

    use super::*;

    fn desk(ids: &[&str]) -> Desk {
        Desk {
            screens: ids
                .iter()
                .map(|id| ScreenSpec {
                    machine: Machine::B,
                    id: (*id).to_owned(),
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                    scale: Scale::ONE,
                })
                .collect(),
            ..Desk::default()
        }
    }

    #[test]
    fn a_hand_edit_is_noticed_by_the_fingerprint() {
        let d = desk(&["a"]);
        let sum = fingerprint(&d);
        assert_eq!(sum.len(), 16);
        assert!(!edited_by_hand(&d, Some(&sum)));
        assert!(edited_by_hand(&desk(&["b"]), Some(&sum)));
        let mut lock = d.clone();
        lock.shortcuts.lock.clear();
        assert!(edited_by_hand(&lock, Some(&sum)));
        // Sin huella: lo de fábrica no es una edición; cualquier otra cosa, sí.
        assert!(!edited_by_hand(&Desk::default(), None));
        assert!(edited_by_hand(&d, None));
        let mut prefs = Desk::default();
        prefs.prefs.drag_files = false;
        assert!(edited_by_hand(&prefs, None));
        // La huella es estable: la misma entrada da lo mismo.
        assert_eq!(fingerprint(&d), sum);
    }
}
