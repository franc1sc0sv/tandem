//! La lista única de lo que parece una credencial (T-130, D2: vale en los dos sentidos).
//!
//! La usan archivos (`Source`) y workspaces (el parche). Se rechaza por omisión, aunque
//! no esté en `.gitignore`; la única salida es nombrar el archivo concreto en `include`.
//! Las rutas son las del cable: relativas, con `/`.

use std::fs;
use std::io::Read;
use std::path::Path;

/// Nombres de carpeta que contienen credenciales: todo lo de adentro es secreto.
const SECRET_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg", ".kube", ".azure"];

/// Pares de carpetas consecutivas (`.config/gh`).
const SECRET_DIR_PAIRS: &[(&str, &str)] = &[(".config", "gh"), (".config", "gcloud")];

/// Nombres exactos de archivo (sin distinguir mayúsculas).
const SECRET_NAMES: &[&str] = &[
    ".env",
    ".git-credentials",
    ".netrc",
    "_netrc",
    ".pgpass",
    ".htpasswd",
    ".dockercfg",
    ".boto",
    "credentials.json",
    "service-account.json",
    "client_secret.json",
];

/// Extensiones de archivo (sin el punto).
const SECRET_EXTS: &[&str] = &["pem", "key", "p12", "pfx", "jks", "keystore", "ppk", "kdbx"];

/// Prefijos de nombre: `id_rsa`, `id_rsa.pub`, `id_ed25519_work`…
const SECRET_PREFIXES: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"];

/// `.env.*` que son plantillas sin valores reales.
const ENV_TEMPLATES: &[&str] = &["example", "sample", "template", "dist", "defaults"];

/// Archivos que solo son secretos si traen un token o una contraseña.
const TOKEN_FILES: &[&str] = &[".npmrc", ".pypirc", ".yarnrc", ".yarnrc.yml", "pip.conf"];

/// Cuánto de un `.npmrc` y similares se lee buscando un token.
const TOKEN_SCAN_LIMIT: u64 = 64 * 1024;

/// Qué es una ruta según su nombre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Clean,
    Secret,
    /// Depende del contenido.
    IfToken,
}

fn classify(rel: &str) -> Class {
    let parts: Vec<String> = rel
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .map(str::to_ascii_lowercase)
        .collect();
    let Some(name) = parts.last() else {
        return Class::Clean;
    };
    // Una carpeta secreta en cualquier tramo, el último incluido (`.ssh` mismo).
    if parts.iter().any(|d| SECRET_DIRS.contains(&d.as_str())) {
        return Class::Secret;
    }
    if parts.windows(2).any(|w| {
        SECRET_DIR_PAIRS
            .iter()
            .any(|(a, b)| w[0] == *a && w[1] == *b)
    }) {
        return Class::Secret;
    }
    if SECRET_NAMES.contains(&name.as_str()) {
        return Class::Secret;
    }
    if let Some((stem, ext)) = name.rsplit_once('.')
        && !stem.is_empty()
        && SECRET_EXTS.contains(&ext)
    {
        return Class::Secret;
    }
    if let Some(suffix) = name.strip_prefix(".env.") {
        return if ENV_TEMPLATES.contains(&suffix) {
            Class::Clean
        } else {
            Class::Secret
        };
    }
    if SECRET_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return Class::Secret;
    }
    if TOKEN_FILES.contains(&name.as_str()) {
        return Class::IfToken;
    }
    Class::Clean
}

/// ¿El contenido de un `.npmrc`/`.pypirc` trae un token o una contraseña?
pub fn has_token(content: &[u8]) -> bool {
    let text = String::from_utf8_lossy(content).to_ascii_lowercase();
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#') && !l.starts_with(';'))
        .any(|l| {
            [
                "authtoken",
                "_auth",
                "password",
                "token",
                "secret",
                "apikey",
            ]
            .iter()
            .any(|k| l.contains(k))
        })
}

/// ¿Esta ruta del cable parece secreta por su nombre? (Los `.npmrc` y similares
/// dependen del contenido: usa [`SecretFilter::blocks`].)
pub fn looks_secret(rel: &str) -> bool {
    classify(rel) == Class::Secret
}

/// El filtro con las excepciones explícitas de una llamada.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretFilter {
    include: Vec<String>,
}

fn norm(path: &str) -> String {
    path.replace('\\', "/")
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect::<Vec<_>>()
        .join("/")
}

impl SecretFilter {
    /// `include`: rutas exactas (relativas a lo que se copia) que sí se mandan.
    pub fn new(include: &[String]) -> Self {
        SecretFilter {
            include: include.iter().map(|p| norm(p)).collect(),
        }
    }

    /// ¿Se omite `rel`? `abs` es dónde está en disco, para mirar el contenido de los
    /// archivos que solo son secretos con token.
    pub fn blocks(&self, rel: &str, abs: &Path) -> bool {
        let blocked = match classify(rel) {
            Class::Clean => false,
            Class::Secret => true,
            Class::IfToken => read_head(abs).is_some_and(|c| has_token(&c)),
        };
        blocked && !self.include.contains(&norm(rel))
    }
}

impl SecretFilter {
    /// ¿Lo que se copia está dentro de una carpeta secreta (`~/.ssh`, `~/.aws`…)? Para
    /// una carpeta no hay excepción; un archivo suelto se salva nombrándolo en `include`.
    pub fn root_blocked(&self, local: &Path, is_dir: bool) -> bool {
        let full = local.to_string_lossy().replace('\\', "/");
        if !looks_secret(&full) {
            return false;
        }
        if is_dir {
            return true;
        }
        let name = local.file_name().map(|n| n.to_string_lossy().into_owned());
        !name.is_some_and(|n| self.include.contains(&norm(&n)))
    }
}

fn read_head(path: &Path) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(TOKEN_SCAN_LIMIT)
        .read_to_end(&mut buf)
        .ok()?;
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn nombres_que_son_secretos() {
        for p in [
            ".env",
            "app/.env",
            ".env.local",
            ".env.production",
            "certs/server.pem",
            "a/b/tls.KEY",
            "id_rsa",
            "home/.ssh/id_ed25519",
            "x/id_rsa.pub",
            ".ssh/config",
            ".ssh/known_hosts",
            "h/.aws/credentials",
            ".config/gh/hosts.yml",
            ".git-credentials",
            ".netrc",
            "deploy/credentials.json",
            "keys/store.p12",
        ] {
            assert!(looks_secret(p), "{p}");
        }
    }

    #[test]
    fn nombres_que_no_lo_son() {
        for p in [
            "src/main.rs",
            ".env.example",
            ".env.sample",
            "README.md",
            "keyboard.rs",
            "monkey.txt",
            "docs/ssh.md",
            ".config/other/x",
            "environment.ts",
            "key",
            ".key",
        ] {
            assert!(!looks_secret(p), "{p}");
        }
    }

    #[test]
    fn el_token_decide_en_npmrc() {
        assert!(has_token(b"//registry.npmjs.org/:_authToken=abc123\n"));
        assert!(has_token(b"[pypi]\nusername = u\npassword = p\n"));
        assert!(!has_token(b"registry=https://registry.npmjs.org/\n"));
        assert!(!has_token(b"# _authToken=comentado\nsave-exact=true\n"));
    }

    #[test]
    fn npmrc_con_y_sin_token_en_disco() {
        let d = tempfile::tempdir().unwrap();
        let con = d.path().join(".npmrc");
        fs::write(&con, "//r/:_authToken=zzz\n").unwrap();
        let sin = d.path().join("sub");
        fs::create_dir(&sin).unwrap();
        fs::write(sin.join(".npmrc"), "registry=https://r/\n").unwrap();
        let f = SecretFilter::default();
        assert!(f.blocks(".npmrc", &con));
        assert!(!f.blocks("sub/.npmrc", &sin.join(".npmrc")));
    }

    #[test]
    fn include_nombra_un_archivo_concreto() {
        let f = SecretFilter::new(&["./app/.env".into()]);
        let nada = Path::new("/nonexistent");
        assert!(!f.blocks("app/.env", nada));
        assert!(f.blocks(".env", nada));
        assert!(f.blocks("otra/.env", nada));
    }

    proptest! {
        // Una carpeta secreta en cualquier lugar del camino hace secreto lo de adentro.
        #[test]
        fn lo_de_adentro_de_una_carpeta_secreta_es_secreto(
            pre in "[a-z]{1,6}(/[a-z]{1,6}){0,2}",
            dir in prop::sample::select(vec![".ssh", ".aws", ".gnupg", ".kube"]),
            name in "[a-z]{1,8}",
        ) {
            let rel = [pre.as_str(), dir, name.as_str()].join("/");
            prop_assert!(looks_secret(&rel));
        }

        // Un nombre sin ninguna marca nunca se rechaza.
        #[test]
        fn lo_corriente_pasa(path in "[a-z]{1,6}(/[a-z]{1,6}){0,3}\\.(rs|md|txt|toml)") {
            prop_assert!(!looks_secret(&path));
        }

        // Con `include` exacto, nada de lo nombrado se bloquea.
        #[test]
        fn include_siempre_gana(pre in "[a-z]{1,6}", name in "[a-z]{1,6}") {
            let rel = format!("{pre}/.ssh/{name}");
            let f = SecretFilter::new(std::slice::from_ref(&rel));
            prop_assert!(!f.blocks(&rel, Path::new("/nonexistent")));
        }
    }
}
