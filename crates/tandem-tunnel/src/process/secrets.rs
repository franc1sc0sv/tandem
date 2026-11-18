//! Environment variables that look like credentials (T-130, D2: a tunnel's destination machine
//! never receives credentials from the origin, whichever machine each is).
//!
//! Processes no longer inherit the origin's environment; this closes the only remaining path,
//! the explicit `env` of `exec`. It is rejected by name on the destination, even if the origin did not
//! check it. `SSH_AUTH_SOCK` and agents have no permission that applies: an agent
//! socket gives the signature without handing over the key, and that is exactly what must not cross.

/// Variables that never pass: agents and programs that ask for credentials.
const AGENTS: &[&str] = &[
    "SSH_AUTH_SOCK",
    "SSH_AGENT_PID",
    "SSH_ASKPASS",
    "GIT_ASKPASS",
    "GPG_AGENT_INFO",
    "GNUPGHOME",
];

/// Prefixes of credential families.
const PREFIXES: &[&str] = &[
    "GH_",
    "GITHUB_",
    "GITLAB_",
    "AWS_",
    "AZURE_",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "NPM_CONFIG_",
    "DOCKER_AUTH",
    "KUBECONFIG",
    // Git reads configuration (helpers, `url.*.insteadOf`, headers) from these variables.
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_KEY_",
    "GIT_CONFIG_VALUE_",
    "GIT_CONFIG_PARAMETERS",
];

/// Suffixes (or the whole name) that give away a secret.
const SUFFIXES: &[&str] = &[
    "TOKEN",
    "SECRET",
    "KEY",
    "PASSWORD",
    "PASSWD",
    "PASS",
    "CREDENTIAL",
    "CREDENTIALS",
    "APIKEY",
    "PAT",
];

/// Why a variable is rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvSecret {
    /// An agent: no permission lets it through.
    Agent,
    /// Looks like a secret: passes only with the call's explicit permission.
    Secret,
}

/// What `name` is, or `None` if it is an ordinary variable.
pub fn classify_env(name: &str) -> Option<EnvSecret> {
    let up = name.to_ascii_uppercase();
    if AGENTS.contains(&up.as_str()) {
        return Some(EnvSecret::Agent);
    }
    if PREFIXES.iter().any(|p| up.starts_with(p)) {
        return Some(EnvSecret::Secret);
    }
    let last = up.rsplit(['_', '-']).next().unwrap_or(&up);
    SUFFIXES.contains(&last).then_some(EnvSecret::Secret)
}

/// Checks the `env` of an `exec`. Returns the name and the reason of the first variable that
/// cannot pass.
pub fn check_env(env: &[(String, String)], allow_secrets: bool) -> Result<(), (String, EnvSecret)> {
    for (k, _) in env {
        match classify_env(k) {
            Some(EnvSecret::Agent) => return Err((k.clone(), EnvSecret::Agent)),
            Some(EnvSecret::Secret) if !allow_secrets => {
                return Err((k.clone(), EnvSecret::Secret));
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn env(names: &[&str]) -> Vec<(String, String)> {
        names.iter().map(|n| ((*n).into(), "v".into())).collect()
    }

    #[test]
    fn secrets_are_recognized_by_name() {
        for n in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "AWS_ACCESS_KEY_ID",
            "AWS_REGION",
            "NPM_TOKEN",
            "MY_SECRET",
            "DB_PASSWORD",
            "OPENAI_API_KEY",
            "gh_token",
            "TOKEN",
            "GOOGLE_APPLICATION_CREDENTIALS",
        ] {
            assert_eq!(classify_env(n), Some(EnvSecret::Secret), "{n}");
        }
    }

    #[test]
    fn git_configuration_through_the_environment_is_secret() {
        for n in [
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_KEY_0",
            "GIT_CONFIG_VALUE_12",
            "GIT_CONFIG_PARAMETERS",
            "git_config_count",
        ] {
            assert_eq!(classify_env(n), Some(EnvSecret::Secret), "{n}");
        }
        let e = env(&["GIT_CONFIG_COUNT"]);
        assert!(check_env(&e, true).is_ok());
        assert_eq!(
            check_env(&e, false),
            Err(("GIT_CONFIG_COUNT".into(), EnvSecret::Secret))
        );
        assert_eq!(classify_env("GIT_DIR"), None);
    }

    #[test]
    fn ordinary_names_pass() {
        for n in [
            "RUST_LOG",
            "PATH",
            "HOME",
            "CI",
            "NODE_ENV",
            "TOKENIZER",
            "MONKEY",
        ] {
            assert_eq!(classify_env(n), None, "{n}");
        }
    }

    #[test]
    fn agents_never_pass() {
        for n in [
            "SSH_AUTH_SOCK",
            "ssh_auth_sock",
            "SSH_AGENT_PID",
            "GIT_ASKPASS",
        ] {
            assert_eq!(classify_env(n), Some(EnvSecret::Agent), "{n}");
        }
        // Not even with permission.
        let e = env(&["SSH_AUTH_SOCK"]);
        assert_eq!(
            check_env(&e, true),
            Err(("SSH_AUTH_SOCK".into(), EnvSecret::Agent))
        );
    }

    #[test]
    fn a_secret_passes_only_with_permission() {
        let e = env(&["RUST_LOG", "GH_TOKEN"]);
        assert_eq!(
            check_env(&e, false),
            Err(("GH_TOKEN".into(), EnvSecret::Secret))
        );
        assert_eq!(check_env(&e, true), Ok(()));
        assert_eq!(check_env(&env(&["RUST_LOG"]), false), Ok(()));
    }

    proptest! {
        // With or without permission, an agent is always rejected.
        #[test]
        fn the_agent_is_always_rejected(allow: bool, extra in "[A-Z]{1,6}") {
            let e = vec![(extra, "x".into()), ("SSH_AUTH_SOCK".into(), "/s".into())];
            prop_assert!(check_env(&e, allow).is_err());
        }

        // Without permission, any name ending in _TOKEN is rejected.
        #[test]
        fn ending_in_token_is_rejected(pre in "[A-Z]{1,8}") {
            let e = vec![(format!("{pre}_TOKEN"), "x".into())];
            prop_assert!(check_env(&e, false).is_err());
            prop_assert!(check_env(&e, true).is_ok());
        }
    }
}
