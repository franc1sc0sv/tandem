//! Lo único del sistema que usa este crate: el programa `git`, detrás de [`Git`].
//!
//! [`SystemGit`] lo invoca de verdad; [`FakeGit`] anota las llamadas y contesta lo que la
//! prueba le diga, para probar la orquestación sin repos.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, PoisonError};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitError {
    #[error("no se pudo ejecutar git: {0}")]
    Spawn(String),
    /// git terminó con error; `stderr` es lo que dijo.
    #[error("git {args}: {stderr}")]
    Failed { args: String, stderr: String },
}

/// Una invocación de git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCmd {
    pub dir: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl GitCmd {
    pub fn new(dir: &Path, args: &[&str]) -> Self {
        GitCmd {
            dir: dir.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            env: Vec::new(),
        }
    }

    pub fn env(mut self, key: &str, value: impl Into<String>) -> Self {
        self.env.push((key.to_owned(), value.into()));
        self
    }
}

pub trait Git: Send + Sync {
    /// Ejecuta y devuelve stdout; un código de salida distinto de 0 es [`GitError::Failed`].
    fn run(&self, cmd: &GitCmd) -> Result<Vec<u8>, GitError>;
}

/// Atajos sobre [`Git`].
pub trait GitExt: Git {
    fn out(&self, dir: &Path, args: &[&str]) -> Result<String, GitError> {
        let bytes = self.run(&GitCmd::new(dir, args))?;
        Ok(String::from_utf8_lossy(&bytes).trim_end().to_owned())
    }

    fn ok(&self, dir: &Path, args: &[&str]) -> bool {
        self.run(&GitCmd::new(dir, args)).is_ok()
    }
}

impl<G: Git + ?Sized> GitExt for G {}

/// Donde apuntar `core.hooksPath` para que no exista ningún hook.
#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// El `git` del PATH.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemGit;

impl Git for SystemGit {
    fn run(&self, cmd: &GitCmd) -> Result<Vec<u8>, GitError> {
        let mut c = Command::new("git");
        // Sin conversión de fin de línea (el parche y los archivos tienen que viajar tal
        // cual), sin pedir nada por la terminal y sin heredar el repo de quien nos lanzó
        // (por ejemplo un hook).
        // Además, nada de código ajeno: los hooks y el fsmonitor vienen de la configuración
        // heredada (o de un `.git` dentro del workspace) y esto puede correr elevado. Con
        // `protocol.file.allow=user` un submódulo `file://` del bundle no se clona solo.
        c.args(["-c", "core.autocrlf=false", "-c", "core.safecrlf=false"])
            .args(["-c", &format!("core.hooksPath={NULL_DEVICE}")])
            // Ningún ayudante de credenciales, tampoco el global del usuario (T-130).
            .args(["-c", "credential.helper="])
            .args([
                "-c",
                "core.fsmonitor=false",
                "-c",
                "protocol.file.allow=user",
            ])
            .args(&cmd.args)
            .current_dir(&cmd.dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .env_remove("GIT_COMMON_DIR");
        for (k, v) in &cmd.env {
            c.env(k, v);
        }
        let out = c.output().map_err(|e| GitError::Spawn(e.to_string()))?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(GitError::Failed {
                args: cmd.args.join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            })
        }
    }
}

/// Un git de mentira: contesta en orden lo que se le encoló (vacío si no hay nada) y
/// anota cada llamada.
#[derive(Debug, Default)]
pub struct FakeGit {
    replies: Mutex<VecDeque<Result<Vec<u8>, GitError>>>,
    calls: Mutex<Vec<GitCmd>>,
}

impl FakeGit {
    pub fn push(&self, reply: Result<&str, &str>) {
        let r = reply
            .map(|s| s.as_bytes().to_vec())
            .map_err(|e| GitError::Failed {
                args: String::new(),
                stderr: e.to_owned(),
            });
        self.replies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(r);
    }

    pub fn calls(&self) -> Vec<GitCmd> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Git for FakeGit {
    fn run(&self, cmd: &GitCmd) -> Result<Vec<u8>, GitError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(cmd.clone());
        self.replies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| Ok(Vec::new()))
    }
}
