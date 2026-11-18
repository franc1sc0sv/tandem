//! Per-tunnel git workspaces (DESIGN §8, D8, T-107): the messages.
//!
//! The data going and coming back (bundles and patches) travel as files
//! ([`crate::FilesUp`]); these requests only prepare folders and ask the destination to
//! run git. The logic is in `tandem-workspace`.

use serde::{Deserialize, Serialize};

use crate::TunnelId;

/// The repo that goes along with a tunnel (`tunnel_open { workspace }`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSpec {
    /// Path of the repo on the opening machine (can be a worktree or a subfolder).
    pub repo_path: String,
    /// Keep the destination folder when the tunnel closes.
    pub keep: bool,
    /// Untracked credentials that do travel, one by one (repo paths). By
    /// default nothing that looks like one is sent (T-130).
    pub include: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitInfo {
    pub id: String,
    pub summary: String,
    pub author: String,
}

/// What `bring_back` left.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BringBackReport {
    /// The branch created on the source; `None` if there were no new commits.
    pub branch: Option<String>,
    pub commits: Vec<CommitInfo>,
    /// A commit was made with what was pending on the destination.
    pub committed_pending: bool,
}

/// From the source to the destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceUp {
    /// Create the folders of the tunnel's workspace. Answers [`WorkspaceDown::Prepared`].
    Prepare { tunnel: TunnelId, keep: bool },
    /// With the bundle and patch already copied to `stage`: clone, apply and leave the folder
    /// as the processes' `cwd`. Answers [`WorkspaceDown::Ready`].
    Checkout { tunnel: TunnelId },
    /// Package what is new: `commit` (a message) commits what is pending first.
    /// Answers [`WorkspaceDown::Packed`].
    Pack {
        tunnel: TunnelId,
        commit: Option<String>,
    },
}

/// From the destination to the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceDown {
    Prepared {
        /// Folder where the source leaves `repo.bundle`, `changes.patch` and `submodules/`.
        stage: String,
    },
    Ready {
        dir: String,
    },
    Packed {
        /// File (on the destination) with the new commits; `None` if there are none.
        bundle: Option<String>,
        /// The commit it starts from (the source's `HEAD` when opening).
        base: String,
        commits: Vec<CommitInfo>,
        committed_pending: bool,
    },
    Failed(String),
}
