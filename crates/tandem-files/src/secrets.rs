//! The single list of what looks like a credential (T-130, D2: applies in both directions).
//!
//! Used by files (`Source`) and workspaces (the patch). Rejected by default, even if
//! it is not in `.gitignore`; the only way out is naming the specific file in `include`.
//! Paths are the wire ones: relative, with `/`.

use std::fs;
use std::io::Read;
use std::path::Path;

/// Folder names that contain credentials: everything inside is secret.
const SECRET_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg", ".kube", ".azure"];

/// Pairs of consecutive folders (`.config/gh`).
const SECRET_DIR_PAIRS: &[(&str, &str)] = &[(".config", "gh"), (".config", "gcloud")];

/// Exact file names (case-insensitive).
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

/// File extensions (without the dot).
const SECRET_EXTS: &[&str] = &["pem", "key", "p12", "pfx", "jks", "keystore", "ppk", "kdbx"];

/// Name prefixes: `id_rsa`, `id_rsa.pub`, `id_ed25519_work`…
const SECRET_PREFIXES: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"];

/// `.env.*` that are templates without real values.
const ENV_TEMPLATES: &[&str] = &["example", "sample", "template", "dist", "defaults"];

/// Files that are only secret if they carry a token or a password.
const TOKEN_FILES: &[&str] = &[".npmrc", ".pypirc", ".yarnrc", ".yarnrc.yml", "pip.conf"];

/// How much of an `.npmrc` and similar is read looking for a token.
const TOKEN_SCAN_LIMIT: u64 = 64 * 1024;

/// What a path is according to its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Clean,
    Secret,
    /// Depends on the content.
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
    // A secret folder at any stretch, the last one included (`.ssh` itself).
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

/// Does the content of an `.npmrc`/`.pypirc` carry a token or a password?
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

/// Does this wire path look secret by its name? (`.npmrc` files and similar
/// depend on the content: use [`SecretFilter::blocks`].)
pub fn looks_secret(rel: &str) -> bool {
    classify(rel) == Class::Secret
}

/// The filter with the explicit exceptions of a call.
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
    /// `include`: exact paths (relative to what is copied) that are sent.
    pub fn new(include: &[String]) -> Self {
        SecretFilter {
            include: include.iter().map(|p| norm(p)).collect(),
        }
    }

    /// Is `rel` omitted? `abs` is where it is on disk, to look at the contents of
    /// files that are only secret with a token.
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
    /// Is what is copied inside a secret folder (`~/.ssh`, `~/.aws`…)? For
    /// a folder there is no exception; a loose file is saved by naming it in `include`.
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
    fn names_that_are_secrets() {
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
    fn names_that_are_not_secrets() {
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
    fn the_token_decides_in_npmrc() {
        assert!(has_token(b"//registry.npmjs.org/:_authToken=abc123\n"));
        assert!(has_token(b"[pypi]\nusername = u\npassword = p\n"));
        assert!(!has_token(b"registry=https://registry.npmjs.org/\n"));
        assert!(!has_token(b"# _authToken=commented\nsave-exact=true\n"));
    }

    #[test]
    fn npmrc_with_and_without_token_on_disk() {
        let d = tempfile::tempdir().unwrap();
        let with_token = d.path().join(".npmrc");
        fs::write(&with_token, "//r/:_authToken=zzz\n").unwrap();
        let without_token = d.path().join("sub");
        fs::create_dir(&without_token).unwrap();
        fs::write(without_token.join(".npmrc"), "registry=https://r/\n").unwrap();
        let f = SecretFilter::default();
        assert!(f.blocks(".npmrc", &with_token));
        assert!(!f.blocks("sub/.npmrc", &without_token.join(".npmrc")));
    }

    #[test]
    fn include_names_a_specific_file() {
        let f = SecretFilter::new(&["./app/.env".into()]);
        let nothing = Path::new("/nonexistent");
        assert!(!f.blocks("app/.env", nothing));
        assert!(f.blocks(".env", nothing));
        assert!(f.blocks("other/.env", nothing));
    }

    proptest! {
        // A secret folder anywhere along the path makes what is inside secret.
        #[test]
        fn what_is_inside_a_secret_folder_is_secret(
            pre in "[a-z]{1,6}(/[a-z]{1,6}){0,2}",
            dir in prop::sample::select(vec![".ssh", ".aws", ".gnupg", ".kube"]),
            name in "[a-z]{1,8}",
        ) {
            let rel = [pre.as_str(), dir, name.as_str()].join("/");
            prop_assert!(looks_secret(&rel));
        }

        // A name with no mark is never rejected.
        #[test]
        fn ordinary_names_pass(path in "[a-z]{1,6}(/[a-z]{1,6}){0,3}\\.(rs|md|txt|toml)") {
            prop_assert!(!looks_secret(&path));
        }

        // With an exact `include`, nothing named is blocked.
        #[test]
        fn include_always_wins(pre in "[a-z]{1,6}", name in "[a-z]{1,6}") {
            let rel = format!("{pre}/.ssh/{name}");
            let f = SecretFilter::new(std::slice::from_ref(&rel));
            prop_assert!(!f.blocks(&rel, Path::new("/nonexistent")));
        }
    }
}
