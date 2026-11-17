//! Cuánto pueden usar los datos según el camino (DECISIONS A2) y cuánta latencia de la
//! entrada se tolera en cada uno (DESIGN §7.1).

use std::time::Duration;

use tandem_core::DataPrefs;

use super::limiter::MIN_CAP;
use crate::manager::LinkKind;

/// Lo que se supone que da el Wi-Fi mientras nadie lo midió (T-102 lo mide a pedido):
/// 20 MB/s (160 Mbit/s), lo que da en la práctica un Wi-Fi 5 modesto. Con el 50 % por
/// omisión, 10 MB/s para los datos.
pub const UNMEASURED_WIFI: u64 = 20_000_000;

/// El tope de los datos por `link`, en bytes por segundo; `None` es sin tope (cable).
/// `measured` es el último ancho de banda medido de ese camino.
pub fn data_cap(link: Option<LinkKind>, prefs: &DataPrefs, measured: Option<u64>) -> Option<u64> {
    match link? {
        LinkKind::Wifi => Some(wifi_cap(prefs, measured)),
        LinkKind::Usb4 | LinkKind::Ethernet => None,
    }
}

fn wifi_cap(prefs: &DataPrefs, measured: Option<u64>) -> u64 {
    let cap = prefs.fixed_cap_bytes().unwrap_or_else(|| {
        let base = measured.filter(|m| *m > 0).unwrap_or(UNMEASURED_WIFI);
        let percent = u64::from(prefs.wifi_cap_percent.clamp(1, 100));
        base / 100 * percent + base % 100 * percent / 100
    });
    cap.max(MIN_CAP)
}

/// El p99 que se tolera a la entrada por `link` (DESIGN §7.1). Sin camino conocido, el
/// del Wi-Fi, el más laxo.
pub fn input_p99_target(link: Option<LinkKind>) -> Duration {
    match link {
        Some(LinkKind::Usb4 | LinkKind::Ethernet) => Duration::from_millis(4),
        Some(LinkKind::Wifi) | None => Duration::from_millis(20),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn por_cable_no_hay_tope() {
        let p = DataPrefs::default();
        assert_eq!(data_cap(Some(LinkKind::Usb4), &p, Some(1000)), None);
        assert_eq!(data_cap(Some(LinkKind::Ethernet), &p, None), None);
        assert_eq!(data_cap(None, &p, None), None);
    }

    #[test]
    fn por_wifi_la_mitad_de_lo_medido() {
        let p = DataPrefs::default();
        assert_eq!(
            data_cap(Some(LinkKind::Wifi), &p, Some(30_000_000)),
            Some(15_000_000)
        );
        // Sin medición, la mitad de lo supuesto.
        assert_eq!(
            data_cap(Some(LinkKind::Wifi), &p, None),
            Some(UNMEASURED_WIFI / 2)
        );
        // Una medición en 0 no deja los datos parados para siempre.
        assert_eq!(
            data_cap(Some(LinkKind::Wifi), &p, Some(0)),
            Some(10_000_000)
        );
    }

    #[test]
    fn el_porcentaje_y_el_tope_fijo_se_respetan() {
        let pct = DataPrefs {
            wifi_cap_percent: 30,
            wifi_cap_mbps: None,
        };
        assert_eq!(
            data_cap(Some(LinkKind::Wifi), &pct, Some(10_000_000)),
            Some(3_000_000)
        );
        let fixed = DataPrefs {
            wifi_cap_percent: 30,
            wifi_cap_mbps: Some(40),
        };
        assert_eq!(
            data_cap(Some(LinkKind::Wifi), &fixed, Some(10_000_000)),
            Some(5_000_000)
        );
        // Nunca por debajo del mínimo, ni con cifras enormes se desborda.
        assert_eq!(
            data_cap(Some(LinkKind::Wifi), &pct, Some(1000)),
            Some(MIN_CAP)
        );
        assert!(data_cap(Some(LinkKind::Wifi), &pct, Some(u64::MAX)).is_some());
    }

    #[test]
    fn objetivos_de_latencia_del_diseño() {
        assert_eq!(
            input_p99_target(Some(LinkKind::Usb4)),
            Duration::from_millis(4)
        );
        assert_eq!(
            input_p99_target(Some(LinkKind::Ethernet)),
            Duration::from_millis(4)
        );
        assert_eq!(
            input_p99_target(Some(LinkKind::Wifi)),
            Duration::from_millis(20)
        );
    }
}
