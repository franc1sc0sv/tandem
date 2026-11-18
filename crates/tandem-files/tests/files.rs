//! Archivos y carpetas entre dos agentes: llegan idénticos, y tras un corte en cualquier
//! fragmento se reanuda sin repetir lo que ya llegó.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use common::{Pair, TIMEOUT, pair, snapshot};
use proptest::prelude::*;
use tandem_files::{Sink, Source};
use tandem_proto::TransferProgress;

const CHUNK: u32 = 8;

#[derive(Debug, Clone)]
enum Node {
    File(Vec<u8>),
    Dir(BTreeMap<String, Vec<u8>>),
}

fn name() -> impl Strategy<Value = String> {
    // Minúsculas y sin punto ni espacio al final: Windows y macOS no distinguen mayúsculas y no
    // guardan esos nombres.
    "f[a-z0-9 ñáéü_-]{0,4}x"
}

fn content() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        Just(Vec::new()),
        proptest::collection::vec(any::<u8>(), 1..60),
    ]
}

fn tree() -> impl Strategy<Value = BTreeMap<String, Node>> {
    let node = prop_oneof![
        content().prop_map(Node::File),
        proptest::collection::btree_map(name(), content(), 0..4).prop_map(Node::Dir),
    ];
    proptest::collection::btree_map(name(), node, 1..5)
}

fn write_tree(root: &Path, tree: &BTreeMap<String, Node>) {
    fs::create_dir_all(root).unwrap();
    for (n, node) in tree {
        match node {
            Node::File(b) => fs::write(root.join(n), b).unwrap(),
            Node::Dir(files) => {
                fs::create_dir_all(root.join(n)).unwrap();
                for (f, b) in files {
                    fs::write(root.join(n).join(f), b).unwrap();
                }
            }
        }
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(TIMEOUT, f).await.unwrap()
}

fn chunks_missing(dst: &Path, src: &Path) -> usize {
    let manifest = Source::scan(src, CHUNK).unwrap().manifest().clone();
    let sink = Sink::open(dst, manifest).unwrap();
    sink.plan().unwrap().iter().map(|n| n.chunks.len()).sum()
}

/// Lo que falta una vez que el stream cortado dejó de escribir (lo que iba en vuelo todavía
/// puede aterrizar un instante después del corte).
async fn settled_missing(dst: &Path, src: &Path) -> usize {
    let mut last = chunks_missing(dst, src);
    let mut stable = 0;
    while stable < 5 {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let now = chunks_missing(dst, src);
        if now == last {
            stable += 1;
        } else {
            (last, stable) = (now, 0);
        }
    }
    last
}

fn total_chunks(src: &Path) -> usize {
    use tandem_proto::EntryKind;
    Source::scan(src, CHUNK)
        .unwrap()
        .manifest()
        .entries
        .iter()
        .map(|e| match &e.kind {
            EntryKind::File { chunks, .. } => chunks.len(),
            EntryKind::Dir => 0,
        })
        .sum()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    /// Cualquier árbol (vacíos, binarios, espacios y acentos) llega idéntico.
    #[test]
    fn el_arbol_llega_identico(t in tree()) {
        rt().block_on(async {
            let d = tempfile::tempdir().unwrap();
            let (src, dst) = (d.path().join("origen"), d.path().join("destino"));
            write_tree(&src, &t);
            let p = pair(CHUNK).await;
            let r = within(p.files_a.put(&p.origin, p.tunnel, &src, dst.to_str().unwrap()))
                .await
                .unwrap();
            prop_assert_eq!(snapshot(&src), snapshot(&dst));
            prop_assert_eq!(r.bytes, snapshot(&src).values().flatten().map(|b| b.len() as u64).sum::<u64>());
            Ok(())
        })?;
    }

    /// Un corte en cualquier fragmento y la reanudación no repite lo ya recibido.
    #[test]
    fn subir_se_reanuda_sin_repetir(t in tree(), cut in 0u64..12) {
        rt().block_on(async {
            let d = tempfile::tempdir().unwrap();
            let (src, dst) = (d.path().join("origen"), d.path().join("destino"));
            write_tree(&src, &t);
            let dst_s = dst.to_str().unwrap();
            let p = pair(CHUNK).await;
            p.path.cut_after_pieces(cut);
            let first = within(p.files_a.put(&p.origin, p.tunnel, &src, dst_s)).await;
            if first.is_ok() {
                prop_assert_eq!(snapshot(&src), snapshot(&dst));
                return Ok(());
            }
            // Un camino nuevo contra la misma destino.
            let again = tandem_files::mem::MemPath::new(&p.host, &p.files_b);
            p.origin.set_link(Some(again.dialer()));
            let missing = settled_missing(&dst, &src).await;
            prop_assert!(missing <= total_chunks(&src));
            within(p.files_a.put(&p.origin, p.tunnel, &src, dst_s)).await.unwrap();
            prop_assert_eq!(again.pieces_passed() as usize, missing);
            prop_assert_eq!(snapshot(&src), snapshot(&dst));
            Ok(())
        })?;
    }

    /// Lo mismo bajando de la destino.
    #[test]
    fn bajar_se_reanuda_sin_repetir(t in tree(), cut in 0u64..12) {
        rt().block_on(async {
            let d = tempfile::tempdir().unwrap();
            let (remote, local) = (d.path().join("remoto"), d.path().join("local"));
            write_tree(&remote, &t);
            let remote_s = remote.to_str().unwrap();
            let p = pair(CHUNK).await;
            p.path.cut_after_pieces(cut);
            let first = within(p.files_a.get(&p.origin, p.tunnel, remote_s, &local)).await;
            if first.is_ok() {
                prop_assert_eq!(snapshot(&remote), snapshot(&local));
                return Ok(());
            }
            let again = tandem_files::mem::MemPath::new(&p.host, &p.files_b);
            p.origin.set_link(Some(again.dialer()));
            let missing = settled_missing(&local, &remote).await;
            within(p.files_a.get(&p.origin, p.tunnel, remote_s, &local)).await.unwrap();
            prop_assert_eq!(again.pieces_passed() as usize, missing);
            prop_assert_eq!(snapshot(&remote), snapshot(&local));
            Ok(())
        })?;
    }
}

#[tokio::test]
async fn el_corte_deja_la_copia_a_medias_y_la_reanudacion_manda_el_resto() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("grande.bin");
    let data: Vec<u8> = (0..80u8).collect(); // 10 fragmentos de 8
    fs::write(&src, &data).unwrap();
    let dst = d.path().join("copia.bin");
    let dst_s = dst.to_str().unwrap();
    let p = pair(CHUNK).await;
    p.path.cut_after_pieces(4);
    let first = within(p.files_a.put(&p.origin, p.tunnel, &src, dst_s)).await;
    assert!(first.is_err(), "con el corte no puede terminar");
    assert!(!dst.exists(), "sin terminar no hay archivo final");
    let again = tandem_files::mem::MemPath::new(&p.host, &p.files_b);
    p.origin.set_link(Some(again.dialer()));
    let missing = settled_missing(&dst, &src).await;
    assert!(missing <= 10, "faltan {missing}");
    let r = within(p.files_a.put(&p.origin, p.tunnel, &src, dst_s))
        .await
        .unwrap();
    assert_eq!(again.pieces_passed() as usize, missing);
    assert_eq!(r.sent, missing as u64 * 8);
    assert_eq!(fs::read(&dst).unwrap(), data);
}

#[tokio::test]
async fn un_archivo_suelto_y_el_progreso() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("ñandú con espacios.bin");
    let data: Vec<u8> = (0..100u8).collect();
    fs::write(&src, &data).unwrap();
    let dst = d.path().join("copia.bin");
    let p = pair(CHUNK).await;
    let mut rx = p.files_a.subscribe_progress();
    let r = within(
        p.files_a
            .put(&p.origin, p.tunnel, &src, dst.to_str().unwrap()),
    )
    .await
    .unwrap();
    assert_eq!((r.files, r.bytes, r.sent), (1, 100, 100));
    assert_eq!(fs::read(&dst).unwrap(), data);
    let mut seen: Vec<TransferProgress> = Vec::new();
    while let Ok(p) = rx.try_recv() {
        seen.push(p);
    }
    assert!(seen.iter().all(|p| p.upload && p.total == 100));
    assert!(seen.windows(2).all(|w| w[0].done <= w[1].done));
    assert_eq!(seen.last().map(|p| (p.done, p.finished)), Some((100, true)));
    // Lo ya copiado no vuelve a viajar.
    let again = within(
        p.files_a
            .put(&p.origin, p.tunnel, &src, dst.to_str().unwrap()),
    )
    .await
    .unwrap();
    assert_eq!((again.bytes, again.sent), (100, 0));
}

#[tokio::test]
async fn un_archivo_suelto_entra_en_una_carpeta_que_ya_existe() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("a.txt");
    fs::write(&src, b"hola").unwrap();
    let dst = d.path().join("carpeta");
    fs::create_dir(&dst).unwrap();
    let p = pair(CHUNK).await;
    within(
        p.files_a
            .put(&p.origin, p.tunnel, &src, dst.to_str().unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"hola");
}

#[tokio::test]
async fn un_tunel_cerrado_no_acepta_archivos() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("a.txt");
    fs::write(&src, b"hola").unwrap();
    let p: Pair = pair(CHUNK).await;
    p.origin.close(p.tunnel).await.unwrap();
    let r = p
        .files_a
        .put(
            &p.origin,
            p.tunnel,
            &src,
            d.path().join("x").to_str().unwrap(),
        )
        .await;
    assert!(r.is_err());
    assert!(!d.path().join("x").exists());
}

#[tokio::test]
async fn una_ruta_con_escape_no_sale_de_la_carpeta() {
    use tandem_proto::{EntryKind, FileEntry, FilesDown, FilesUp, Manifest, TunnelDown, TunnelUp};
    let d = tempfile::tempdir().unwrap();
    let p = pair(CHUNK).await;
    let mut wire = p.origin.dial().await.unwrap();
    let manifest = Manifest {
        chunk_size: CHUNK,
        single: false,
        entries: vec![FileEntry {
            path: "../fuera".into(),
            kind: EntryKind::File {
                size: 0,
                chunks: vec![],
                hash: [0; 32],
            },
            mode: None,
            mtime_ms: None,
        }],
    };
    wire.send(TunnelUp::Files(FilesUp::Put {
        tunnel: p.tunnel,
        remote: d.path().join("dst").to_str().unwrap().into(),
        manifest,
    }))
    .await
    .unwrap();
    let reply = wire.recv().await.unwrap();
    assert!(matches!(
        reply,
        Some(TunnelDown::Files(FilesDown::Failed(_)))
    ));
    assert!(!d.path().join("fuera").exists());
}
