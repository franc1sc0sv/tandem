//! Del resultado de una calibración al perfil de la base (`<PNP>.toml`, T-079), que es lo
//! que se guarda como perfil del usuario y lo que se comparte.

use std::time::{SystemTime, UNIX_EPOCH};

use tandem_core::Input;

use super::CalibratedProfile;
use crate::ddc::profiles::{DdcProfile, ProfileCodes, ReportedCodes};
use crate::ddc::switch::input_name;

/// Quién figura como verificador de un perfil calibrado.
pub const VERIFIED_BY: &str = "calibración de Tandem";

impl CalibratedProfile {
    /// El perfil para la base. `None` sin EDID: sin id PnP no hay con qué reconocerlo.
    ///
    /// Se marca `generic_name`: solo se probó este modelo, y un nombre del EDID como
    /// «Odyssey G5» lo comparten otros con códigos distintos.
    pub fn to_profile(&self, verified_on: Option<String>) -> Option<DdcProfile> {
        let pnp_id = self.pnp_id.clone()?;
        let codes = self.ddc_profile()?;
        let name = self
            .edid_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| self.description.clone());
        let name = if name.trim().is_empty() {
            pnp_id.clone()
        } else {
            name
        };
        let declared = self
            .declared
            .iter()
            .map(|c| format!("{c:02X}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut notes = format!(
            "Calibrado con Tandem: {} códigos probados, vuelta a {} verificada.",
            self.attempts,
            input_name(self.start)
        );
        if !declared.is_empty() {
            notes.push_str(&format!("\nDeclara 60 = {{{declared}}}."));
        }
        Some(DdcProfile {
            name: format!("{name} ({pnp_id})"),
            pnp_id,
            pnp_aliases: Vec::new(),
            edid_names: self.edid_name.clone().into_iter().collect(),
            generic_name: true,
            codes: ProfileCodes {
                hdmi: codes.hdmi,
                display_port: codes.display_port,
                hdmi2: None,
                display_port2: None,
            },
            reported: ReportedCodes {
                hdmi: self.reported.hdmi,
                display_port: self.reported.display_port,
                hdmi2: None,
                display_port2: None,
            },
            notes: Some(notes),
            verified_by: Some(VERIFIED_BY.into()),
            verified_on,
        })
    }

    /// El resultado en una línea: `Odyssey G5: HDMI ✓ 0x05 · DisplayPort ✓ 0x0F`.
    pub fn summary(&self, label: &str) -> String {
        let code = |input| match input {
            Input::Hdmi => self.codes.hdmi.first(),
            Input::DisplayPort => self.codes.display_port.first(),
        };
        let part = |input| match code(input) {
            Some(c) => format!("{} ✓ {c:#04X}", short_name(input)),
            None => format!("{} ✗", short_name(input)),
        };
        format!(
            "{label}: {} · {}",
            part(Input::Hdmi),
            part(Input::DisplayPort)
        )
    }
}

fn short_name(input: Input) -> &'static str {
    match input {
        Input::Hdmi => "HDMI",
        Input::DisplayPort => "DP",
    }
}

/// La fecha de hoy (UTC) como `AAAA-MM-DD`, para `verified_on`.
pub fn today() -> Option<String> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(civil_date(i64::try_from(secs / 86_400).ok()?))
}

/// Días desde 1970-01-01 a fecha del calendario gregoriano (algoritmo de los días civiles
/// de H. Hinnant), para no sumar una dependencia por una fecha.
pub fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibrate::Reported;
    use crate::ddc::switch::InputCodes;
    use proptest::prelude::*;

    fn calibrated() -> CalibratedProfile {
        CalibratedProfile {
            codes: InputCodes {
                hdmi: vec![0x05],
                display_port: vec![0x0F],
            },
            pnp_id: Some("SAM7489".into()),
            edid_name: Some("Odyssey G5".into()),
            description: "Monitor PnP genérico".into(),
            start: Input::DisplayPort,
            reported: Reported {
                hdmi: Some(0x05),
                display_port: Some(0x0F),
            },
            declared: vec![0x01, 0x0F],
            mccs_ver: Some("2.0".into()),
            attempts: 2,
        }
    }

    #[test]
    fn the_saved_profile_reads_back_with_the_codes_and_the_hints() {
        let p = calibrated().to_profile(Some("2026-10-16".into())).unwrap();
        let back = DdcProfile::parse(&p.to_toml()).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.pnp_id, "SAM7489");
        assert_eq!(back.name, "Odyssey G5 (SAM7489)");
        assert_eq!((back.codes.hdmi, back.codes.display_port), (0x05, 0x0F));
        assert_eq!(back.reported.hdmi, Some(0x05));
        assert!(back.generic_name);
        assert_eq!(back.verified_by.as_deref(), Some(VERIFIED_BY));
        let notes = back.notes.unwrap();
        assert!(notes.contains("2 códigos probados"), "{notes}");
        assert!(notes.contains("{01, 0F}"), "{notes}");
    }

    #[test]
    fn without_edid_there_is_nothing_to_save() {
        let mut c = calibrated();
        c.pnp_id = None;
        assert!(c.to_profile(None).is_none());
        // Sin nombre en el EDID, la descripción del sistema.
        let mut c = calibrated();
        c.edid_name = None;
        assert_eq!(
            c.to_profile(None).unwrap().name,
            "Monitor PnP genérico (SAM7489)"
        );
    }

    #[test]
    fn summary_has_a_check_per_input() {
        assert_eq!(
            calibrated().summary("32\""),
            "32\": HDMI ✓ 0x05 · DP ✓ 0x0F"
        );
        let mut c = calibrated();
        c.codes.hdmi.clear();
        assert_eq!(c.summary("x"), "x: HDMI ✗ · DP ✓ 0x0F");
    }

    #[test]
    fn known_dates() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(59), "1970-03-01");
        assert_eq!(civil_date(11_016), "2000-02-29");
        assert_eq!(civil_date(20_742), "2026-10-16");
        assert!(today().is_some_and(|d| d.len() == 10));
    }

    proptest! {
        #[test]
        fn dates_are_valid_and_consecutive(days in 0i64..200_000) {
            let (a, b) = (civil_date(days), civil_date(days + 1));
            prop_assert!(a < b, "{a} {b}");
            let p = DdcProfile {
                verified_on: Some(a),
                ..calibrated().to_profile(None).unwrap()
            };
            prop_assert!(p.validate().is_ok());
        }
    }
}
