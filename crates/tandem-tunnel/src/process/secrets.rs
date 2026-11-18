//! Variables de entorno que parecen credenciales (T-130, D2: la máquina destino de un túnel
//! nunca recibe credenciales del origen, sea cual sea cada máquina).
//!
//! Los procesos ya no heredan el entorno del origen; esto cierra el único camino que queda,
//! el `env` explícito de `exec`. Se rechaza por nombre en la destino, aunque el origen no
//! lo haya revisado. `SSH_AUTH_SOCK` y los agentes no tienen permiso que valga: un socket
//! de agente da la firma sin entregar la llave, y justo eso es lo que no debe cruzar.

/// Variables que nunca pasan: agentes y programas que piden credenciales.
const AGENTS: &[&str] = &[
    "SSH_AUTH_SOCK",
    "SSH_AGENT_PID",
    "SSH_ASKPASS",
    "GIT_ASKPASS",
    "GPG_AGENT_INFO",
    "GNUPGHOME",
];

/// Prefijos de familias de credenciales.
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
];

/// Sufijos (o el nombre entero) que delatan un secreto.
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

/// Por qué se rechaza una variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvSecret {
    /// Un agente: no hay permiso que lo deje pasar.
    Agent,
    /// Parece un secreto: pasa solo con permiso explícito de la llamada.
    Secret,
}

/// Qué es `name`, o `None` si es una variable corriente.
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

/// Revisa el `env` de un `exec`. Devuelve el nombre y el motivo de la primera variable que
/// no puede pasar.
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
    fn los_secretos_se_reconocen_por_nombre() {
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
    fn lo_corriente_pasa() {
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
    fn los_agentes_nunca_pasan() {
        for n in [
            "SSH_AUTH_SOCK",
            "ssh_auth_sock",
            "SSH_AGENT_PID",
            "GIT_ASKPASS",
        ] {
            assert_eq!(classify_env(n), Some(EnvSecret::Agent), "{n}");
        }
        // Ni con permiso.
        let e = env(&["SSH_AUTH_SOCK"]);
        assert_eq!(
            check_env(&e, true),
            Err(("SSH_AUTH_SOCK".into(), EnvSecret::Agent))
        );
    }

    #[test]
    fn un_secreto_pasa_solo_con_permiso() {
        let e = env(&["RUST_LOG", "GH_TOKEN"]);
        assert_eq!(
            check_env(&e, false),
            Err(("GH_TOKEN".into(), EnvSecret::Secret))
        );
        assert_eq!(check_env(&e, true), Ok(()));
        assert_eq!(check_env(&env(&["RUST_LOG"]), false), Ok(()));
    }

    proptest! {
        // Con o sin permiso, un agente siempre se rechaza.
        #[test]
        fn el_agente_siempre_se_rechaza(allow: bool, extra in "[A-Z]{1,6}") {
            let e = vec![(extra, "x".into()), ("SSH_AUTH_SOCK".into(), "/s".into())];
            prop_assert!(check_env(&e, allow).is_err());
        }

        // Sin permiso, cualquier nombre que termine en _TOKEN se rechaza.
        #[test]
        fn terminar_en_token_se_rechaza(pre in "[A-Z]{1,8}") {
            let e = vec![(format!("{pre}_TOKEN"), "x".into())];
            prop_assert!(check_env(&e, false).is_err());
            prop_assert!(check_env(&e, true).is_ok());
        }
    }
}
