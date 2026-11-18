//! The only system thing this crate uses: the `git` program, behind [`Git`].
//!
//! [`SystemGit`] really invokes it; [`FakeGit`] records the calls and answers whatever the
//! test tells it to, to test the orchestration without repos.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, PoisonError};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitError {
    #[error("could not run git: {0}")]
    Spawn(String),
    /// git ended with an error; `stderr` is what it said.
    #[error("git {args}: {stderr}")]
    Failed { args: String, stderr: String },
}

/// One git invocation.
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
    /// Runs and returns stdout; an exit code other than 0 is [`GitError::Failed`].
    fn run(&self, cmd: &GitCmd) -> Result<Vec<u8>, GitError>;
}

/// Shortcuts over [`Git`].
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

/// Where to point `core.hooksPath` so that no hook exists.
#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// The `git` on the PATH.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemGit;

impl Git for SystemGit {
    fn run(&self, cmd: &GitCmd) -> Result<Vec<u8>, GitError> {
        let mut c = Command::new("git");
        // No line-ending conversion (the patch and the files have to travel as
        // they are), no prompting on the terminal and no inheriting the repo of whoever launched us
        // (for example a hook).
        // Also, no foreign code: hooks and fsmonitor come from the inherited configuration
        // (or from a `.git` inside the workspace) and this may run elevated. With
        // `protocol.file.allow=user` a `file://` submodule of the bundle is not cloned on its own.
        c.args(["-c", "core.autocrlf=false", "-c", "core.safecrlf=false"])
            .args(["-c", &format!("core.hooksPath={NULL_DEVICE}")])
            // No credential helper, not even the user's global one (T-130).
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

/// A fake git: answers in order what was queued for it (empty if nothing) and
/// records each call.
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
