//! Correr programas del sistema para el escaneo, detrás de [`Runner`].
//!
//! El escaneo solo lee la salida de herramientas (`git --version`, `wsl.exe -l -v`,
//! `nvidia-smi`...). Detrás del trait, las pruebas usan las salidas guardadas como fixtures
//! ([`crate::fake::FakeRunner`]) y en la máquina real va [`SystemRunner`].

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Lo que dejó un programa.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    /// Terminó con código 0.
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    /// Salida exitosa con `stdout`.
    pub fn ok(stdout: impl Into<Vec<u8>>) -> Self {
        Output {
            success: true,
            stdout: stdout.into(),
            stderr: Vec::new(),
        }
    }

    /// Salida fallida con `stdout` y `stderr`.
    pub fn failed(stdout: impl Into<Vec<u8>>, stderr: impl Into<Vec<u8>>) -> Self {
        Output {
            success: false,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    /// `stdout` como texto; `wsl.exe` escribe UTF-16LE, el resto UTF-8.
    pub fn text(&self) -> String {
        crate::parse::decode_text(&self.stdout)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunError {
    #[error("no se pudo lanzar {program}: {reason}")]
    Spawn { program: String, reason: String },
    #[error("{program} no terminó en {secs} s")]
    Timeout { program: String, secs: u64 },
}

/// Corre programas y los busca en el `PATH`.
pub trait Runner: Send + Sync {
    /// Dónde está `program` (sin extensión) en el `PATH` de este proceso o en `extra` (otro
    /// `PATH`, por ejemplo el del shell de inicio de sesión del usuario).
    fn find(&self, program: &str, extra: Option<&str>) -> Option<PathBuf>;

    /// Todas las copias de `program` en el `PATH`, en orden (por ejemplo, el `bash.exe` de
    /// System32 y el de Git).
    fn find_all(&self, program: &str) -> Vec<PathBuf> {
        self.find(program, None).into_iter().collect()
    }

    /// Corre `program` (una ruta de [`Runner::find`]) y espera hasta `timeout`. Un código de
    /// salida distinto de 0 no es error: lo dice [`Output::success`].
    fn run(&self, program: &Path, args: &[&str], timeout: Duration) -> Result<Output, RunError>;

    /// Una variable de entorno de este proceso.
    fn var(&self, name: &str) -> Option<String>;
}

/// Los programas de verdad.
#[derive(Debug, Clone, Default)]
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn find(&self, program: &str, extra: Option<&str>) -> Option<PathBuf> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let exts = executable_exts(std::env::var_os("PATHEXT"));
        let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        if let Some(extra) = extra {
            dirs.extend(std::env::split_paths(extra));
        }
        find_in(program, &dirs, &exts, |p| p.is_file())
    }

    fn find_all(&self, program: &str) -> Vec<PathBuf> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let exts = executable_exts(std::env::var_os("PATHEXT"));
        std::env::split_paths(&path)
            .filter_map(|d| find_in(program, &[d], &exts, |p| p.is_file()))
            .collect()
    }

    fn run(&self, program: &Path, args: &[&str], timeout: Duration) -> Result<Output, RunError> {
        let name = program.display().to_string();
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        no_window(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| RunError::Spawn {
            program: name.clone(),
            reason: e.to_string(),
        })?;
        // Las tuberías se leen en hilos aparte: un programa que escribe mucho no se traba
        // esperando a que alguien lea mientras aquí se espera a que termine.
        let out = child.stdout.take().map(drain);
        let err = child.stderr.take().map(drain);
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() >= deadline => break None,
                Ok(None) => std::thread::sleep(Duration::from_millis(15)),
                Err(e) => {
                    return Err(RunError::Spawn {
                        program: name,
                        reason: e.to_string(),
                    });
                }
            }
        };
        let Some(status) = status else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunError::Timeout {
                program: name,
                secs: timeout.as_secs(),
            });
        };
        // Un nieto que se quedó con la tubería abierta no puede colgar el escaneo.
        let grace = Duration::from_secs(2);
        Ok(Output {
            success: status.success(),
            stdout: out
                .and_then(|rx| rx.recv_timeout(grace).ok())
                .unwrap_or_default(),
            stderr: err
                .and_then(|rx| rx.recv_timeout(grace).ok())
                .unwrap_or_default(),
        })
    }

    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|v| !v.is_empty())
    }
}

fn drain(mut r: impl Read + Send + 'static) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    rx
}

/// En Windows el agente corre sin consola: sin esta bandera cada programa abriría una
/// ventana negra un instante.
#[cfg(windows)]
fn no_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn no_window(_cmd: &mut Command) {}

/// Las extensiones con que se prueba cada nombre: en Windows las de `PATHEXT` (`npm` es
/// `npm.cmd`); en el resto, el nombre tal cual.
pub(crate) fn executable_exts(pathext: Option<OsString>) -> Vec<String> {
    if !cfg!(windows) {
        return vec![String::new()];
    }
    let raw = pathext
        .and_then(|p| p.into_string().ok())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
    raw.split(';')
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| e.starts_with('.'))
        .collect()
}

/// El primer `dir/program{ext}` que existe, en el orden de `dirs` y de `exts`.
pub(crate) fn find_in(
    program: &str,
    dirs: &[PathBuf],
    exts: &[String],
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    dirs.iter()
        .filter(|d| !d.as_os_str().is_empty())
        .flat_map(|d| exts.iter().map(move |e| d.join(format!("{program}{e}"))))
        .find(|p| exists(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_first_directory_and_extension_that_exists() {
        let dirs = [PathBuf::from("/a"), PathBuf::from(""), PathBuf::from("/b")];
        let exts = vec![".exe".to_owned(), ".cmd".to_owned()];
        let found = find_in("npm", &dirs, &exts, |p| {
            p == Path::new("/b/npm.cmd") || p == Path::new("/b/npm.exe")
        });
        assert_eq!(found, Some(PathBuf::from("/b/npm.exe")));
        assert_eq!(find_in("git", &dirs, &exts, |_| false), None);
    }

    #[cfg(windows)]
    #[test]
    fn pathext_is_lowercased_and_filtered() {
        assert_eq!(
            executable_exts(Some(".COM;.EXE;;CMD;.Bat".into())),
            [".com", ".exe", ".bat"]
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn outside_windows_the_name_is_used_as_is() {
        assert_eq!(executable_exts(Some(".EXE".into())), [""]);
    }

    #[cfg(unix)]
    #[test]
    fn a_real_program_runs_and_a_slow_one_is_cut() {
        let r = SystemRunner;
        let sh = r.find("sh", None).expect("sh en el PATH");
        let out = r
            .run(
                &sh,
                &["-c", "echo hola; echo mal >&2; exit 3"],
                Duration::from_secs(5),
            )
            .unwrap();
        assert!(!out.success);
        assert_eq!(out.text(), "hola\n");
        assert_eq!(out.stderr, b"mal\n");
        let e = r
            .run(&sh, &["-c", "sleep 5"], Duration::from_millis(100))
            .unwrap_err();
        assert!(matches!(e, RunError::Timeout { .. }), "{e}");
        let e = r
            .run(Path::new("/no/existe"), &[], Duration::from_secs(1))
            .unwrap_err();
        assert!(matches!(e, RunError::Spawn { .. }), "{e}");
    }
}
