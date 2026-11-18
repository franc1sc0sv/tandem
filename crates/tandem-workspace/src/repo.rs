//! Las operaciones sobre repos, bloqueantes y detrás de [`Git`].
//!
//! Ida (origen): un bundle del `HEAD` y un parche con todo lo pendiente (también lo nuevo
//! sin seguimiento, nunca lo ignorado), calculado con un índice temporal para no tocar el
//! del usuario. Destino: init + fetch del bundle + parche. Vuelta: commit opcional de lo
//! pendiente, bundle de los commits nuevos y, en el origen, una rama nueva con `fetch`
//! (no toca la rama actual ni el árbol de trabajo).

use std::fs;
use std::path::{Path, PathBuf};

use tandem_proto::CommitInfo;

use crate::error::WorkspaceError;
use crate::git::{Git, GitCmd, GitExt};

pub const BUNDLE: &str = "repo.bundle";
pub const PATCH: &str = "changes.patch";
pub const SUBMODULES: &str = "submodules";
/// Quién es el usuario en el origen (nombre y correo, uno por línea): los commits de la
/// destino salen a su nombre.
pub const IDENTITY: &str = "identity.txt";
pub const BACK_BUNDLE: &str = "back.bundle";
/// El commit del que parte el trabajo en la destino.
pub const BASE_REF: &str = "refs/tandem/base";
/// La rama de la destino.
pub const WORK_BRANCH: &str = "tandem/work";

/// Lo que el origen dejó en la carpeta de envío.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packed {
    pub root: PathBuf,
    /// El `HEAD` del origen: de él parte el trabajo y a él se vuelve.
    pub head: String,
    pub branch: Option<String>,
    pub has_patch: bool,
    pub submodules: Vec<String>,
}

/// Borra el archivo al salir, por el camino que sea (un `?` incluido).
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

/// Prepara en `stage` lo que viaja a la destino.
pub fn pack_origin(git: &dyn Git, repo: &Path, stage: &Path) -> Result<Packed, WorkspaceError> {
    fs::create_dir_all(stage).map_err(|e| io("crear la carpeta de envío", e))?;
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

    // Índice aparte: `add -A` en el del usuario le cambiaría lo que tiene preparado.
    let index = stage.join("index.tmp");
    let _cleanup = RemoveOnDrop(&index);
    let with_index = |args: &[&str]| GitCmd::new(&root, args).env("GIT_INDEX_FILE", s(&index));
    git.run(&with_index(&["read-tree", "HEAD"]))?;
    git.run(&with_index(&["add", "-A"]))?;
    // Los submódulos viajan copiados (abajo), no como parche.
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
        fs::write(stage.join(PATCH), &patch).map_err(|e| io("escribir el parche", e))?;
    }

    let name = git.out(&root, &["config", "user.name"]).unwrap_or_default();
    let email = git
        .out(&root, &["config", "user.email"])
        .unwrap_or_default();
    if !name.is_empty() && !email.is_empty() {
        fs::write(stage.join(IDENTITY), format!("{name}\n{email}\n"))
            .map_err(|e| io("escribir la identidad", e))?;
    }
    let submodules = submodule_paths(git, &root)?;
    for sub in &submodules {
        copy_tree(&root.join(sub), &stage.join(SUBMODULES).join(sub))?;
    }
    Ok(Packed {
        root,
        head,
        branch,
        has_patch,
        submodules,
    })
}

/// Las rutas de los submódulos del `HEAD` (entradas `160000` del índice).
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

/// Copia un árbol sin su `.git` ni enlaces simbólicos.
pub fn copy_tree(from: &Path, to: &Path) -> Result<(), WorkspaceError> {
    fs::create_dir_all(to).map_err(|e| io("crear una carpeta", e))?;
    let Ok(entries) = fs::read_dir(from) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry.map_err(|e| io("leer una carpeta", e))?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let kind = entry.file_type().map_err(|e| io("leer una entrada", e))?;
        let dest = to.join(&name);
        if kind.is_dir() {
            copy_tree(&entry.path(), &dest)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &dest).map_err(|e| io("copiar un archivo", e))?;
        }
    }
    Ok(())
}

/// En la destino: arma `work` con el bundle y el parche que dejó el origen en `stage`.
pub fn checkout(git: &dyn Git, stage: &Path, work: &Path) -> Result<(), WorkspaceError> {
    let bundle = stage.join(BUNDLE);
    if !bundle.is_file() {
        return Err(WorkspaceError::Missing(s(&bundle)));
    }
    if work.exists() {
        fs::remove_dir_all(work).map_err(|e| io("limpiar el workspace", e))?;
    }
    fs::create_dir_all(work).map_err(|e| io("crear el workspace", e))?;
    git.run(&GitCmd::new(work, &["init", "-q", "--template="]))?;
    let fetch = format!("+HEAD:{BASE_REF}");
    git.run(&GitCmd::new(
        work,
        &["fetch", "-q", "--no-tags", &s(&bundle), &fetch],
    ))?;
    git.run(&GitCmd::new(
        work,
        &["checkout", "-q", "-b", WORK_BRANCH, BASE_REF],
    ))?;
    // Los commits salen a nombre del usuario del origen; si no se sabe, una identidad local
    // solo si hace falta (sin ella git no puede hacer commit).
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

/// Lo que `bring_back` saca de la destino.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Back {
    pub bundle: Option<PathBuf>,
    pub base: String,
    pub commits: Vec<CommitInfo>,
    pub committed_pending: bool,
}

/// En la destino: con `message`, hace commit de lo pendiente; después empaqueta los
/// commits que no estaban en el origen en `out`.
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
    // 0x1f separa campos y 0x1e registros: el resumen puede llevar cualquier otro carácter.
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

/// En el origen: crea la rama `tandem/<name>` con los commits del bundle. `fetch` solo
/// agrega objetos y una referencia nueva: ni la rama actual ni el árbol de trabajo
/// cambian. Si el nombre existe se le agrega `-2`, `-3`…
pub fn fetch_back(
    git: &dyn Git,
    root: &Path,
    bundle: &Path,
    name: &str,
) -> Result<String, WorkspaceError> {
    // Primero a una referencia propia y después `update-ref` con "no existe" como valor
    // viejo: la comprobación y la creación son una sola operación atómica, así que otra
    // llamada concurrente con el mismo nombre no puede pisar la rama.
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
            // Ya existía (o la ganaron): siguiente sufijo. Otro fallo es real.
            Err(e) => {
                if !git.ok(root, &["show-ref", "--verify", "--quiet", &full]) {
                    return Err(e.into());
                }
                n += 1;
            }
        }
    }
}

/// Un nombre de rama seguro a partir de un título: minúsculas ASCII, dígitos y `-`.
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
    fn el_nombre_de_rama_sale_del_titulo() {
        assert_eq!(slug("Arregla el bug #42!", "x"), "arregla-el-bug-42");
        assert_eq!(slug("¡¡¡", "tunel-1"), "tunel-1");
        assert_eq!(slug("  --a--  ", "x"), "a");
    }

    proptest! {
        #[test]
        fn el_nombre_siempre_es_un_nombre_de_rama_valido(title in ".{0,80}") {
            let n = slug(&title, "t");
            prop_assert!(!n.is_empty() && n.len() <= 40);
            prop_assert!(n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
            prop_assert!(!n.starts_with('-') && !n.ends_with('-') && !n.contains("--"));
        }
    }

    #[test]
    fn sin_repo_el_error_lo_dice() {
        let git = FakeGit::default();
        git.push(Err("fatal: not a git repository"));
        let dir = tempfile::tempdir().unwrap();
        let e = pack_origin(&git, dir.path(), &dir.path().join("s")).unwrap_err();
        assert!(matches!(e, WorkspaceError::NotARepo(_)), "{e}");
    }

    #[test]
    fn el_indice_temporal_se_borra_aunque_falle_read_tree_o_add() {
        for fallo in ["read-tree", "add"] {
            let git = FakeGit::default();
            git.push(Ok("/r")); // show-toplevel
            git.push(Ok("abc")); // HEAD
            git.push(Ok("main")); // rama
            git.push(Ok("")); // bundle
            if fallo == "add" {
                git.push(Ok("")); // read-tree
            }
            git.push(Err("boom"));
            let dir = tempfile::tempdir().unwrap();
            let stage = dir.path().join("s");
            fs::create_dir_all(&stage).unwrap();
            fs::write(stage.join("index.tmp"), b"x").unwrap();
            assert!(pack_origin(&git, dir.path(), &stage).is_err());
            assert!(!stage.join("index.tmp").exists(), "tras fallar {fallo}");
        }
    }

    #[test]
    fn sin_commits_el_error_lo_dice() {
        let git = FakeGit::default();
        git.push(Ok("/r"));
        git.push(Err("fatal: Needed a single revision"));
        let dir = tempfile::tempdir().unwrap();
        let e = pack_origin(&git, dir.path(), &dir.path().join("s")).unwrap_err();
        assert!(matches!(e, WorkspaceError::NoCommits(_)), "{e}");
    }

    #[test]
    fn la_vuelta_sin_commits_nuevos_no_arma_bundle() {
        let git = FakeGit::default();
        git.push(Ok("abc")); // rev-parse base
        git.push(Ok("")); // log vacío
        let dir = tempfile::tempdir().unwrap();
        let back = pack_back(&git, dir.path(), None, &dir.path().join("b")).unwrap();
        assert_eq!(back.bundle, None);
        assert_eq!(back.base, "abc");
        assert!(git.calls().iter().all(|c| c.args[0] != "bundle"));
    }

    #[test]
    fn el_log_se_lee_aunque_el_resumen_lleve_caracteres_raros() {
        let git = FakeGit::default();
        git.push(Ok("abc"));
        git.push(Ok("h1\x1fAna\x1funo | dos\x1eh2\x1fLuis\x1fñ\x1e"));
        let dir = tempfile::tempdir().unwrap();
        let back = pack_back(&git, dir.path(), None, &dir.path().join("b")).unwrap();
        let got: Vec<_> = back.commits.iter().map(|c| (&*c.id, &*c.summary)).collect();
        assert_eq!(got, [("h1", "uno | dos"), ("h2", "ñ")]);
    }

    #[test]
    fn lo_pendiente_solo_se_commitea_si_se_pide() {
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
