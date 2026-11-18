//! De un `exec` (programa, argumentos, entorno elegido) a la orden concreta que se lanza.
//!
//! Portable y sin efectos: se prueba en Linux para los dos sistemas. Con un shell, `cmd` es
//! la línea que interpreta ese shell y cada argumento se le agrega citado como un literal.

use tandem_proto::ExecSpec;

use super::secrets;

/// El sistema de la destino: cambia qué shells hay y cómo se citan los argumentos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Unix,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(windows) { Os::Windows } else { Os::Unix }
    }
}

/// La orden concreta que lanza un [`super::Spawner`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    /// Se agrega tal cual al final de la línea de órdenes de Windows (`cmd /s /c "…"`, que
    /// no sigue las reglas de citado de los demás programas). En Unix no se usa.
    pub raw: Option<String>,
    /// Carpeta de trabajo del proceso; `None` deja la del agente.
    pub cwd: Option<String>,
    /// Variables que se agregan al proceso.
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LaunchError {
    #[error("falta el programa a ejecutar")]
    NoCommand,
    #[error("no hay ningún entorno «{0}» que sepa lanzar")]
    UnknownShell(String),
    #[error("el entorno «{0}» solo existe en Windows")]
    WindowsOnly(String),
    #[error("para Docker hay que decir el contenedor: «docker:<contenedor>»")]
    NoContainer,
    #[error("nombre de variable inválido: «{0}»")]
    BadVariable(String),
    #[error(
        "la variable «{0}» parece un secreto: la otra máquina no recibe credenciales; \
         hace falta permiso explícito en esta llamada (allow_secret_env)"
    )]
    SecretEnv(String),
    #[error("la variable «{0}» es un agente de credenciales y nunca se reenvía")]
    AgentEnv(String),
}

/// Lo que se muestra del proceso: el programa y sus argumentos (citados si hace falta).
pub fn display(spec: &ExecSpec) -> String {
    let mut line = spec.cmd.trim().to_owned();
    for a in &spec.args {
        line.push(' ');
        line.push_str(&posix_quote(a));
    }
    line
}

pub fn plan(spec: &ExecSpec, os: Os) -> Result<Launch, LaunchError> {
    let cmd = spec.cmd.trim();
    if cmd.is_empty() {
        return Err(LaunchError::NoCommand);
    }
    for (k, _) in &spec.env {
        if k.is_empty() || k.contains('=') || k.contains('\0') {
            return Err(LaunchError::BadVariable(k.clone()));
        }
    }
    secrets::check_env(&spec.env, spec.allow_secret_env).map_err(|(name, why)| match why {
        secrets::EnvSecret::Agent => LaunchError::AgentEnv(name),
        secrets::EnvSecret::Secret => LaunchError::SecretEnv(name),
    })?;
    let base = Launch {
        cwd: spec.cwd.clone(),
        env: spec.env.clone(),
        ..Launch::default()
    };
    let Some(shell) = spec.shell.as_deref() else {
        return Ok(Launch {
            program: cmd.to_owned(),
            args: spec.args.clone(),
            ..base
        });
    };
    let posix_line = || {
        let mut line = cmd.to_owned();
        for a in &spec.args {
            line.push(' ');
            line.push_str(&posix_quote(a));
        }
        line
    };
    match shell {
        // Shell de login: el agente arranca con el PATH mínimo del sistema (launchd, la
        // tarea programada) y las herramientas del usuario (cargo, uv) están en su perfil.
        "zsh" | "bash" | "fish" | "sh" => Ok(Launch {
            program: shell.to_owned(),
            args: vec!["-l".into(), "-c".into(), posix_line()],
            ..base
        }),
        "pwsh" | "powershell" => {
            let mut line = cmd.to_owned();
            for a in &spec.args {
                line.push(' ');
                line.push_str(&pwsh_quote(a));
            }
            // `-EncodedCommand` (UTF-16LE en base64): la línea llega entera, sin pasar por
            // las reglas de citado de la línea de órdenes de Windows.
            Ok(Launch {
                program: shell.to_owned(),
                args: vec![
                    "-NoLogo".into(),
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-EncodedCommand".into(),
                    encode_utf16_base64(&line),
                ],
                ..base
            })
        }
        "cmd" => {
            if os != Os::Windows {
                return Err(LaunchError::WindowsOnly(shell.into()));
            }
            let mut line = cmd.to_owned();
            for a in &spec.args {
                line.push(' ');
                line.push_str(&cmd_quote(a));
            }
            // Con `/s`, cmd quita la primera y la última comilla y ejecuta lo de adentro
            // tal cual.
            Ok(Launch {
                program: "cmd".into(),
                args: vec!["/d".into(), "/s".into(), "/c".into()],
                raw: Some(format!("\"{line}\"")),
                ..base
            })
        }
        _ => {
            if let Some(distro) = shell.strip_prefix("wsl:") {
                if os != Os::Windows {
                    return Err(LaunchError::WindowsOnly(shell.into()));
                }
                if distro.is_empty() {
                    return Err(LaunchError::UnknownShell(shell.into()));
                }
                // Las variables de Windows no cruzan a WSL (salvo con WSLENV): van en la línea.
                let mut args = vec!["-d".into(), distro.to_owned()];
                if let Some(cwd) = &spec.cwd {
                    args.push("--cd".into());
                    args.push(cwd.clone());
                }
                args.extend(["--exec".into(), "sh".into(), "-lc".into()]);
                args.push(format!("{}{}", exports(&spec.env), posix_line()));
                return Ok(Launch {
                    program: "wsl.exe".into(),
                    args,
                    raw: None,
                    cwd: None,
                    env: Vec::new(),
                });
            }
            if shell == "docker" {
                return Err(LaunchError::NoContainer);
            }
            if let Some(container) = shell.strip_prefix("docker:") {
                if container.is_empty() {
                    return Err(LaunchError::NoContainer);
                }
                let mut args = vec!["exec".into(), "-i".into()];
                if let Some(cwd) = &spec.cwd {
                    args.push("-w".into());
                    args.push(cwd.clone());
                }
                for (k, v) in &spec.env {
                    args.push("-e".into());
                    args.push(format!("{k}={v}"));
                }
                args.extend([container.to_owned(), "sh".into(), "-lc".into()]);
                args.push(posix_line());
                return Ok(Launch {
                    program: "docker".into(),
                    args,
                    raw: None,
                    cwd: None,
                    env: Vec::new(),
                });
            }
            Err(LaunchError::UnknownShell(shell.into()))
        }
    }
}

fn exports(env: &[(String, String)]) -> String {
    env.iter()
        .map(|(k, v)| format!("export {k}={}; ", posix_quote(v)))
        .collect()
}

/// Un literal para sh: sin citar si no hace falta, si no entre comillas simples.
pub fn posix_quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:,+@%".contains(&b));
    if plain {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Un literal para PowerShell: comillas simples, con `''` para una comilla.
pub fn pwsh_quote(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        // PowerShell también toma las comillas tipográficas simples como delimitadores.
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// Un argumento para cmd: entre comillas si tiene espacios o metacaracteres. Las comillas
/// de adentro se duplican (como las entiende la mayoría de los programas de Windows).
pub fn cmd_quote(s: &str) -> String {
    let plain = !s.is_empty()
        && !s
            .chars()
            .any(|c| c.is_whitespace() || "\"&|<>^()%!,;=".contains(c));
    if plain {
        s.to_owned()
    } else {
        format!("\"{}\"", s.replace('"', "\"\""))
    }
}

/// Base64 (estándar, con relleno) del texto en UTF-16LE: lo que pide `-EncodedCommand`.
pub fn encode_utf16_base64(text: &str) -> String {
    let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64(&bytes)
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ABC[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn spec(cmd: &str, args: &[&str], shell: Option<&str>) -> ExecSpec {
        ExecSpec {
            cmd: cmd.into(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            shell: shell.map(Into::into),
            cwd: Some("/src/x".into()),
            allow_secret_env: false,
            env: vec![("RUST_LOG".into(), "debug".into())],
        }
    }

    #[test]
    fn un_secreto_en_el_entorno_se_rechaza_sin_permiso_explicito() {
        let mut s = spec("git", &["push"], None);
        s.env = vec![("GH_TOKEN".into(), "ghp_x".into())];
        assert_eq!(
            plan(&s, Os::Unix),
            Err(LaunchError::SecretEnv("GH_TOKEN".into()))
        );
        s.allow_secret_env = true;
        assert!(plan(&s, Os::Unix).is_ok());
        // Un agente no tiene permiso que valga, ni con shell.
        s.env = vec![("SSH_AUTH_SOCK".into(), "/tmp/a".into())];
        s.shell = Some("zsh".into());
        assert_eq!(
            plan(&s, Os::Unix),
            Err(LaunchError::AgentEnv("SSH_AUTH_SOCK".into()))
        );
    }

    #[test]
    fn sin_shell_es_el_programa_directo() {
        let l = plan(&spec("cargo", &["test", "a b"], None), Os::Unix).unwrap();
        assert_eq!(l.program, "cargo");
        assert_eq!(l.args, ["test", "a b"]);
        assert_eq!(l.cwd.as_deref(), Some("/src/x"));
        assert_eq!(l.env, [("RUST_LOG".to_owned(), "debug".to_owned())]);
        assert_eq!(
            plan(&spec("  ", &[], None), Os::Unix),
            Err(LaunchError::NoCommand)
        );
    }

    #[test]
    fn los_shells_posix_son_de_login_y_citan_los_argumentos() {
        let l = plan(&spec("cargo test", &["it's", "x"], Some("zsh")), Os::Unix).unwrap();
        assert_eq!(l.program, "zsh");
        assert_eq!(l.args, ["-l", "-c", r"cargo test 'it'\''s' x"]);
    }

    #[test]
    fn powershell_va_codificado() {
        let l = plan(&spec("Write-Output", &["a'b"], Some("pwsh")), Os::Windows).unwrap();
        assert_eq!(l.program, "pwsh");
        assert_eq!(
            l.args[..4],
            [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand"
            ]
        );
        assert_eq!(l.args[4], encode_utf16_base64("Write-Output 'a''b'"));
    }

    #[test]
    fn cmd_va_crudo_y_solo_en_windows() {
        let l = plan(&spec("echo", &["a b", "c\"d"], Some("cmd")), Os::Windows).unwrap();
        assert_eq!(l.args, ["/d", "/s", "/c"]);
        assert_eq!(l.raw.as_deref(), Some(r#""echo "a b" "c""d"""#));
        assert_eq!(
            plan(&spec("echo", &[], Some("cmd")), Os::Unix),
            Err(LaunchError::WindowsOnly("cmd".into()))
        );
    }

    #[test]
    fn wsl_lleva_la_carpeta_y_las_variables_en_la_linea() {
        let l = plan(
            &spec("cargo", &["test"], Some("wsl:Ubuntu-24.04")),
            Os::Windows,
        )
        .unwrap();
        assert_eq!(l.program, "wsl.exe");
        assert_eq!(
            l.args,
            [
                "-d",
                "Ubuntu-24.04",
                "--cd",
                "/src/x",
                "--exec",
                "sh",
                "-lc",
                "export RUST_LOG=debug; cargo test"
            ]
        );
        assert_eq!((l.cwd, l.env), (None, vec![]));
    }

    #[test]
    fn docker_necesita_el_contenedor() {
        assert_eq!(
            plan(&spec("ls", &[], Some("docker")), Os::Unix),
            Err(LaunchError::NoContainer)
        );
        let l = plan(&spec("ls", &["-la"], Some("docker:api")), Os::Unix).unwrap();
        assert_eq!(l.program, "docker");
        assert_eq!(
            l.args,
            [
                "exec",
                "-i",
                "-w",
                "/src/x",
                "-e",
                "RUST_LOG=debug",
                "api",
                "sh",
                "-lc",
                "ls -la"
            ]
        );
    }

    #[test]
    fn entornos_y_variables_invalidos() {
        assert_eq!(
            plan(&spec("x", &[], Some("tcsh")), Os::Unix),
            Err(LaunchError::UnknownShell("tcsh".into()))
        );
        let mut s = spec("x", &[], None);
        s.env = vec![("A=B".into(), "c".into())];
        assert_eq!(
            plan(&s, Os::Unix),
            Err(LaunchError::BadVariable("A=B".into()))
        );
    }

    #[test]
    fn base64_conocido() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        // `powershell -EncodedCommand` de «dir».
        assert_eq!(encode_utf16_base64("dir"), "ZABpAHIA");
    }

    /// Decodifica lo que produce `posix_quote` como lo haría sh (solo comillas simples y
    /// `\'` fuera de ellas).
    fn sh_unquote(s: &str) -> String {
        let (mut out, mut inside, mut chars) = (String::new(), false, s.chars());
        while let Some(c) = chars.next() {
            match (c, inside) {
                ('\'', _) => inside = !inside,
                ('\\', false) => out.extend(chars.next()),
                (c, _) => out.push(c),
            }
        }
        out
    }

    proptest! {
        #[test]
        fn citar_para_sh_conserva_el_literal(s in "\\PC{0,40}") {
            prop_assert_eq!(sh_unquote(&posix_quote(&s)), s);
        }

        #[test]
        fn citar_para_powershell_conserva_el_literal(s in "[a-z' \u{2019}]{0,20}") {
            let q = pwsh_quote(&s);
            let inner = &q[1..q.len() - 1];
            let mut out = String::new();
            let mut chars = inner.chars().peekable();
            while let Some(c) = chars.next() {
                out.push(c);
                if matches!(c, '\'' | '\u{2019}') {
                    prop_assert_eq!(chars.next(), Some(c));
                }
            }
            prop_assert_eq!(out, s);
        }
    }
}
