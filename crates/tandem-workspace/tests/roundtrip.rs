//! Real git repos in temporary folders, with two agents in the same process: outbound with
//! uncommitted changes, commits on the destination and return as a branch.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use tandem_core::Machine;
use tandem_files::mem::MemPath;
use tandem_files::{Files, Options};
use tandem_proto::{Client, ClientKind, Tunnel, WorkspaceSpec};
use tandem_tunnel::{Changes, Host, HostOptions, OpenSpec, Origin, OriginOptions};
use tandem_workspace::{Opened, Workspaces};

/// Runs real git in `dir`; fails the test if git fails.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "protocol.file.allow=always"])
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_WORK_TREE")
        .output()
        .expect("git on the PATH");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_owned()
}

fn init_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "yo@example.com"]);
    git(dir, &["config", "user.name", "Yo"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn write(dir: &Path, rel: &str, content: &[u8]) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn commit_all(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
}

struct Rig {
    _tmp: tempfile::TempDir,
    /// Folder that contains the origin repo.
    base: PathBuf,
    origin: Origin,
    host: Host,
    files_a: Files,
    ws_a: Workspaces,
    ws_b: Workspaces,
    data_b: PathBuf,
    tunnel: Tunnel,
}

fn opts() -> Options {
    Options {
        chunk_size: 64 * 1024,
        attempts: 1,
        retry_delay: Duration::from_millis(1),
    }
}

async fn rig() -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("a");
    let data_b = tmp.path().join("data-b").join("workspaces");
    let origin = Origin::new(OriginOptions::default(), Changes::default());
    let host = Host::new(HostOptions::default(), Changes::default());
    let (files_a, files_b) = (Files::new(opts()), Files::new(opts()));
    let ws_a = Workspaces::new(tmp.path().join("data-a").join("workspaces"));
    let ws_b = Workspaces::new(&data_b);
    let (ws, files) = (ws_b.clone(), files_b.clone());
    let path = MemPath::with_server(&host, move |host, wire| {
        let (ws, files) = (ws.clone(), files.clone());
        Box::pin(async move { ws.serve(&files, &host, wire).await })
    });
    origin.set_link(Some(path.dialer()));
    let tunnel = origin
        .open(OpenSpec {
            title: "Fix the bug".into(),
            project: None,
            origin: Machine::A,
            target: Machine::B,
            opened_by: Client {
                kind: ClientKind::Cli,
                session: "s".into(),
            },
        })
        .await
        .unwrap();
    Rig {
        _tmp: tmp,
        base,
        origin,
        host,
        files_a,
        ws_a,
        ws_b,
        data_b,
        tunnel,
    }
}

impl Rig {
    async fn open(&self, repo: &Path, keep: bool) -> String {
        self.open_with(repo, keep, &[]).await.dir
    }

    async fn open_with(&self, repo: &Path, keep: bool, include: &[&str]) -> Opened {
        let spec = WorkspaceSpec {
            repo_path: repo.to_string_lossy().into_owned(),
            keep,
            include: include.iter().map(|s| (*s).to_owned()).collect(),
        };
        within(
            self.ws_a
                .open(&self.files_a, &self.origin, &self.tunnel, &spec),
        )
        .await
        .unwrap()
    }

    async fn back(
        &self,
        commit: Option<&str>,
        name: Option<&str>,
    ) -> Result<tandem_proto::BringBackReport, tandem_workspace::WorkspaceError> {
        within(self.ws_a.bring_back(
            &self.files_a,
            &self.origin,
            self.tunnel.id,
            commit.map(str::to_owned),
            name.map(str::to_owned),
        ))
        .await
    }
}

async fn within<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(60), f)
        .await
        .expect("the operation did not finish")
}

/// Everything the user sees of the repo: branch, HEAD, status and files.
fn fingerprint(repo: &Path) -> (String, String, String, String, String) {
    (
        git(repo, &["symbolic-ref", "-q", "HEAD"]),
        git(repo, &["rev-parse", "HEAD"]),
        git(repo, &["status", "--porcelain=v1", "-uall"]),
        git(repo, &["diff", "--cached", "--stat"]),
        git(repo, &["diff", "--stat"]),
    )
}

fn tree_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            if e.file_name() == ".git" {
                continue;
            }
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                walk(root, &p, out);
            } else {
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn sample_repo(repo: &Path) {
    init_repo(repo);
    write(repo, ".gitignore", b"target/\n");
    write(repo, "src/lib.rs", b"fn a() {}\n");
    write(repo, "README.md", b"hello\n");
    write(repo, "bin.dat", &[0, 1, 2, 255, 254, 0, 7]);
    commit_all(repo, "initial");
}

#[tokio::test]
async fn the_outbound_carries_the_commit_and_the_uncommitted_changes() {
    let r = rig().await;
    let repo = r.base.join("repo");
    sample_repo(&repo);
    // Pending work of every kind: modified, staged, new untracked, binary, deleted, ignored.
    write(&repo, "src/lib.rs", b"fn a() {}\nfn b() {}\n");
    write(&repo, "README.md", b"hello, world\n");
    git(&repo, &["add", "README.md"]);
    write(&repo, "new/file.txt", "emu\n".as_bytes());
    write(&repo, "bin.dat", &[9, 0, 0, 1, 2, 3]);
    write(&repo, "target/junk", b"must not travel");
    let before = (fingerprint(&repo), tree_files(&repo));

    let dir = r.open(&repo, false).await;
    let work = PathBuf::from(&dir);
    assert_eq!(work, r.data_b.join(r.tunnel.id.to_string()));

    assert_eq!(
        git(&work, &["rev-parse", "HEAD"]),
        git(&repo, &["rev-parse", "HEAD"])
    );
    let sent: Vec<_> = tree_files(&repo)
        .into_iter()
        .filter(|(p, _)| !p.starts_with("target/"))
        .collect();
    assert_eq!(tree_files(&work), sent);
    // What is uncommitted stays uncommitted on the destination.
    assert!(git(&work, &["status", "--porcelain"]).contains("new/"));
    // The origin stayed exactly as it was (including what it had staged).
    assert_eq!(before, (fingerprint(&repo), tree_files(&repo)));
    // The tunnel's processes start in the folder.
    assert_eq!(r.host.get(r.tunnel.id).unwrap().default_cwd(), Some(dir));
}

#[tokio::test]
async fn destination_commits_return_as_a_branch_without_touching_the_origin() {
    let r = rig().await;
    let repo = r.base.join("repo");
    sample_repo(&repo);
    write(&repo, "pending.txt", b"uncommitted at the origin\n");
    let dir = PathBuf::from(r.open(&repo, false).await);

    write(&dir, "src/lib.rs", b"fn a() {}\nfn fix() {}\n");
    commit_all(&dir, "Fix the bug");
    write(&dir, "src/other.rs", b"fn other() {}\n");
    commit_all(&dir, "Add other");
    write(&dir, "extra.txt", b"uncommitted work\n");

    let before = (fingerprint(&repo), tree_files(&repo));
    let report = r.back(Some("Save the pending work"), None).await.unwrap();

    assert_eq!(report.branch.as_deref(), Some("tandem/fix-the-bug"));
    assert!(report.committed_pending);
    let summaries: Vec<_> = report.commits.iter().map(|c| c.summary.as_str()).collect();
    // The origin's pending file (pending.txt) also traveled, so it goes into the last commit.
    assert_eq!(
        summaries,
        ["Fix the bug", "Add other", "Save the pending work"]
    );
    assert!(report.commits.iter().all(|c| c.author == "Yo"));

    // The branch exists at the origin with those commits on top of the usual HEAD.
    let log = git(&repo, &["log", "--format=%s", "main..tandem/fix-the-bug"]);
    assert_eq!(log, "Save the pending work\nAdd other\nFix the bug");
    assert_eq!(
        git(&repo, &["show", "tandem/fix-the-bug:extra.txt"]),
        "uncommitted work"
    );
    // Current branch, HEAD, index and working tree: untouched.
    assert_eq!(before, (fingerprint(&repo), tree_files(&repo)));
    assert_eq!(git(&repo, &["symbolic-ref", "--short", "HEAD"]), "main");
}

#[tokio::test]
async fn without_asking_for_a_commit_pending_work_does_not_return_and_without_commits_there_is_no_branch()
 {
    let r = rig().await;
    let repo = r.base.join("repo");
    sample_repo(&repo);
    let dir = PathBuf::from(r.open(&repo, false).await);

    write(&dir, "loose.txt", b"uncommitted\n");
    let report = r.back(None, None).await.unwrap();
    assert_eq!(report.branch, None);
    assert!(report.commits.is_empty() && !report.committed_pending);
    assert_eq!(git(&repo, &["branch", "--list", "tandem/*"]), "");

    commit_all(&dir, "one");
    let report = r.back(None, Some("My Branch")).await.unwrap();
    assert_eq!(report.branch.as_deref(), Some("tandem/my-branch"));
    // A second return with the same name does not overwrite the previous one.
    let again = r.back(None, Some("My Branch")).await.unwrap();
    assert_eq!(again.branch.as_deref(), Some("tandem/my-branch-2"));
}

#[tokio::test]
async fn works_with_a_git_worktree() {
    let r = rig().await;
    let main = r.base.join("main");
    sample_repo(&main);
    let linked = r.base.join("linked");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "linked-branch",
            linked.to_str().unwrap(),
        ],
    );
    write(&linked, "linked-only.txt", b"x\n");
    let before = (fingerprint(&linked), fingerprint(&main));

    let dir = PathBuf::from(r.open(&linked, false).await);
    assert_eq!(
        git(&dir, &["rev-parse", "HEAD"]),
        git(&linked, &["rev-parse", "HEAD"])
    );
    assert!(dir.join("linked-only.txt").is_file());

    write(&dir, "new.txt", b"y\n");
    let report = r.back(Some("from the worktree"), Some("wt")).await.unwrap();
    assert_eq!(report.branch.as_deref(), Some("tandem/wt"));
    assert_eq!(before, (fingerprint(&linked), fingerprint(&main)));
    // The branch ended up in the shared repo.
    assert!(git(&main, &["branch", "--list", "tandem/wt"]).contains("tandem/wt"));
}

#[tokio::test]
async fn submodules_are_copied_as_they_are() {
    let r = rig().await;
    let lib = r.base.join("lib");
    init_repo(&lib);
    write(&lib, "lib.txt", b"from the library\n");
    commit_all(&lib, "lib");
    let repo = r.base.join("repo");
    sample_repo(&repo);
    git(
        &repo,
        &[
            "submodule",
            "add",
            "-q",
            lib.to_str().unwrap(),
            "vendor/lib",
        ],
    );
    commit_all(&repo, "with submodule");
    // An uncommitted change inside the submodule travels as is.
    write(&repo.join("vendor/lib"), "lib.txt", b"edited\n");

    let dir = PathBuf::from(r.open(&repo, false).await);
    assert_eq!(
        fs::read(dir.join("vendor/lib/lib.txt")).unwrap(),
        b"edited\n"
    );
    assert!(!dir.join("vendor/lib/.git").exists());

    write(&dir, "extra.txt", b"z\n");
    let report = r.back(Some("work"), Some("sub")).await.unwrap();
    assert_eq!(report.commits.len(), 1);
    // The submodule did not become loose files of the repo.
    let tree = git(&repo, &["ls-tree", "-r", "tandem/sub"]);
    assert!(tree.contains("160000 commit"), "{tree}");
    assert!(!tree.contains("vendor/lib/lib.txt"), "{tree}");
}

#[tokio::test]
async fn closing_the_tunnel_cleans_the_workspace_unless_keep_is_requested() {
    for keep in [false, true] {
        let r = rig().await;
        let repo = r.base.join("repo");
        sample_repo(&repo);
        let dir = PathBuf::from(r.open(&repo, keep).await);
        assert!(dir.is_dir());
        r.host.close(r.tunnel.id).unwrap();
        // The deletion runs on its own thread.
        for _ in 0..100 {
            if !dir.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(dir.exists(), keep, "keep = {keep}");
        assert!(r.ws_b.dir_of(r.tunnel.id).is_none());
    }
}

#[tokio::test]
async fn errors_are_clear() {
    let r = rig().await;
    let nothing = r.base.join("not-a-repo");
    fs::create_dir_all(&nothing).unwrap();
    let spec = WorkspaceSpec {
        repo_path: nothing.to_string_lossy().into_owned(),
        keep: false,
        include: vec![],
    };
    let e = r
        .ws_a
        .open(&r.files_a, &r.origin, &r.tunnel, &spec)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("is not inside a git repo"), "{e}");

    let empty_repo = r.base.join("empty_repo");
    init_repo(&empty_repo);
    let spec = WorkspaceSpec {
        repo_path: empty_repo.to_string_lossy().into_owned(),
        keep: false,
        include: vec![],
    };
    let e = r
        .ws_a
        .open(&r.files_a, &r.origin, &r.tunnel, &spec)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("has no commits"), "{e}");

    // Without opening a workspace, there is nowhere to return to.
    let e = r.back(None, None).await.unwrap_err();
    assert!(e.to_string().contains("has no workspace"), "{e}");
}

/// Hooks and `core.fsmonitor` inherited from the global configuration are foreign code running
/// on the destination (perhaps elevated): the crate must not run them on the way out or on the way back.
/// It runs in a child process so `GIT_CONFIG_GLOBAL` can be set without touching the environment of
/// the other tests.
#[test]
fn inherited_hooks_and_fsmonitor_are_not_run() {
    let tmp = tempfile::tempdir().unwrap();
    let hooks = tmp.path().join("hooks");
    let sentinels = tmp.path().join("sentinels");
    fs::create_dir_all(&hooks).unwrap();
    fs::create_dir_all(&sentinels).unwrap();
    let fwd = |p: &Path| p.to_string_lossy().replace('\\', "/");
    // They only leave a trace inside the folders of the two machines of the inner test.
    let script = |name: &str| {
        format!(
            "#!/bin/sh\ncase \"$PWD\" in */data-b/*|*/a/*) echo x > \"{}/{name}\" ;; esac\nexit 0\n",
            fwd(&sentinels)
        )
    };
    for name in ["post-commit", "post-checkout", "fsmonitor"] {
        let p = hooks.join(name);
        fs::write(&p, script(name)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let global = tmp.path().join("gitconfig");
    fs::write(
        &global,
        format!(
            "[core]\n\thooksPath = {}\n\tfsmonitor = {}\n",
            fwd(&hooks),
            fwd(&hooks.join("fsmonitor"))
        ),
    )
    .unwrap();

    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "inherited_hooks_inner", "--include-ignored"])
        .env("GIT_CONFIG_GLOBAL", &global)
        .env("TANDEM_SENTINELS", &sentinels)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The sentinels the hooks left since the last time; deletes them.
fn sentinels_found() -> Vec<String> {
    let dir = PathBuf::from(std::env::var("TANDEM_SENTINELS").unwrap());
    let mut found: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    for n in &found {
        let _ = fs::remove_file(dir.join(n));
    }
    found
}

#[tokio::test]
#[ignore = "run by `inherited_hooks_and_fsmonitor_are_not_run` in a child"]
async fn inherited_hooks_inner() {
    let r = rig().await;
    let repo = r.base.join("repo");
    sample_repo(&repo);
    write(&repo, "pending.txt", b"uncommitted\n");
    // Control: if the global config were not applied, the test would prove nothing.
    assert!(
        !sentinels_found().is_empty(),
        "the inherited configuration was not applied: the test is useless"
    );

    let dir = PathBuf::from(r.open(&repo, false).await);
    assert_eq!(sentinels_found(), Vec::<String>::new(), "on the way out");

    // The destination commit goes without hooks on our part: what is measured is the crate.
    write(&dir, "new.txt", b"done on the destination\n");
    let quiet = |args: &[&str]| {
        let mut a = vec![
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
        ];
        a.extend_from_slice(args);
        git(&dir, &a)
    };
    quiet(&["add", "-A"]);
    quiet(&["commit", "-q", "-m", "On the destination"]);
    write(&dir, "more.txt", b"pending\n");
    sentinels_found();

    let report = r.back(Some("Pending"), None).await.unwrap();
    assert!(report.branch.is_some());
    assert_eq!(sentinels_found(), Vec::<String>::new(), "on the way back");
}

/// A repo with credentials that git does not know are such: none of this is in `.gitignore`.
fn repo_with_credentials(repo: &Path) {
    sample_repo(repo);
    write(repo, ".env", b"DB_PASSWORD=hunter2\n");
    write(repo, "config/.env.local", b"X=1\n");
    write(
        repo,
        "deploy/id_ed25519",
        b"-----BEGIN OPENSSH PRIVATE KEY-----\n",
    );
    write(repo, ".ssh/config", b"Host x\n");
    write(repo, "certs/server.pem", b"pem\n");
    write(
        repo,
        ".npmrc",
        b"//registry.npmjs.org/:_authToken=npm_abc\n",
    );
    write(
        repo,
        "sub/.npmrc",
        b"registry=https://registry.npmjs.org/\n",
    );
    write(repo, "src/new.rs", b"fn n() {}\n");
}

#[tokio::test]
async fn credentials_do_not_reach_the_destination_and_the_report_says_so() {
    let r = rig().await;
    let repo = r.base.join("repo");
    repo_with_credentials(&repo);
    let before = (fingerprint(&repo), tree_files(&repo));

    let opened = r.open_with(&repo, false, &[]).await;
    let work = PathBuf::from(&opened.dir);
    let got: Vec<String> = tree_files(&work).into_iter().map(|(p, _)| p).collect();
    for secret in [
        ".env",
        "config/.env.local",
        "deploy/id_ed25519",
        ".ssh/config",
        "certs/server.pem",
        ".npmrc",
    ] {
        assert!(
            !got.contains(&secret.to_owned()),
            "arrived {secret}: {got:?}"
        );
        assert!(
            opened.skipped.contains(&secret.to_owned()),
            "the report does not mention {secret}: {:?}",
            opened.skipped
        );
    }
    // The ordinary ones do arrive, also a .npmrc without a token and new untracked files.
    for ok in ["src/new.rs", "sub/.npmrc", "README.md"] {
        assert!(got.contains(&ok.to_owned()), "missing {ok}: {got:?}");
    }
    // The origin repo is left intact.
    assert_eq!(before, (fingerprint(&repo), tree_files(&repo)));
}

#[tokio::test]
async fn include_sends_only_the_named_file() {
    let r = rig().await;
    let repo = r.base.join("repo");
    repo_with_credentials(&repo);
    let opened = r.open_with(&repo, false, &["config/.env.local"]).await;
    let got: Vec<String> = tree_files(Path::new(&opened.dir))
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    assert!(got.contains(&"config/.env.local".to_owned()));
    assert!(!got.contains(&".env".to_owned()));
    assert!(!opened.skipped.contains(&"config/.env.local".to_owned()));
    assert!(opened.skipped.contains(&".env".to_owned()));
}

#[tokio::test]
async fn a_modified_credential_returns_to_its_committed_version() {
    let r = rig().await;
    let repo = r.base.join("repo");
    sample_repo(&repo);
    write(&repo, "app.pem", b"old\n");
    commit_all(&repo, "with pem");
    write(&repo, "app.pem", b"new secret\n");
    let opened = r.open_with(&repo, false, &[]).await;
    assert!(opened.skipped.contains(&"app.pem".to_owned()));
    // The change is not sent (nor is the file deleted): what was already committed stays.
    let pem = fs::read(Path::new(&opened.dir).join("app.pem")).unwrap();
    assert_eq!(pem, b"old\n");
}

#[tokio::test]
async fn the_destination_repo_has_no_remotes_or_credential_helper() {
    let r = rig().await;
    let repo = r.base.join("repo");
    sample_repo(&repo);
    // The origin does have them: they do not need to cross.
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://ghp_abc123@github.com/u/r.git",
        ],
    );
    git(&repo, &["config", "credential.helper", "store"]);
    let dir = r.open(&repo, false).await;
    let work = Path::new(&dir);
    assert_eq!(git(work, &["remote"]), "");
    let config = fs::read_to_string(work.join(".git/config")).unwrap();
    assert!(!config.contains("credential"), "{config}");
    assert!(!config.contains("ghp_"), "{config}");
    // Not even the bundle left the URL in any reference.
    let refs = git(work, &["for-each-ref"]);
    assert!(!refs.contains("origin/"), "{refs}");
}

#[tokio::test]
async fn credentials_in_history_are_reported_even_if_no_longer_present() {
    let r = rig().await;
    let repo = r.base.join("repo");
    sample_repo(&repo);
    write(&repo, "deploy/id_ed25519", b"key\n");
    commit_all(&repo, "with key");
    // It is deleted from the tree, but remains in the history that travels in the bundle.
    fs::remove_file(repo.join("deploy/id_ed25519")).unwrap();
    commit_all(&repo, "without key");
    let opened = r.open_with(&repo, false, &[]).await;
    assert_eq!(opened.history_secrets, vec!["deploy/id_ed25519".to_owned()]);
    assert!(opened.skipped.is_empty(), "{:?}", opened.skipped);
}

#[tokio::test]
async fn a_clean_history_does_not_warn() {
    let r = rig().await;
    let repo = r.base.join("repo");
    sample_repo(&repo);
    let opened = r.open_with(&repo, false, &[]).await;
    assert!(opened.history_secrets.is_empty());
}
