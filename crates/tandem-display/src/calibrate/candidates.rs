//! Orden de los códigos que se prueban para una entrada.
//!
//! Lo que aprendimos con los dos Samsung (T-050) manda el orden: lo que el monitor reporta
//! estando en la entrada acertó en el 32" y en el DP del 34"; MCCS y el capabilities
//! string casi nunca aciertan pero cuestan poco; la marca ayuda con modelos hermanos; el
//! barrido es el último recurso porque cada fallo cuesta un timeout del `Confirmer`.

use std::fmt;

use tandem_core::Input;

use crate::ddc::switch::InputCodes;

/// Rango del barrido de último recurso.
pub const SWEEP: std::ops::RangeInclusive<u8> = 0x01..=0x1F;

/// De dónde sale un candidato, en orden de prioridad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    /// Lo que el monitor devolvió en `0x60` estando en esa entrada.
    Reported,
    /// Los valores de MCCS para la entrada.
    Mccs,
    /// Los valores declarados de `0x60` en el capabilities string (sin entrada asociada).
    Capabilities,
    /// Códigos de esa entrada en perfiles del mismo fabricante.
    SameBrand,
    Sweep,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Source::Reported => "lectura del monitor",
            Source::Mccs => "MCCS",
            Source::Capabilities => "capabilities",
            Source::SameBrand => "misma marca",
            Source::Sweep => "barrido",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub code: u8,
    pub source: Source,
}

/// Perfil verificado de otro monitor, usado solo como pista para los de su marca.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrandProfile {
    /// Fabricante PnP de tres letras (`SAM`).
    pub manufacturer: String,
    pub codes: InputCodes,
}

/// MCCS 2.2: `0x0F`/`0x10` son DP-1/DP-2 y `0x11`/`0x12` HDMI-1/HDMI-2.
pub fn mccs(input: Input) -> [u8; 2] {
    match input {
        Input::DisplayPort => [0x0F, 0x10],
        Input::Hdmi => [0x11, 0x12],
    }
}

/// Lo que se sabe del monitor antes de probar una entrada.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hints {
    /// Lectura de `0x60` estando en la entrada que se busca.
    pub reported: Option<u8>,
    /// Valores declarados de `0x60` en el capabilities string.
    pub declared: Vec<u8>,
    /// Códigos de la entrada en perfiles de la misma marca, en el orden de la base.
    pub same_brand: Vec<u8>,
}

/// Códigos de la marca del monitor para una entrada, sin repetir, en el orden de la base.
pub fn brand_codes(profiles: &[BrandProfile], manufacturer: Option<&str>, input: Input) -> Vec<u8> {
    let Some(manufacturer) = manufacturer else {
        return Vec::new();
    };
    profiles
        .iter()
        .filter(|p| p.manufacturer.eq_ignore_ascii_case(manufacturer))
        .flat_map(|p| p.codes.for_input(input).iter().copied())
        .collect()
}

/// Candidatos para `input`, sin duplicados y sin los de `exclude` (los códigos ya
/// verificados para otra entrada: un código no elige dos entradas). El `0x00` nunca se
/// prueba: no es una entrada en MCCS.
pub fn ordered(input: Input, hints: &Hints, exclude: &[u8]) -> Vec<Candidate> {
    let groups: [(Source, Vec<u8>); 5] = [
        (Source::Reported, hints.reported.into_iter().collect()),
        (Source::Mccs, mccs(input).to_vec()),
        (Source::Capabilities, hints.declared.clone()),
        (Source::SameBrand, hints.same_brand.clone()),
        (Source::Sweep, SWEEP.collect()),
    ];
    let mut out: Vec<Candidate> = Vec::new();
    for (source, codes) in groups {
        for code in codes {
            if code == 0 || exclude.contains(&code) || out.iter().any(|c| c.code == code) {
                continue;
            }
            out.push(Candidate { code, source });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn codes(c: &[Candidate]) -> Vec<u8> {
        c.iter().map(|c| c.code).collect()
    }

    #[test]
    fn order_follows_the_sources() {
        let hints = Hints {
            reported: Some(0x05),
            declared: vec![0x01, 0x03, 0x04, 0x11, 0x12, 0x0F, 0x10],
            same_brand: vec![0x06, 0x05],
        };
        let c = ordered(Input::Hdmi, &hints, &[0x0F]);
        assert_eq!(&codes(&c)[..7], &[0x05, 0x11, 0x12, 0x01, 0x03, 0x04, 0x10]);
        assert_eq!(c[0].source, Source::Reported);
        assert_eq!(c[1].source, Source::Mccs);
        assert_eq!(c[3].source, Source::Capabilities);
        // 0x05 ya salió como lectura; 0x06 queda como de la marca.
        assert_eq!(
            c[7],
            Candidate {
                code: 0x06,
                source: Source::SameBrand
            }
        );
        assert_eq!(
            c[8],
            Candidate {
                code: 0x02,
                source: Source::Sweep
            }
        );
        assert!(!codes(&c).contains(&0x0F));
        // Todo el rango del barrido aparece una sola vez, menos lo excluido.
        assert_eq!(c.len(), SWEEP.count() - 1);
    }

    #[test]
    fn without_hints_mccs_comes_first() {
        let c = ordered(Input::DisplayPort, &Hints::default(), &[]);
        assert_eq!(&codes(&c)[..3], &[0x0F, 0x10, 0x01]);
        assert_eq!(c.len(), SWEEP.count());
    }

    #[test]
    fn reported_zero_and_out_of_range_codes() {
        let hints = Hints {
            reported: Some(0),
            declared: vec![0x2A],
            same_brand: vec![],
        };
        let c = ordered(Input::Hdmi, &hints, &[]);
        assert_eq!(&codes(&c)[..3], &[0x11, 0x12, 0x2A]);
        assert!(!codes(&c).contains(&0));
    }

    #[test]
    fn brand_codes_only_from_the_same_manufacturer() {
        let p = |m: &str, hdmi: u8| BrandProfile {
            manufacturer: m.into(),
            codes: InputCodes {
                hdmi: vec![hdmi],
                display_port: vec![0x0F],
            },
        };
        let base = [p("SAM", 0x06), p("DEL", 0x11), p("sam", 0x05)];
        assert_eq!(brand_codes(&base, Some("SAM"), Input::Hdmi), [0x06, 0x05]);
        assert_eq!(
            brand_codes(&base, Some("SAM"), Input::DisplayPort),
            [0x0F, 0x0F]
        );
        assert!(brand_codes(&base, None, Input::Hdmi).is_empty());
        assert!(brand_codes(&base, Some("GSM"), Input::Hdmi).is_empty());
    }

    proptest! {
        /// Sin duplicados, sin excluidos, cubre todo el barrido y respeta el orden de fuentes.
        #[test]
        fn ordered_is_a_clean_priority_list(
            reported in proptest::option::of(any::<u8>()),
            declared in proptest::collection::vec(any::<u8>(), 0..8),
            same_brand in proptest::collection::vec(any::<u8>(), 0..6),
            exclude in proptest::collection::vec(any::<u8>(), 0..3),
            hdmi in any::<bool>(),
        ) {
            let input = if hdmi { Input::Hdmi } else { Input::DisplayPort };
            let hints = Hints { reported, declared, same_brand };
            let c = ordered(input, &hints, &exclude);
            let all = codes(&c);
            let mut dedup = all.clone();
            dedup.sort_unstable();
            dedup.dedup();
            prop_assert_eq!(dedup.len(), all.len());
            prop_assert!(all.iter().all(|x| *x != 0 && !exclude.contains(x)));
            for code in SWEEP.filter(|x| !exclude.contains(x)) {
                prop_assert!(all.contains(&code));
            }
            prop_assert!(c.windows(2).all(|w| w[0].source <= w[1].source));
            if let Some(r) = reported.filter(|r| *r != 0 && !exclude.contains(r)) {
                prop_assert_eq!(c[0], Candidate { code: r, source: Source::Reported });
            }
        }
    }
}
