//! Repos de git reales en carpetas temporales, con dos agentes en el mismo proceso: ida con
//! cambios sin commit, commits en la destino y vuelta como rama.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use tandem_core::Machine;
use tandem_files::mem::MemPath;
use tandem_files::{Files, Options};
use tandem_proto::{Client, ClientKind, Tunnel, WorkspaceSpec};
use tandem_tunnel::{Changes, Host, HostOptions, OpenSpec, Origin, OriginOptions};
use tandem_workspace::Workspaces;

/// Corre git de verdad en `dir`; falla la prueba si git falla.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "protocol.file.allow=always"])
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_WORK_TREE")
        .output()
        .expect("git en el PATH");
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
    /// Carpeta que contiene el repo de origen.
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
    let data_b = tmp.path().join("datos-b").join("workspaces");
    let origin = Origin::new(OriginOptions::default(), Changes::default());
    let host = Host::new(HostOptions::default(), Changes::default());
    let (files_a, files_b) = (Files::new(opts()), Files::new(opts()));
    let ws_a = Workspaces::new(tmp.path().join("datos-a").join("workspaces"));
    let ws_b = Workspaces::new(&data_b);
    let (ws, files) = (ws_b.clone(), files_b.clone());
    let path = MemPath::with_server(&host, move |host, wire| {
        let (ws, files) = (ws.clone(), files.clone());
        Box::pin(async move { ws.serve(&files, &host, wire).await })
    });
    origin.set_link(Some(path.dialer()));
    let tunnel = origin
        .open(OpenSpec {
            title: "Arregla el bug".into(),
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
        let spec = WorkspaceSpec {
            repo_path: repo.to_string_lossy().into_owned(),
            keep,
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
        .expect("la operación no terminó")
}

/// Todo lo que el usuario ve del repo: rama, HEAD, estado y archivos.
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
    write(repo, "README.md", b"hola\n");
    write(repo, "bin.dat", &[0, 1, 2, 255, 254, 0, 7]);
    commit_all(repo, "inicial");
}

#[tokio::test]
async fn la_ida_lleva_el_commit_y_los_cambios_sin_commit() {
    let r = rig().await;
    let repo = r.base.join("repo");
    sample_repo(&repo);
    // Pendiente de todo tipo: modificado, preparado, nuevo sin seguimiento, binario, borrado, ignorado.
    write(&repo, "src/lib.rs", b"fn a() {}\nfn b() {}\n");
    write(&repo, "README.md", b"hola, mundo\n");
    git(&repo, &["add", "README.md"]);
    write(&repo, "nuevo/archivo.txt", "ñandú\n".as_bytes());
    write(&repo, "bin.dat", &[9, 0, 0, 1, 2, 3]);
    write(&repo, "target/basura", b"no debe viajar");
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
    // Lo sin commit sigue sin commit en la destino.
    assert!(git(&work, &["status", "--porcelain"]).contains("nuevo/"));
    // El origen quedó exactamente como estaba (incluido lo que tenía preparado).
    assert_eq!(before, (fingerprint(&repo), tree_files(&repo)));
    // Los procesos del túnel arrancan en la carpeta.
    assert_eq!(r.host.get(r.tunnel.id).unwrap().default_cwd(), Some(dir));
}

#[tokio::test]
async fn los_commits_de_la_destino_vuelven_como_rama_sin_tocar_el_origen() {
    let r = rig().await;
    let repo = r.base.join("repo");
    sample_repo(&repo);
    write(&repo, "pendiente.txt", b"sin commit en el origen\n");
    let dir = PathBuf::from(r.open(&repo, false).await);

    write(&dir, "src/lib.rs", b"fn a() {}\nfn arreglo() {}\n");
    commit_all(&dir, "Arregla el bug");
    write(&dir, "src/otro.rs", b"fn otro() {}\n");
    commit_all(&dir, "Agrega otro");
    write(&dir, "sobra.txt", b"trabajo sin commit\n");

    let before = (fingerprint(&repo), tree_files(&repo));
    let report = r.back(Some("Guarda lo pendiente"), None).await.unwrap();

    assert_eq!(report.branch.as_deref(), Some("tandem/arregla-el-bug"));
    assert!(report.committed_pending);
    let summaries: Vec<_> = report.commits.iter().map(|c| c.summary.as_str()).collect();
    // El pendiente del origen (pendiente.txt) también viajó, así que entra en el último commit.
    assert_eq!(
        summaries,
        ["Arregla el bug", "Agrega otro", "Guarda lo pendiente"]
    );
    assert!(report.commits.iter().all(|c| c.author == "Yo"));

    // La rama existe en el origen con esos commits sobre el HEAD de siempre.
    let log = git(
        &repo,
        &["log", "--format=%s", "main..tandem/arregla-el-bug"],
    );
    assert_eq!(log, "Guarda lo pendiente\nAgrega otro\nArregla el bug");
    assert_eq!(
        git(&repo, &["show", "tandem/arregla-el-bug:sobra.txt"]),
        "trabajo sin commit"
    );
    // Rama actual, HEAD, índice y árbol de trabajo: intactos.
    assert_eq!(before, (fingerprint(&repo), tree_files(&repo)));
    assert_eq!(git(&repo, &["symbolic-ref", "--short", "HEAD"]), "main");
}

#[tokio::test]
async fn sin_pedir_commit_lo_pendiente_no_vuelve_y_sin_commits_no_hay_rama() {
    let r = rig().await;
    let repo = r.base.join("repo");
    sample_repo(&repo);
    let dir = PathBuf::from(r.open(&repo, false).await);

    write(&dir, "suelto.txt", b"sin commit\n");
    let report = r.back(None, None).await.unwrap();
    assert_eq!(report.branch, None);
    assert!(report.commits.is_empty() && !report.committed_pending);
    assert_eq!(git(&repo, &["branch", "--list", "tandem/*"]), "");

    commit_all(&dir, "uno");
    let report = r.back(None, Some("Mi Rama")).await.unwrap();
    assert_eq!(report.branch.as_deref(), Some("tandem/mi-rama"));
    // Una segunda vuelta con el mismo nombre no pisa la anterior.
    let again = r.back(None, Some("Mi Rama")).await.unwrap();
    assert_eq!(again.branch.as_deref(), Some("tandem/mi-rama-2"));
}

#[tokio::test]
async fn funciona_con_un_worktree_de_git() {
    let r = rig().await;
    let main = r.base.join("principal");
    sample_repo(&main);
    let linked = r.base.join("enlazado");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "rama-enlazada",
            linked.to_str().unwrap(),
        ],
    );
    write(&linked, "solo-enlazado.txt", b"x\n");
    let before = (fingerprint(&linked), fingerprint(&main));

    let dir = PathBuf::from(r.open(&linked, false).await);
    assert_eq!(
        git(&dir, &["rev-parse", "HEAD"]),
        git(&linked, &["rev-parse", "HEAD"])
    );
    assert!(dir.join("solo-enlazado.txt").is_file());

    write(&dir, "nuevo.txt", b"y\n");
    let report = r.back(Some("desde el worktree"), Some("wt")).await.unwrap();
    assert_eq!(report.branch.as_deref(), Some("tandem/wt"));
    assert_eq!(before, (fingerprint(&linked), fingerprint(&main)));
    // La rama quedó en el repo compartido.
    assert!(git(&main, &["branch", "--list", "tandem/wt"]).contains("tandem/wt"));
}

#[tokio::test]
async fn los_submodulos_se_copian_como_estan() {
    let r = rig().await;
    let lib = r.base.join("lib");
    init_repo(&lib);
    write(&lib, "lib.txt", b"de la libreria\n");
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
    commit_all(&repo, "con submodulo");
    // Un cambio sin commit dentro del submódulo viaja tal cual.
    write(&repo.join("vendor/lib"), "lib.txt", b"editada\n");

    let dir = PathBuf::from(r.open(&repo, false).await);
    assert_eq!(
        fs::read(dir.join("vendor/lib/lib.txt")).unwrap(),
        b"editada\n"
    );
    assert!(!dir.join("vendor/lib/.git").exists());

    write(&dir, "extra.txt", b"z\n");
    let report = r.back(Some("trabajo"), Some("sub")).await.unwrap();
    assert_eq!(report.commits.len(), 1);
    // El submódulo no se convirtió en archivos sueltos del repo.
    let tree = git(&repo, &["ls-tree", "-r", "tandem/sub"]);
    assert!(tree.contains("160000 commit"), "{tree}");
    assert!(!tree.contains("vendor/lib/lib.txt"), "{tree}");
}

#[tokio::test]
async fn cerrar_el_tunel_limpia_el_workspace_salvo_que_se_pida_conservarlo() {
    for keep in [false, true] {
        let r = rig().await;
        let repo = r.base.join("repo");
        sample_repo(&repo);
        let dir = PathBuf::from(r.open(&repo, keep).await);
        assert!(dir.is_dir());
        r.host.close(r.tunnel.id).unwrap();
        // El borrado corre en su propio hilo.
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
async fn los_errores_son_claros() {
    let r = rig().await;
    let nada = r.base.join("no-es-repo");
    fs::create_dir_all(&nada).unwrap();
    let spec = WorkspaceSpec {
        repo_path: nada.to_string_lossy().into_owned(),
        keep: false,
    };
    let e = r
        .ws_a
        .open(&r.files_a, &r.origin, &r.tunnel, &spec)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("no está dentro de un repo"), "{e}");

    let vacio = r.base.join("vacio");
    init_repo(&vacio);
    let spec = WorkspaceSpec {
        repo_path: vacio.to_string_lossy().into_owned(),
        keep: false,
    };
    let e = r
        .ws_a
        .open(&r.files_a, &r.origin, &r.tunnel, &spec)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("ningún commit"), "{e}");

    // Sin abrir workspace, no hay a dónde volver.
    let e = r.back(None, None).await.unwrap_err();
    assert!(e.to_string().contains("no tiene workspace"), "{e}");
}
