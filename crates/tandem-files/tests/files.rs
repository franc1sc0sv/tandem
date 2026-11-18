//! Files and folders between two agents: they arrive identical, and after a cut at any
//! chunk it resumes without repeating what already arrived.

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
    // Lowercase and no trailing dot or space: Windows and macOS are case-insensitive and do not
    // store those names.
    "f[a-z0-9 \u{f1}\u{e1}\u{e9}\u{fc}_-]{0,4}x"
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

/// What is missing once the cut stream stopped writing (what was in flight may still
/// land an instant after the cut).
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

    /// Any tree (empty ones, binaries, spaces and accents) arrives identical.
    #[test]
    fn the_tree_arrives_identical(t in tree()) {
        rt().block_on(async {
            let d = tempfile::tempdir().unwrap();
            let (src, dst) = (d.path().join("origin"), d.path().join("destination"));
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

    /// A cut at any chunk and the resumption does not repeat what was already received.
    #[test]
    fn upload_resumes_without_repeating(t in tree(), cut in 0u64..12) {
        rt().block_on(async {
            let d = tempfile::tempdir().unwrap();
            let (src, dst) = (d.path().join("origin"), d.path().join("destination"));
            write_tree(&src, &t);
            let dst_s = dst.to_str().unwrap();
            let p = pair(CHUNK).await;
            p.path.cut_after_pieces(cut);
            let first = within(p.files_a.put(&p.origin, p.tunnel, &src, dst_s)).await;
            if first.is_ok() {
                prop_assert_eq!(snapshot(&src), snapshot(&dst));
                return Ok(());
            }
            // A new path against the same destination.
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

    /// The same, downloading from the destination.
    #[test]
    fn download_resumes_without_repeating(t in tree(), cut in 0u64..12) {
        rt().block_on(async {
            let d = tempfile::tempdir().unwrap();
            let (remote, local) = (d.path().join("remote"), d.path().join("local"));
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
async fn the_cut_leaves_the_copy_half_done_and_the_resumption_sends_the_rest() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("big.bin");
    let data: Vec<u8> = (0..80u8).collect(); // 10 fragmentos de 8
    fs::write(&src, &data).unwrap();
    let dst = d.path().join("copy.bin");
    let dst_s = dst.to_str().unwrap();
    let p = pair(CHUNK).await;
    p.path.cut_after_pieces(4);
    let first = within(p.files_a.put(&p.origin, p.tunnel, &src, dst_s)).await;
    assert!(first.is_err(), "it cannot finish with the cut");
    assert!(!dst.exists(), "without finishing there is no final file");
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
async fn a_loose_file_and_the_progress() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("\u{fc}ber with spaces.bin");
    let data: Vec<u8> = (0..100u8).collect();
    fs::write(&src, &data).unwrap();
    let dst = d.path().join("copy.bin");
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
    // What was already copied does not travel again.
    let again = within(
        p.files_a
            .put(&p.origin, p.tunnel, &src, dst.to_str().unwrap()),
    )
    .await
    .unwrap();
    assert_eq!((again.bytes, again.sent), (100, 0));
}

#[tokio::test]
async fn a_loose_file_goes_into_a_folder_that_already_exists() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("a.txt");
    fs::write(&src, b"hello").unwrap();
    let dst = d.path().join("folder");
    fs::create_dir(&dst).unwrap();
    let p = pair(CHUNK).await;
    within(
        p.files_a
            .put(&p.origin, p.tunnel, &src, dst.to_str().unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"hello");
}

#[tokio::test]
async fn a_closed_tunnel_does_not_accept_files() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("a.txt");
    fs::write(&src, b"hello").unwrap();
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

/// T-155: a single file that looks like a credential does not open the copy: report with what was skipped and
/// nothing reaches the destination (before, the other machine rejected an empty manifest).
#[tokio::test]
async fn a_loose_secret_file_is_skipped_without_error() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("id_rsa");
    fs::write(&src, b"key").unwrap();
    let dst = d.path().join("destination");
    let p: Pair = pair(CHUNK).await;
    let r = within(
        p.files_a
            .put(&p.origin, p.tunnel, &src, dst.to_str().unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(r.skipped, ["id_rsa"]);
    assert_eq!((r.files, r.dirs, r.bytes, r.sent), (0, 0, 0, 0));
    assert!(!dst.exists());
}

/// Naming it in `include`, the same file is copied.
#[tokio::test]
async fn a_loose_secret_file_is_copied_if_named() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("id_rsa");
    fs::write(&src, b"key").unwrap();
    let dst = d.path().join("destination");
    let p: Pair = pair(CHUNK).await;
    let r = within(p.files_a.put_with(
        &p.origin,
        p.tunnel,
        &src,
        dst.to_str().unwrap(),
        &["id_rsa".to_owned()],
    ))
    .await
    .unwrap();
    assert!(r.skipped.is_empty());
    assert_eq!(r.files, 1);
    assert_eq!(fs::read(&dst).unwrap(), b"key");
}

#[tokio::test]
async fn a_path_with_an_escape_does_not_leave_the_folder() {
    use tandem_proto::{EntryKind, FileEntry, FilesDown, FilesUp, Manifest, TunnelDown, TunnelUp};
    let d = tempfile::tempdir().unwrap();
    let p = pair(CHUNK).await;
    let mut wire = p.origin.dial().await.unwrap();
    let manifest = Manifest {
        chunk_size: CHUNK,
        single: false,
        entries: vec![FileEntry {
            path: "../outside".into(),
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
        chunk_size: manifest.chunk_size,
        single: manifest.single,
    }))
    .await
    .unwrap();
    wire.send(TunnelUp::Files(FilesUp::Entries {
        entries: manifest.entries,
        last: true,
    }))
    .await
    .unwrap();
    let reply = wire.recv().await.unwrap();
    assert!(matches!(
        reply,
        Some(TunnelDown::Files(FilesDown::Failed(_)))
    ));
    assert!(!d.path().join("outside").exists());
}

/// A tree of ~12,000 files: the manifest and the plan do not fit in a frame, so
/// they travel in pages; it arrives identical in both directions.
#[tokio::test(flavor = "multi_thread")]
async fn a_huge_tree_travels_paged() {
    let d = tempfile::tempdir().unwrap();
    let (src, dst, back) = (
        d.path().join("origin"),
        d.path().join("destination"),
        d.path().join("return"),
    );
    for i in 0..12_000u32 {
        let dir = src.join(format!("d{:03}", i / 200));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("archivo-{i}.txt")),
            format!("contenido {i}"),
        )
        .unwrap();
    }
    let p = pair(CHUNK).await;
    let dst_s = dst.to_str().unwrap();
    let r = slow(p.files_a.put(&p.origin, p.tunnel, &src, dst_s))
        .await
        .unwrap();
    assert_eq!(r.files, 12_000);
    assert_eq!(snapshot(&src), snapshot(&dst));
    slow(p.files_a.get(&p.origin, p.tunnel, dst_s, &back))
        .await
        .unwrap();
    assert_eq!(snapshot(&src), snapshot(&back));
}

/// Like `within`, with more margin: 12,000 files on a Windows CI disk take a while.
async fn slow<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(240), f)
        .await
        .unwrap()
}
