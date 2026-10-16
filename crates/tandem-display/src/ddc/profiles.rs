//! Base de perfiles DDC: qué código de `0x60` obedece cada monitor para cada entrada.
//!
//! Los perfiles verificados del repo (`profiles/ddc/<PNP>.toml`) van embebidos en el
//! binario. El usuario puede agregar los suyos en `<datos>/profiles/ddc/`; con el mismo id
//! PnP, el del usuario gana. Formato y cómo aportar uno: `docs/dev/ddc-profiles.md`.
//!
//! Se busca por id PnP: es lo único que identifica al modelo (el nombre del EDID lo
//! comparten a veces varios modelos, como "Odyssey G5"). El nombre solo sirve de respaldo
//! cuando no es genérico.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use tandem_core::model::Input;

use super::MonitorInfo;
use super::switch::InputCodes;

macro_rules! builtin {
    ($($file:literal),* $(,)?) => {
        &[$(($file, include_str!(concat!("../../../../profiles/ddc/", $file)))),*]
    };
}

/// Perfiles del repo: (nombre del archivo en `profiles/ddc/`, contenido).
pub const BUILTIN: &[(&str, &str)] = builtin!("SAM711A.toml", "SAM7489.toml");

/// Perfil de un modelo de monitor, tal como se guarda en `<PNP>.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DdcProfile {
    /// Fabricante (tres letras) y producto (cuatro dígitos hexadecimales): `SAM7489`.
    pub pnp_id: String,
    /// Nombre para mostrar.
    pub name: String,
    /// Nombres del descriptor 0xFC del EDID con los que se anuncia este modelo.
    #[serde(default)]
    pub edid_names: Vec<String>,
    /// Los nombres los comparten otros modelos: el perfil solo se aplica por id PnP.
    #[serde(default)]
    pub generic_name: bool,
    pub codes: ProfileCodes,
    /// Lo que devuelve la lectura de `0x60` estando en cada entrada (la pista de T-050:
    /// varios monitores reportan el código de la entrada en la que están).
    #[serde(default)]
    pub reported: ReportedCodes,
    pub notes: Option<String>,
    pub verified_by: Option<String>,
    /// Fecha de la verificación, `AAAA-MM-DD`.
    pub verified_on: Option<String>,
}

/// Un código por entrada, probado en el equipo.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileCodes {
    pub hdmi: u8,
    pub display_port: u8,
    pub hdmi2: Option<u8>,
    pub display_port2: Option<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportedCodes {
    pub hdmi: Option<u8>,
    pub display_port: Option<u8>,
    pub hdmi2: Option<u8>,
    pub display_port2: Option<u8>,
}

impl ReportedCodes {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    #[error("TOML inválido: {0}")]
    Toml(String),
    #[error(
        "id PnP inválido `{0}`: son tres letras mayúsculas y cuatro dígitos hexadecimales (SAM7489)"
    )]
    BadPnpId(String),
    #[error("`name` no puede estar vacío")]
    EmptyName,
    #[error("`edid_names` no puede tener nombres vacíos")]
    EmptyEdidName,
    #[error("el código de `{0}` no puede ser 0x00")]
    ZeroCode(&'static str),
    #[error("`verified_on` debe ser una fecha AAAA-MM-DD, no `{0}`")]
    BadDate(String),
}

impl DdcProfile {
    pub fn parse(text: &str) -> Result<Self, ProfileError> {
        let profile: DdcProfile =
            toml::from_str(text).map_err(|e| ProfileError::Toml(e.message().to_owned()))?;
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<(), ProfileError> {
        if !valid_pnp_id(&self.pnp_id) {
            return Err(ProfileError::BadPnpId(self.pnp_id.clone()));
        }
        if self.name.trim().is_empty() {
            return Err(ProfileError::EmptyName);
        }
        if self.edid_names.iter().any(|n| n.trim().is_empty()) {
            return Err(ProfileError::EmptyEdidName);
        }
        let c = &self.codes;
        let codes = [
            ("codes.hdmi", Some(c.hdmi)),
            ("codes.display_port", Some(c.display_port)),
            ("codes.hdmi2", c.hdmi2),
            ("codes.display_port2", c.display_port2),
        ];
        if let Some((field, _)) = codes.iter().find(|(_, code)| *code == Some(0)) {
            return Err(ProfileError::ZeroCode(field));
        }
        if let Some(date) = &self.verified_on
            && !valid_date(date)
        {
            return Err(ProfileError::BadDate(date.clone()));
        }
        Ok(())
    }

    /// Los códigos que usa el cambio de entrada (`HDMI-1` y `DP-1`).
    pub fn input_codes(&self) -> InputCodes {
        InputCodes {
            hdmi: vec![self.codes.hdmi],
            display_port: vec![self.codes.display_port],
        }
    }

    pub fn code_for(&self, input: Input) -> u8 {
        match input {
            Input::Hdmi => self.codes.hdmi,
            Input::DisplayPort => self.codes.display_port,
        }
    }

    /// Las tres letras del fabricante (`SAM`): los modelos de una marca suelen compartir
    /// códigos, y la calibración los prueba antes del barrido.
    pub fn manufacturer(&self) -> &str {
        self.pnp_id.get(..3).unwrap_or(&self.pnp_id)
    }

    /// El perfil como `<PNP>.toml`, para guardarlo o compartirlo. `parse` lo lee igual.
    pub fn to_toml(&self) -> String {
        let mut out = String::from("# Perfil DDC de Tandem. Formato: docs/dev/ddc-profiles.md\n");
        let mut line = |key: &str, value: String| out.push_str(&format!("{key} = {value}\n"));
        line("pnp_id", quote(&self.pnp_id));
        line("name", quote(&self.name));
        let names: Vec<String> = self.edid_names.iter().map(|n| quote(n)).collect();
        line("edid_names", format!("[{}]", names.join(", ")));
        if self.generic_name {
            line("generic_name", "true".into());
        }
        for (key, value) in [
            ("notes", &self.notes),
            ("verified_by", &self.verified_by),
            ("verified_on", &self.verified_on),
        ] {
            if let Some(v) = value {
                line(key, quote(v));
            }
        }
        let table = |out: &mut String, name: &str, codes: [(&str, Option<u8>); 4]| {
            out.push_str(&format!("\n[{name}]\n"));
            for (key, code) in codes {
                if let Some(code) = code {
                    out.push_str(&format!("{key} = {code:#04X}\n"));
                }
            }
        };
        let c = &self.codes;
        table(
            &mut out,
            "codes",
            [
                ("hdmi", Some(c.hdmi)),
                ("display_port", Some(c.display_port)),
                ("hdmi2", c.hdmi2),
                ("display_port2", c.display_port2),
            ],
        );
        let r = &self.reported;
        if !r.is_empty() {
            table(
                &mut out,
                "reported",
                [
                    ("hdmi", r.hdmi),
                    ("display_port", r.display_port),
                    ("hdmi2", r.hdmi2),
                    ("display_port2", r.display_port2),
                ],
            );
        }
        out
    }
}

/// Cadena básica de TOML con sus escapes (las comillas y los saltos de línea de las notas).
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Hexadecimal solo en mayúsculas: `get()` y `Edid::pnp_id()` comparan contra
/// `{:04X}`, así que una clave con minúsculas nunca coincidiría sin avisar.
fn valid_pnp_id(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() == 7
        && b[..3].iter().all(u8::is_ascii_uppercase)
        && b[3..]
            .iter()
            .all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(c))
}

fn valid_date(date: &str) -> bool {
    let b = date.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// De dónde sale un perfil.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileSource {
    /// Embebido: nombre del archivo en `profiles/ddc/` del repo.
    Builtin(&'static str),
    User(PathBuf),
}

impl fmt::Display for ProfileSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileSource::Builtin(file) => write!(f, "base del repo (profiles/ddc/{file})"),
            ProfileSource::User(path) => write!(f, "perfil del usuario ({})", path.display()),
        }
    }
}

/// Un archivo de perfil que no se pudo usar. Los demás se cargan igual.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{source_name}: {kind}")]
pub struct ProfileFileError {
    /// El archivo (o el directorio, si no se pudo listar).
    pub source_name: String,
    pub kind: ProfileFileErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileFileErrorKind {
    #[error("no se pudo leer: {0}")]
    Io(String),
    #[error(transparent)]
    Invalid(#[from] ProfileError),
    #[error("el id PnP {pnp_id} ya lo define {first}; se ignora este archivo")]
    Duplicate { pnp_id: String, first: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub profile: DdcProfile,
    pub source: ProfileSource,
    /// El perfil del repo que este (del usuario) reemplaza.
    pub overrides: Option<ProfileSource>,
}

/// Cómo se encontró el perfil de un monitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchedBy {
    PnpId,
    EdidName(String),
}

impl fmt::Display for MatchedBy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MatchedBy::PnpId => write!(f, "id PnP"),
            MatchedBy::EdidName(name) => write!(f, "nombre del EDID `{name}`"),
        }
    }
}

/// Perfiles cargados, uno por id PnP, más los archivos que fallaron.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileDb {
    by_pnp: BTreeMap<String, Entry>,
    errors: Vec<ProfileFileError>,
}

/// `<datos>/profiles/ddc/`: los perfiles del usuario.
pub fn user_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("profiles").join("ddc")
}

impl ProfileDb {
    /// Solo los perfiles del repo. Se cargan una vez.
    pub fn builtin() -> &'static ProfileDb {
        static DB: OnceLock<ProfileDb> = OnceLock::new();
        DB.get_or_init(|| Self::from_builtin(BUILTIN))
    }

    fn from_builtin(files: &[(&'static str, &str)]) -> Self {
        let mut db = ProfileDb::default();
        for (file, text) in files {
            let source = ProfileSource::Builtin(file);
            match DdcProfile::parse(text) {
                Ok(profile) => db.insert(profile, source),
                Err(e) => db.errors.push(ProfileFileError {
                    source_name: source.to_string(),
                    kind: e.into(),
                }),
            }
        }
        db
    }

    /// Los del repo más los de `user_dir` (si existe), que ganan con el mismo id PnP.
    pub fn load(user_dir: Option<&Path>) -> Self {
        let mut db = Self::builtin().clone();
        if let Some(dir) = user_dir {
            db.load_user_dir(dir);
        }
        db
    }

    fn load_user_dir(&mut self, dir: &Path) {
        let listing = match std::fs::read_dir(dir) {
            Ok(listing) => listing,
            // Sin directorio no hay perfiles del usuario; no es un error.
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => {
                self.errors.push(ProfileFileError {
                    source_name: dir.display().to_string(),
                    kind: ProfileFileErrorKind::Io(e.to_string()),
                });
                return;
            }
        };
        let mut files: Vec<PathBuf> = listing
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("toml"))
                    && !p.is_dir()
            })
            .collect();
        // Orden fijo: con dos archivos del mismo id, siempre gana el mismo.
        files.sort();
        for path in files {
            self.load_user_file(path);
        }
    }

    fn load_user_file(&mut self, path: PathBuf) {
        let source_name = path.display().to_string();
        let profile = match std::fs::read_to_string(&path) {
            Ok(text) => DdcProfile::parse(&text).map_err(ProfileFileErrorKind::from),
            Err(e) => Err(ProfileFileErrorKind::Io(e.to_string())),
        };
        let profile = match profile {
            Ok(p) => p,
            Err(kind) => {
                self.errors.push(ProfileFileError { source_name, kind });
                return;
            }
        };
        if let Some(prev) = self.by_pnp.get(&profile.pnp_id)
            && matches!(prev.source, ProfileSource::User(_))
        {
            self.errors.push(ProfileFileError {
                source_name,
                kind: ProfileFileErrorKind::Duplicate {
                    pnp_id: profile.pnp_id,
                    first: prev.source.to_string(),
                },
            });
            return;
        }
        self.insert(profile, ProfileSource::User(path));
    }

    fn insert(&mut self, profile: DdcProfile, source: ProfileSource) {
        let overrides = self
            .by_pnp
            .remove(&profile.pnp_id)
            .map(|prev| prev.overrides.unwrap_or(prev.source));
        self.by_pnp.insert(
            profile.pnp_id.clone(),
            Entry {
                profile,
                source,
                overrides,
            },
        );
    }

    /// Archivos que no se pudieron usar (para avisar al usuario).
    pub fn errors(&self) -> &[ProfileFileError] {
        &self.errors
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.by_pnp.values()
    }

    pub fn get(&self, pnp_id: &str) -> Option<&Entry> {
        self.by_pnp.get(&pnp_id.to_ascii_uppercase())
    }

    /// Perfiles de un fabricante (`SAM`), para probar sus códigos en monitores nuevos.
    pub fn by_manufacturer<'a>(&'a self, manufacturer: &'a str) -> impl Iterator<Item = &'a Entry> {
        self.entries()
            .filter(move |e| e.profile.manufacturer().eq_ignore_ascii_case(manufacturer))
    }

    /// El perfil de un monitor: por id PnP, o por el nombre del EDID si ese nombre
    /// identifica a un solo modelo de la base.
    pub fn find(&self, info: &MonitorInfo) -> Option<(&Entry, MatchedBy)> {
        let edid = info.edid.as_ref()?;
        if let Some(entry) = self.get(&edid.pnp_id()) {
            return Some((entry, MatchedBy::PnpId));
        }
        let name = edid.name.as_deref()?.trim();
        let mut found = self.entries().filter(|e| {
            !e.profile.generic_name
                && e.profile
                    .edid_names
                    .iter()
                    .any(|n| n.trim().eq_ignore_ascii_case(name))
        });
        let first = found.next()?;
        // Un nombre en dos modelos es genérico aunque nadie lo haya marcado.
        if found.next().is_some() {
            return None;
        }
        Some((first, MatchedBy::EdidName(name.to_owned())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ddc::edid::{self, Edid};
    use proptest::prelude::*;

    fn info(pnp: (&str, u16), name: &str) -> MonitorInfo {
        let mut e: Edid = edid::parse(&edid::tests::sample(name, "S")).unwrap();
        e.manufacturer = pnp.0.into();
        e.product_code = pnp.1;
        MonitorInfo {
            index: 0,
            description: String::new(),
            device: String::new(),
            edid: Some(e),
        }
    }

    fn profile(pnp: &str, hdmi: u8, dp: u8) -> String {
        format!(
            "pnp_id = \"{pnp}\"\nname = \"Prueba {pnp}\"\n[codes]\nhdmi = {hdmi:#04X}\ndisplay_port = {dp:#04X}\n"
        )
    }

    fn user_db(files: &[(&str, &str)]) -> (tempfile::TempDir, ProfileDb) {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            std::fs::write(dir.path().join(name), text).unwrap();
        }
        let db = ProfileDb::load(Some(dir.path()));
        (dir, db)
    }

    #[test]
    fn every_repo_file_is_embedded_and_valid() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles/ddc");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        on_disk.sort();
        let mut embedded: Vec<String> = BUILTIN.iter().map(|(f, _)| f.to_string()).collect();
        embedded.sort();
        assert_eq!(on_disk, embedded, "profiles/ddc/ y BUILTIN deben coincidir");
        let db = ProfileDb::builtin();
        assert!(db.errors().is_empty(), "{:?}", db.errors());
        for (file, _) in BUILTIN {
            let pnp = file.trim_end_matches(".toml");
            let entry = db
                .get(pnp)
                .unwrap_or_else(|| panic!("{file}: pnp_id ≠ nombre"));
            assert_eq!(entry.source, ProfileSource::Builtin(file));
        }
    }

    #[test]
    fn verified_profiles_from_t050() {
        let db = ProfileDb::builtin();
        let big = &db.get("SAM711A").unwrap().profile;
        assert_eq!((big.codes.hdmi, big.codes.display_port), (0x06, 0x09));
        assert_eq!(big.reported.display_port, Some(0x09));
        let small = &db.get("sam7489").unwrap().profile;
        assert_eq!((small.codes.hdmi, small.codes.display_port), (0x05, 0x0F));
        assert_eq!(small.reported.hdmi, Some(0x05));
        assert!(small.generic_name);
        assert_eq!(db.by_manufacturer("sam").count(), 2);
        assert_eq!(db.by_manufacturer("DEL").count(), 0);
    }

    #[test]
    fn finds_by_pnp_then_by_specific_name() {
        let db = ProfileDb::builtin();
        let (e, by) = db.find(&info(("SAM", 0x711A), "otro")).unwrap();
        assert_eq!(
            (e.profile.pnp_id.as_str(), by),
            ("SAM711A", MatchedBy::PnpId)
        );
        let (e, by) = db.find(&info(("SAM", 0x7140), "lc34g55t")).unwrap();
        assert_eq!(e.profile.pnp_id, "SAM711A");
        assert_eq!(by, MatchedBy::EdidName("lc34g55t".into()));
        // "Odyssey G5" es genérico: otro modelo con ese nombre no usa el perfil del 32".
        assert!(db.find(&info(("SAM", 0x7400), "Odyssey G5")).is_none());
        let mut no_edid = info(("SAM", 0x711A), "LC34G55T");
        no_edid.edid = None;
        assert!(db.find(&no_edid).is_none());
    }

    #[test]
    fn name_shared_by_two_models_is_not_used() {
        let a = profile("AAA0001", 1, 2).replacen("[codes]", "edid_names = [\"X1\"]\n[codes]", 1);
        let b = profile("BBB0002", 3, 4).replacen("[codes]", "edid_names = [\"x1\"]\n[codes]", 1);
        let (_dir, db) = user_db(&[("AAA0001.toml", &a), ("BBB0002.toml", &b)]);
        assert!(db.errors().is_empty(), "{:?}", db.errors());
        assert!(db.find(&info(("CCC", 3), "X1")).is_none());
        assert!(db.find(&info(("AAA", 1), "X1")).is_some());
    }

    #[test]
    fn user_profile_with_same_pnp_wins_over_repo() {
        let text = profile("SAM7489", 0x07, 0x0F);
        let (dir, db) = user_db(&[("mio.toml", &text)]);
        assert!(db.errors().is_empty(), "{:?}", db.errors());
        let (entry, by) = db.find(&info(("SAM", 0x7489), "Odyssey G5")).unwrap();
        assert_eq!(by, MatchedBy::PnpId);
        assert_eq!(entry.profile.codes.hdmi, 0x07);
        assert_eq!(
            entry.source,
            ProfileSource::User(dir.path().join("mio.toml"))
        );
        assert_eq!(
            entry.overrides,
            Some(ProfileSource::Builtin("SAM7489.toml"))
        );
        // La base del repo no cambió.
        assert_eq!(
            ProfileDb::builtin()
                .get("SAM7489")
                .unwrap()
                .profile
                .codes
                .hdmi,
            0x05
        );
        // El otro perfil del repo sigue ahí.
        assert!(db.get("SAM711A").is_some());
    }

    #[test]
    fn invalid_file_reports_its_name_and_others_still_load() {
        let good = profile("DEL4321", 0x11, 0x0F);
        let (dir, db) = user_db(&[
            ("a-roto.toml", "pnp_id = \"DEL1234\"\n[codes]\nhdmi = 300\n"),
            ("b-bueno.toml", &good),
            ("c-pnp.toml", &profile("dell123", 1, 2)),
            // Minúsculas en el producto: get() y el EDID usan mayúsculas, nunca coincidiría.
            ("cc-pnp.toml", &profile("SAM748a", 1, 2)),
            ("d-desconocido.toml", &(good.clone() + "hdmi3 = 1\n")),
            ("leeme.txt", "no es un perfil"),
        ]);
        let errors = db.errors();
        assert_eq!(errors.len(), 4, "{errors:?}");
        for (err, file) in errors
            .iter()
            .zip(["a-roto", "c-pnp", "cc-pnp", "d-desconocido"])
        {
            let msg = err.to_string();
            assert!(msg.contains(&format!("{file}.toml")), "{msg}");
            assert!(msg.starts_with(&dir.path().display().to_string()), "{msg}");
        }
        for err in &errors[1..3] {
            assert!(matches!(
                err.kind,
                ProfileFileErrorKind::Invalid(ProfileError::BadPnpId(_))
            ));
        }
        assert!(db.get("DEL4321").is_some());
        assert!(db.get("SAM711A").is_some());
    }

    #[test]
    fn second_user_file_with_same_pnp_is_reported() {
        let (dir, db) = user_db(&[
            ("a.toml", &profile("DEL4321", 1, 2)),
            ("b.toml", &profile("DEL4321", 3, 4)),
        ]);
        assert_eq!(db.get("DEL4321").unwrap().profile.codes.hdmi, 1);
        let [err] = db.errors() else {
            panic!("{:?}", db.errors())
        };
        assert!(err.source_name.ends_with("b.toml"));
        assert!(
            err.to_string()
                .contains(&dir.path().join("a.toml").display().to_string())
        );
    }

    #[test]
    fn missing_user_dir_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let db = ProfileDb::load(Some(&dir.path().join("no-existe")));
        assert!(db.errors().is_empty());
        assert_eq!(db, ProfileDb::load(None));
        assert_eq!(
            user_dir(Path::new("d")),
            Path::new("d").join("profiles").join("ddc")
        );
    }

    #[test]
    fn validation_errors_are_clear() {
        let base = profile("ABC0001", 1, 2);
        let cases = [
            (base.replace("hdmi = 0x01", "hdmi = 0x00"), "codes.hdmi"),
            (base.replace("Prueba ABC0001", " "), "`name`"),
            (
                base.replacen("[codes]", "verified_on = \"16/10/2026\"\n[codes]", 1),
                "AAAA-MM-DD",
            ),
            (
                base.replacen("[codes]", "edid_names = [\"\"]\n[codes]", 1),
                "edid_names",
            ),
            (base.replace("display_port = 0x02\n", ""), "display_port"),
            ("pnp_id = ".into(), "TOML inválido"),
        ];
        for (text, expected) in cases {
            let err = DdcProfile::parse(&text).unwrap_err().to_string();
            assert!(err.contains(expected), "{text}\n→ {err}");
        }
    }

    #[test]
    fn export_of_repo_profiles_round_trips() {
        for entry in ProfileDb::builtin().entries() {
            let text = entry.profile.to_toml();
            assert_eq!(DdcProfile::parse(&text).unwrap(), entry.profile, "{text}");
        }
        let big = &ProfileDb::builtin().get("SAM711A").unwrap().profile;
        assert!(big.to_toml().contains("hdmi = 0x06\n"));
    }

    fn arb_code() -> impl Strategy<Value = u8> {
        1u8..=255
    }

    prop_compose! {
        fn arb_profile()(
            mfr in "[A-Z]{3}",
            product in any::<u16>(),
            name in "\\PC{1,20}".prop_filter("no vacío", |s| !s.trim().is_empty()),
            edid_names in proptest::collection::vec(
                "[ -~]{1,13}".prop_filter("no vacío", |s| !s.trim().is_empty()), 0..3),
            generic_name in any::<bool>(),
            codes in (arb_code(), arb_code(), proptest::option::of(arb_code()), proptest::option::of(arb_code())),
            reported in proptest::collection::vec(proptest::option::of(any::<u8>()), 4),
            notes in proptest::option::of("\\PC{0,40}|[\"\\\\\n\t]{1,5}"),
            verified_by in proptest::option::of("\\PC{0,20}"),
            date in proptest::option::of((1990u32..2100, 1u32..13, 1u32..29)),
        ) -> DdcProfile {
            DdcProfile {
                pnp_id: format!("{mfr}{product:04X}"),
                name,
                edid_names,
                generic_name,
                codes: ProfileCodes { hdmi: codes.0, display_port: codes.1, hdmi2: codes.2, display_port2: codes.3 },
                reported: ReportedCodes {
                    hdmi: reported[0], display_port: reported[1], hdmi2: reported[2], display_port2: reported[3],
                },
                notes,
                verified_by,
                verified_on: date.map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}")),
            }
        }
    }

    proptest! {
        /// Lo que se exporta se vuelve a leer igual: un perfil compartido no pierde nada.
        #[test]
        fn export_round_trips(p in arb_profile()) {
            prop_assert_eq!(p.validate(), Ok(()));
            let text = p.to_toml();
            prop_assert_eq!(DdcProfile::parse(&text), Ok(p), "{}", text);
        }
    }
}
