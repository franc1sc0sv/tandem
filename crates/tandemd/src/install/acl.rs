//! La política de la ACL de la tarea elevada, sin tocar el sistema: dado el propietario y los
//! ACE de una ruta, dice si solo la pueden escribir administradores. La lectura real de la
//! DACL vive en `windows.rs`; aquí se prueba en cualquier plataforma.

/// SYSTEM, Administrators y NT SERVICE\TrustedInstaller: lo único que puede escribir o ser dueño.
pub const TRUSTED_SIDS: [&str; 3] = [
    "S-1-5-18",
    "S-1-5-32-544",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
];

const FILE_WRITE_DATA: u32 = 0x2; // y FILE_ADD_FILE en carpetas
const FILE_APPEND_DATA: u32 = 0x4; // y FILE_ADD_SUBDIRECTORY
const FILE_DELETE_CHILD: u32 = 0x40;
const DELETE: u32 = 0x1_0000;
const WRITE_DAC: u32 = 0x4_0000;
const WRITE_OWNER: u32 = 0x8_0000;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

/// Cualquiera de estos permisos permite sustituir lo que hay.
pub const WRITE_MASK: u32 = FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_DELETE_CHILD
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER
    | GENERIC_ALL
    | GENERIC_WRITE;

/// Para los ancestros estrictos basta con lo que permite reemplazar o borrar un hijo que ya
/// existe: crear archivos o subcarpetas no sustituye nada (`C:\` trae de fábrica
/// `Authenticated Users:(AD)`).
pub const ANCESTOR_MASK: u32 =
    FILE_DELETE_CHILD | DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_ALL | GENERIC_WRITE;

/// Cuánto rigor se aplica a una ruta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rigor {
    /// El ejecutable y su carpeta contenedora: crear un archivo ahí planta una DLL.
    Full,
    /// Carpetas más arriba en la ruta.
    Ancestor,
}

impl Rigor {
    pub fn mask(self) -> u32 {
        match self {
            Rigor::Full => WRITE_MASK,
            Rigor::Ancestor => ANCESTOR_MASK,
        }
    }
}

const ACCESS_ALLOWED: u8 = 0;
const ACCESS_ALLOWED_OBJECT: u8 = 5;
const ACCESS_ALLOWED_CALLBACK: u8 = 9;
const ACCESS_ALLOWED_CALLBACK_OBJECT: u8 = 11;
const INHERIT_ONLY_ACE: u8 = 0x08;

/// Un ACE de permiso de la DACL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ace {
    pub sid: String,
    pub mask: u32,
    /// Solo se hereda a los hijos: no aplica a este objeto.
    pub inherit_only: bool,
}

/// Lo que aporta un ACE crudo de la DACL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawAce {
    /// Un ACE de permiso a tener en cuenta.
    Allow(Ace),
    /// Denegación, auditoría, etiqueta…: no concede nada, no cuenta.
    Ignored,
}

/// Clasifica un ACE por su tipo y banderas. Los de permiso con objeto no existen en archivos
/// y no se interpretan: `Err` (se cuenta como inseguro).
pub fn classify(ace_type: u8, flags: u8, mask: u32, sid: String) -> Result<RawAce, String> {
    match ace_type {
        ACCESS_ALLOWED | ACCESS_ALLOWED_CALLBACK => Ok(RawAce::Allow(Ace {
            sid,
            mask,
            inherit_only: flags & INHERIT_ONLY_ACE != 0,
        })),
        ACCESS_ALLOWED_OBJECT | ACCESS_ALLOWED_CALLBACK_OBJECT => {
            Err(format!("ACE de permiso con objeto (tipo {ace_type})"))
        }
        _ => Ok(RawAce::Ignored),
    }
}

/// Lista blanca: el dueño y todo SID con escritura (según `rigor`) tienen que ser de confianza.
/// `Err` trae el motivo.
pub fn verdict(owner: &str, aces: &[Ace], rigor: Rigor) -> Result<(), String> {
    let trusted = |sid: &str| TRUSTED_SIDS.contains(&sid);
    // El dueño tiene WRITE_DAC implícito aunque la DACL no lo diga.
    if !trusted(owner) {
        return Err(format!("el propietario es {owner}"));
    }
    for ace in aces {
        if !ace.inherit_only && ace.mask & rigor.mask() != 0 && !trusted(&ace.sid) {
            return Err(format!(
                "{} puede escribir (máscara {:#x})",
                ace.sid, ace.mask
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const ADMINS: &str = "S-1-5-32-544";
    const USER: &str = "S-1-5-21-1-2-3-1001";
    const READ: u32 = 0x12_00A9; // lectura y ejecución

    fn ace(sid: &str, mask: u32) -> Ace {
        Ace {
            sid: sid.into(),
            mask,
            inherit_only: false,
        }
    }

    #[test]
    fn solo_confiables_pasan() {
        let aces = [
            ace("S-1-5-18", GENERIC_ALL),
            ace(ADMINS, 0x1F_01FF),
            ace(TRUSTED_SIDS[2], GENERIC_ALL),
            ace("S-1-5-32-545", READ),
        ];
        assert_eq!(verdict(ADMINS, &aces, Rigor::Full), Ok(()));
    }

    #[test]
    fn dueno_ajeno_se_rechaza_con_dacl_limpia() {
        let aces = [ace(ADMINS, GENERIC_ALL), ace(USER, READ)];
        assert!(
            verdict(USER, &aces, Rigor::Full)
                .unwrap_err()
                .contains("propietario")
        );
    }

    #[test]
    fn cualquier_permiso_de_escritura_de_un_sid_no_listado_se_rechaza() {
        for sid in ["S-1-5-4", "S-1-5-32-547", USER, "S-1-3-0", "S-1-1-0"] {
            for bit in [
                FILE_WRITE_DATA,
                FILE_APPEND_DATA,
                FILE_DELETE_CHILD,
                DELETE,
                WRITE_DAC,
                WRITE_OWNER,
                GENERIC_WRITE,
                GENERIC_ALL,
            ] {
                let aces = [ace(ADMINS, GENERIC_ALL), ace(sid, READ | bit)];
                assert!(
                    verdict(ADMINS, &aces, Rigor::Full).is_err(),
                    "{sid} {bit:#x}"
                );
            }
        }
    }

    #[test]
    fn solo_lectura_de_otros_sids_pasa() {
        assert_eq!(verdict(ADMINS, &[ace(USER, READ)], Rigor::Full), Ok(()));
    }

    #[test]
    fn el_ace_solo_de_herencia_no_aplica_al_objeto() {
        let heredable = Ace {
            inherit_only: true,
            ..ace("S-1-3-0", GENERIC_ALL)
        };
        assert_eq!(verdict(ADMINS, &[heredable], Rigor::Full), Ok(()));
    }

    #[test]
    fn clasifica_por_tipo_y_banderas() {
        let sid = || USER.to_string();
        assert_eq!(classify(0, 0, 2, sid()), Ok(RawAce::Allow(ace(USER, 2))));
        let Ok(RawAce::Allow(a)) = classify(9, 0x08 | 0x01, 2, sid()) else {
            panic!("callback");
        };
        assert!(a.inherit_only);
        assert_eq!(classify(1, 0, GENERIC_ALL, sid()), Ok(RawAce::Ignored));
        assert_eq!(classify(2, 0, GENERIC_ALL, sid()), Ok(RawAce::Ignored));
        assert!(classify(5, 0, 2, sid()).is_err());
        assert!(classify(11, 0, 2, sid()).is_err());
    }

    #[test]
    fn acl_de_fabrica_de_la_raiz_pasa_como_ancestro_y_falla_como_contenedora() {
        // `icacls C:\\` de un Windows 11: Authenticated Users tiene (OI)(CI)(IO)(M), que no
        // aplica a la raíz, y (AD) sin herencia.
        const AUTH: &str = "S-1-5-11";
        const USERS: &str = "S-1-5-32-545";
        let inherit_only = |sid: &str, mask| Ace {
            inherit_only: true,
            ..ace(sid, mask)
        };
        let aces = [
            ace(ADMINS, 0x1F_01FF),
            ace("S-1-5-18", 0x1F_01FF),
            ace(USERS, READ),
            inherit_only(AUTH, 0x1301BF),
            ace(AUTH, FILE_APPEND_DATA),
        ];
        assert_eq!(
            verdict(
                "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
                &aces,
                Rigor::Ancestor
            ),
            Ok(())
        );
        assert!(
            verdict(ADMINS, &aces, Rigor::Full)
                .unwrap_err()
                .contains(AUTH)
        );
        // Crear subcarpetas no basta como ancestro, pero borrar hijos sí.
        let borra = [ace(AUTH, FILE_DELETE_CHILD)];
        assert!(verdict(ADMINS, &borra, Rigor::Ancestor).is_err());
    }

    proptest! {
        // Con dueño de confianza, el veredicto es exactamente "ningún ACE aplicable de un
        // SID ajeno con escritura".
        #[test]
        fn veredicto_igual_al_modelo(
            raw in proptest::collection::vec((0usize..5, any::<u32>(), any::<bool>()), 0..8)
        ) {
            let sids = [ADMINS, "S-1-5-18", USER, "S-1-5-4", "S-1-1-0"];
            let aces: Vec<Ace> = raw.iter().map(|&(i, mask, inherit_only)| Ace {
                sid: sids[i].into(), mask, inherit_only,
            }).collect();
            let peligroso = aces.iter().any(|a| {
                !a.inherit_only && a.mask & Rigor::Full.mask() != 0 && !TRUSTED_SIDS.contains(&a.sid.as_str())
            });
            prop_assert_eq!(verdict(ADMINS, &aces, Rigor::Full).is_err(), peligroso);
        }
    }
}
