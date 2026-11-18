//! Repo operations, blocking and behind [`Git`].
//!
//! Outbound (origin): a bundle of `HEAD` and a patch with everything pending (also new
//! untracked files, never ignored ones), computed with a temporary index so as not to touch the
//! user's. Destination: init + bundle fetch + patch. Return: optional commit of the
//! pending work, bundle of the new commits and, at the origin, a new branch with `fetch`
//! (it touches neither the current branch nor the working tree).

use std::fs;
use std::path::{Path, PathBuf};

use tandem_files::SecretFilter;
use tandem_proto::CommitInfo;

use crate::error::WorkspaceError;
use crate::git::{Git, GitCmd, GitExt};

pub const BUNDLE: &str = "repo.bundle";
pub const PATCH: &str = "changes.patch";
pub const SUBMODULES: &str = "submodules";
/// Who the user is at the origin (name and email, one per line): commits on the
/// destination are made in their name.
pub const IDENTITY: &str = "identity.txt";
pub const BACK_BUNDLE: &str = "back.bundle";
/// The commit the work starts from on the destination.
pub const BASE_REF: &str = "refs/tandem/base";
/// The destination branch.
pub const WORK_BRANCH: &str = "tandem/work";

/// What the origin left in the send folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packed {
    pub root: PathBuf,
    /// The origin's `HEAD`: the work starts from it and returns to it.
    pub head: String,
    pub branch: Option<String>,
    pub has_patch: bool,
    pub submodules: Vec<String>,
    /// What looked like a credential and does not travel (T-130); repo paths.
    pub skipped: Vec<String>,
    /// Credentials that at some point entered the history that travels in the bundle (T-130):
    /// they cannot be removed, only reported.
    pub history_secrets: Vec<String>,
}

/// Deletes the file on exit, whichever way (a `?` included).
struct RemoveOnDrop<'a>(&'a Path);

impl Drop for RemoveOnDrop<'_> {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0);
    }
}

fn io(what: &str, e: std::io::Error) -> WorkspaceError {
    WorkspaceError::Io(format!("{what}: {e}"))
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Prepares in `stage` what travels to the destination.
pub fn pack_origin(
    git: &dyn Git,
    repo: &Path,
    stage: &Path,
    filter: &SecretFilter,
) -> Result<Packed, WorkspaceError> {
    fs::create_dir_all(stage).map_err(|e| io("create the send folder", e))?;
    let top = git
        .out(repo, &["rev-parse", "--show-toplevel"])
        .map_err(|_| WorkspaceError::NotARepo(s(repo)))?;
    let root = PathBuf::from(top);
    let head = git
        .out(&root, &["rev-parse", "--verify", "HEAD"])
        .map_err(|_| WorkspaceError::NoCommits(s(&root)))?;
    let branch = git
        .out(&root, &["symbolic-ref", "--short", "-q", "HEAD"])
        .ok()
        .filter(|b| !b.is_empty());

    let bundle = stage.join(BUNDLE);
    git.run(&GitCmd::new(
        &root,
        &["bundle", "create", "-q", &s(&bundle), "HEAD"],
    ))?;

    // Separate index: `add -A` on the user's would change what they have staged.
    let index = stage.join("index.tmp");
    let _cleanup = RemoveOnDrop(&index);
    let with_index = |args: &[&str]| GitCmd::new(&root, args).env("GIT_INDEX_FILE", s(&index));
    git.run(&with_index(&["read-tree", "HEAD"]))?;
    git.run(&with_index(&["add", "-A"]))?;
    // Credentials do not travel, whether or not they are in `.gitignore` (T-130): in the
    // temporary index they are reset to what `HEAD` says (new ones disappear, modified ones go back to
    // their committed version) and are noted. What is already committed is in the bundle: the
    // history is not rewritten.
    let mut skipped = Vec::new();
    let changed = git.run(&with_index(&[
        "diff",
        "--cached",
        "--name-only",
        "-z",
        "--no-renames",
        "--ignore-submodules=all",
        "HEAD",
    ]))?;
    for rel in changed.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let rel = String::from_utf8_lossy(rel).into_owned();
        if filter.blocks(&rel, &root.join(&rel)) {
            skipped.push(rel);
        }
    }
    for chunk in skipped.chunks(64) {
        let mut args = vec!["--literal-pathspecs", "reset", "-q", "HEAD", "--"];
        args.extend(chunk.iter().map(String::as_str));
        git.run(&with_index(&args))?;
    }
    // Submodules travel copied (below), not as a patch.
    let patch = git.run(&with_index(&[
        "diff",
        "--cached",
        "--binary",
        "--full-index",
        "--no-color",
        "--no-ext-diff",
        "--no-renames",
        "--ignore-submodules=all",
        "HEAD",
    ]));
    let patch = patch?;
    let has_patch = !patch.is_empty();
    if has_patch {
        fs::write(stage.join(PATCH), &patch).map_err(|e| io("write the patch", e))?;
    }

    let name = git.out(&root, &["config", "user.name"]).unwrap_or_default();
    let email = git
        .out(&root, &["config", "user.email"])
        .unwrap_or_default();
    if !name.is_empty() && !email.is_empty() {
        fs::write(stage.join(IDENTITY), format!("{name}\n{email}\n"))
            .map_err(|e| io("write the identity", e))?;
    }
    let submodules = submodule_paths(git, &root)?;
    for sub in &submodules {
        let to = stage.join(SUBMODULES).join(sub);
        copy_tree_with(&root.join(sub), &to, sub, filter, &mut skipped)?;
    }
    let history_secrets = history_secrets(git, &root);
    Ok(Packed {
        root,
        head,
        branch,
        has_patch,
        submodules,
        skipped,
        history_secrets,
    })
}

/// Paths that look like credentials and were at some point added to the history of `HEAD`
/// (the one that travels in the bundle). If `git log` fails nothing is reported: it is a notice, not a filter.
fn history_secrets(git: &dyn Git, root: &Path) -> Vec<String> {
    let Ok(out) = git.run(&GitCmd::new(
        root,
        &[
            "log",
            "HEAD",
            "--diff-filter=A",
            "--name-only",
            "--no-renames",
            "-z",
            "--format=",
        ],
    )) else {
        return Vec::new();
    };
    let mut found: Vec<String> = out
        .split(|b| *b == 0)
        .filter(|r| !r.is_empty())
        .map(|r| String::from_utf8_lossy(r).into_owned())
        .filter(|rel| tandem_files::secrets::looks_secret(rel))
        .collect();
    found.sort();
    found.dedup();
    found
}

/// The submodule paths of `HEAD` (`160000` index entries).
fn submodule_paths(git: &dyn Git, root: &Path) -> Result<Vec<String>, WorkspaceError> {
    let out = git.run(&GitCmd::new(root, &["ls-files", "-z", "--stage"]))?;
    Ok(out
        .split(|b| *b == 0)
        .filter_map(|rec| {
            let rec = String::from_utf8_lossy(rec);
            let (meta, path) = rec.split_once('\t')?;
            meta.starts_with("160000").then(|| path.to_owned())
        })
        .collect())
}

/// Copies a tree without its `.git` or symbolic links.
pub fn copy_tree(from: &Path, to: &Path) -> Result<(), WorkspaceError> {
    copy_tree_with(from, to, "", &SecretFilter::default(), &mut Vec::new())
}

/// Like [`copy_tree`], without the credentials (T-130); `rel` is the path of `from` in the repo.
fn copy_tree_with(
    from: &Path,
    to: &Path,
    rel: &str,
    filter: &SecretFilter,
    skipped: &mut Vec<String>,
) -> Result<(), WorkspaceError> {
    fs::create_dir_all(to).map_err(|e| io("create a folder", e))?;
    let Ok(entries) = fs::read_dir(from) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry.map_err(|e| io("read a folder", e))?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let kind = entry.file_type().map_err(|e| io("read an entry", e))?;
        let dest = to.join(&name);
        let sub = if rel.is_empty() {
            name.to_string_lossy().into_owned()
        } else {
            format!("{rel}/{}", name.to_string_lossy())
        };
        if (kind.is_dir() || kind.is_file()) && filter.blocks(&sub, &entry.path()) {
            skipped.push(sub);
        } else if kind.is_dir() {
            copy_tree_with(&entry.path(), &dest, &sub, filter, skipped)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &dest).map_err(|e| io("copy a file", e))?;
        }
    }
    Ok(())
}

/// On the destination: builds `work` with the bundle and the patch the origin left in `stage`.
pub fn checkout(git: &dyn Git, stage: &Path, work: &Path) -> Result<(), WorkspaceError> {
    let bundle = stage.join(BUNDLE);
    if !bundle.is_file() {
        return Err(WorkspaceError::Missing(s(&bundle)));
    }
    if work.exists() {
        fs::remove_dir_all(work).map_err(|e| io("clean the workspace", e))?;
    }
    fs::create_dir_all(work).map_err(|e| io("create the workspace", e))?;
    git.run(&GitCmd::new(work, &["init", "-q", "--template="]))?;
    strip_credentials(git, work)?;
    let fetch = format!("+HEAD:{BASE_REF}");
    git.run(&GitCmd::new(
        work,
        &["fetch", "-q", "--no-tags", &s(&bundle), &fetch],
    ))?;
    git.run(&GitCmd::new(
        work,
        &["checkout", "-q", "-b", WORK_BRANCH, BASE_REF],
    ))?;
    // Commits are made in the origin user's name; if unknown, a local identity
    // only if needed (without one git cannot commit).
    let identity = fs::read_to_string(stage.join(IDENTITY)).unwrap_or_default();
    let mut lines = identity.lines();
    if let (Some(name), Some(email)) = (lines.next(), lines.next()) {
        git.run(&GitCmd::new(work, &["config", "user.name", name]))?;
        git.run(&GitCmd::new(work, &["config", "user.email", email]))?;
    } else if git
        .out(work, &["config", "user.email"])
        .unwrap_or_default()
        .is_empty()
    {
        git.run(&GitCmd::new(
            work,
            &["config", "user.email", "tandem@localhost"],
        ))?;
        git.run(&GitCmd::new(work, &["config", "user.name", "Tandem"]))?;
    }
    let subs = stage.join(SUBMODULES);
    if subs.is_dir() {
        copy_tree(&subs, work)?;
    }
    let patch = stage.join(PATCH);
    if patch.is_file() {
        git.run(&GitCmd::new(
            work,
            &["apply", "--binary", "--whitespace=nowarn", &s(&patch)],
        ))?;
    }
    Ok(())
}

/// The destination repo keeps no remote or credential helper (T-130): not even
/// the bundle `fetch` creates them, but they are deleted in case the template or the system
/// put them there, and an empty `credential.helper` is left so the global one does not apply either.
pub fn strip_credentials(git: &dyn Git, work: &Path) -> Result<(), WorkspaceError> {
    for remote in git.out(work, &["remote"])?.lines().map(str::trim) {
        if !remote.is_empty() {
            git.run(&GitCmd::new(work, &["remote", "remove", remote]))?;
        }
    }
    // `--local`: if there is no helper of its own, `--unset-all` exits with 5 and it is not an error.
    let _ = git.run(&GitCmd::new(
        work,
        &["config", "--local", "--unset-all", "credential.helper"],
    ));
    Ok(())
}

/// What `bring_back` takes out of the destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Back {
    pub bundle: Option<PathBuf>,
    pub base: String,
    pub commits: Vec<CommitInfo>,
    pub committed_pending: bool,
}

/// On the destination: with `message`, commits the pending work; then packs the
/// commits that were not at the origin into `out`.
pub fn pack_back(
    git: &dyn Git,
    work: &Path,
    message: Option<&str>,
    out: &Path,
) -> Result<Back, WorkspaceError> {
    let mut committed_pending = false;
    if let Some(msg) = message
        && !git.out(work, &["status", "--porcelain"])?.is_empty()
    {
        git.run(&GitCmd::new(work, &["add", "-A"]))?;
        git.run(&GitCmd::new(
            work,
            &["commit", "-q", "--no-verify", "-m", msg],
        ))?;
        committed_pending = true;
    }
    let base = git
        .out(work, &["rev-parse", "--verify", BASE_REF])
        .map_err(|_| WorkspaceError::Missing(BASE_REF.into()))?;
    let range = format!("{base}..HEAD");
    // 0x1f separates fields and 0x1e records: the summary may carry any other character.
    let log = git.out(
        work,
        &["log", "--reverse", "--format=%H%x1f%an%x1f%s%x1e", &range],
    )?;
    let commits: Vec<CommitInfo> = log
        .split('\x1e')
        .filter_map(|rec| {
            let mut f = rec.trim_matches(['\n', '\r']).split('\x1f');
            Some(CommitInfo {
                id: f.next().filter(|i| !i.is_empty())?.to_owned(),
                author: f.next()?.to_owned(),
                summary: f.next()?.to_owned(),
            })
        })
        .collect();
    let bundle = if commits.is_empty() {
        None
    } else {
        let _ = fs::remove_file(out);
        git.run(&GitCmd::new(
            work,
            &[
                "bundle",
                "create",
                "-q",
                &s(out),
                "HEAD",
                &format!("^{base}"),
            ],
        ))?;
        Some(out.to_owned())
    };
    Ok(Back {
        bundle,
        base,
        commits,
        committed_pending,
    })
}

/// At the origin: creates the branch `tandem/<name>` with the bundle's commits. `fetch` only
/// adds objects and a new reference: neither the current branch nor the working tree
/// change. If the name exists, `-2`, `-3`... is appended.
pub fn fetch_back(
    git: &dyn Git,
    root: &Path,
    bundle: &Path,
    name: &str,
) -> Result<String, WorkspaceError> {
    // First to a reference of our own and then `update-ref` with "does not exist" as the old
    // value: the check and the creation are a single atomic operation, so another
    // concurrent call with the same name cannot overwrite the branch.
    let incoming = format!("refs/tandem/incoming/{name}");
    git.run(&GitCmd::new(
        root,
        &[
            "fetch",
            "-q",
            "--no-tags",
            &s(bundle),
            &format!("+HEAD:{incoming}"),
        ],
    ))?;
    let result = create_branch(git, root, name, &incoming);
    let _ = git.run(&GitCmd::new(root, &["update-ref", "-d", &incoming]));
    let branch = result?;
    Ok(branch)
}

fn create_branch(
    git: &dyn Git,
    root: &Path,
    name: &str,
    incoming: &str,
) -> Result<String, WorkspaceError> {
    let sha = git.out(root, &["rev-parse", "--verify", incoming])?;
    let mut n = 1;
    loop {
        let branch = if n == 1 {
            format!("tandem/{name}")
        } else {
            format!("tandem/{name}-{n}")
        };
        let full = format!("refs/heads/{branch}");
        match git.run(&GitCmd::new(root, &["update-ref", &full, &sha, ""])) {
            Ok(_) => return Ok(branch),
            // It already existed (or they won the race): next suffix. Any other failure is real.
            Err(e) => {
                if !git.ok(root, &["show-ref", "--verify", "--quiet", &full]) {
                    return Err(e.into());
                }
                n += 1;
            }
        }
    }
}

/// A safe branch name from a title: ASCII lowercase, digits and `-`.
pub fn slug(title: &str, fallback: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out: String = out.trim_end_matches('-').chars().take(40).collect();
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() {
        fallback.to_owned()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::FakeGit;
    use proptest::prelude::*;

    #[test]
    fn branch_name_comes_from_the_title() {
        assert_eq!(slug("Fix the bug #42!", "x"), "fix-the-bug-42");
        assert_eq!(slug("!!!", "tunnel-1"), "tunnel-1");
        assert_eq!(slug("  --a--  ", "x"), "a");
    }

    proptest! {
        #[test]
        fn the_name_is_always_a_valid_branch_name(title in ".{0,80}") {
            let n = slug(&title, "t");
            prop_assert!(!n.is_empty() && n.len() <= 40);
            prop_assert!(n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
            prop_assert!(!n.starts_with('-') && !n.ends_with('-') && !n.contains("--"));
        }
    }

    #[test]
    fn without_a_repo_the_error_says_so() {
        let git = FakeGit::default();
        git.push(Err("fatal: not a git repository"));
        let dir = tempfile::tempdir().unwrap();
        let e = pack_origin(
            &git,
            dir.path(),
            &dir.path().join("s"),
            &SecretFilter::default(),
        )
        .unwrap_err();
        assert!(matches!(e, WorkspaceError::NotARepo(_)), "{e}");
    }

    #[test]
    fn the_temporary_index_is_deleted_even_if_read_tree_or_add_fails() {
        for failure in ["read-tree", "add"] {
            let git = FakeGit::default();
            git.push(Ok("/r")); // show-toplevel
            git.push(Ok("abc")); // HEAD
            git.push(Ok("main")); // branch
            git.push(Ok("")); // bundle
            if failure == "add" {
                git.push(Ok("")); // read-tree
            }
            git.push(Err("boom"));
            let dir = tempfile::tempdir().unwrap();
            let stage = dir.path().join("s");
            fs::create_dir_all(&stage).unwrap();
            fs::write(stage.join("index.tmp"), b"x").unwrap();
            assert!(pack_origin(&git, dir.path(), &stage, &SecretFilter::default()).is_err());
            assert!(!stage.join("index.tmp").exists(), "after {failure} failed");
        }
    }

    #[test]
    fn without_commits_the_error_says_so() {
        let git = FakeGit::default();
        git.push(Ok("/r"));
        git.push(Err("fatal: Needed a single revision"));
        let dir = tempfile::tempdir().unwrap();
        let e = pack_origin(
            &git,
            dir.path(),
            &dir.path().join("s"),
            &SecretFilter::default(),
        )
        .unwrap_err();
        assert!(matches!(e, WorkspaceError::NoCommits(_)), "{e}");
    }

    #[test]
    fn return_without_new_commits_builds_no_bundle() {
        let git = FakeGit::default();
        git.push(Ok("abc")); // rev-parse base
        git.push(Ok("")); // empty log
        let dir = tempfile::tempdir().unwrap();
        let back = pack_back(&git, dir.path(), None, &dir.path().join("b")).unwrap();
        assert_eq!(back.bundle, None);
        assert_eq!(back.base, "abc");
        assert!(git.calls().iter().all(|c| c.args[0] != "bundle"));
    }

    #[test]
    fn the_log_is_read_even_if_the_summary_has_odd_characters() {
        let git = FakeGit::default();
        git.push(Ok("abc"));
        git.push(Ok("h1\x1fAna\x1fone | two\x1eh2\x1fLuis\x1fü\x1e"));
        let dir = tempfile::tempdir().unwrap();
        let back = pack_back(&git, dir.path(), None, &dir.path().join("b")).unwrap();
        let got: Vec<_> = back.commits.iter().map(|c| (&*c.id, &*c.summary)).collect();
        assert_eq!(got, [("h1", "one | two"), ("h2", "ü")]);
    }

    #[test]
    fn pending_work_is_committed_only_if_asked() {
        let git = FakeGit::default();
        git.push(Ok(" M a.txt")); // status
        let dir = tempfile::tempdir().unwrap();
        let back = pack_back(&git, dir.path(), Some("wip"), &dir.path().join("b")).unwrap();
        assert!(back.committed_pending);
        let cmds: Vec<_> = git.calls().iter().map(|c| c.args[0].clone()).collect();
        assert_eq!(&cmds[..3], ["status", "add", "commit"]);

        let git = FakeGit::default();
        pack_back(&git, dir.path(), None, &dir.path().join("b")).unwrap();
        assert!(git.calls().iter().all(|c| c.args[0] != "commit"));
    }
}
