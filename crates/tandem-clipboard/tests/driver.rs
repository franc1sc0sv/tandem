//! Clipboard between two agents of the same process, over loopback with real QUIC
//! and simulated clipboards.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tandem_clipboard::fake::{FakeClipboard, FakeState};
use tandem_clipboard::{
    ClipConfig, ClipContent, ClipDriver, Image, SyncConfig, SyncEnd, SyncStats,
};
use tandem_core::Machine;
use tandem_link::identity::{Identity, PublicKey};
use tandem_link::transport::{Connection, ControlStream, Endpoint, PeerTrust};
use tandem_proto::{Hello, Peer};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

const LOOPBACK: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
/// Cap so a broken test fails instead of hanging.
const TIMEOUT: Duration = Duration::from_secs(10);

struct Only(PublicKey);

impl PeerTrust for Only {
    fn is_trusted(&self, key: &PublicKey) -> bool {
        *key == self.0
    }
}

fn cfg() -> ClipConfig {
    ClipConfig {
        poll: Duration::from_millis(5),
        sync: SyncConfig {
            inline_max: 1024,
            // Small chunks: a test image is already many.
            chunk_len: 4096,
            ..SyncConfig::default()
        },
    }
}

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .expect("the test exceeded the maximum time")
}

struct Link {
    _mac_ep: Endpoint,
    _pc_ep: Endpoint,
    mac: Connection,
    pc: Connection,
}

async fn link() -> Link {
    let mac = Identity::from_seed([1; 32]);
    let pc = Identity::from_seed([2; 32]);
    let mac_ep = Endpoint::bind(LOOPBACK, &mac, Arc::new(Only(pc.public_key()))).unwrap();
    let pc_ep = Endpoint::bind(LOOPBACK, &pc, Arc::new(Only(mac.public_key()))).unwrap();
    let addr = pc_ep.local_addr().unwrap();
    let (m, p) = within(async { tokio::join!(mac_ep.connect(addr), pc_ep.accept()) }).await;
    Link {
        _mac_ep: mac_ep,
        _pc_ep: pc_ep,
        mac: m.unwrap(),
        pc: p.unwrap().unwrap(),
    }
}

/// The clipboard stream: opened by the Mac (the one that connected) and accepted by the PC.
async fn streams(l: &Link) -> (ControlStream, ControlStream) {
    let mac_hello = Hello::new(Peer::Agent(Machine::A));
    let pc_hello = Hello::new(Peer::Agent(Machine::B));
    let (m, p) = within(async {
        tokio::join!(
            l.mac.open_control(&mac_hello),
            l.pc.accept_control(&pc_hello)
        )
    })
    .await;
    (m.unwrap().0, p.unwrap().0)
}

type Driver = ClipDriver<FakeClipboard>;

struct Running {
    stop: oneshot::Sender<()>,
    task: JoinHandle<(Driver, SyncEnd)>,
}

impl Running {
    async fn stop(self) -> Driver {
        let _ = self.stop.send(());
        // If the other side stopped earlier, this session may already have ended due to the
        // closed stream.
        let (driver, end) = within(self.task).await.unwrap();
        assert!(matches!(end, SyncEnd::Stopped | SyncEnd::Link(_)), "{end}");
        driver
    }
}

fn spawn(mut driver: Driver, stream: ControlStream) -> Running {
    let (stop, rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let end = driver
            .run(stream, async {
                let _ = rx.await;
            })
            .await;
        (driver, end)
    });
    Running { stop, task }
}

async fn wait_state(cb: &FakeClipboard, f: impl Fn(&FakeState) -> bool) {
    let mut rx = cb.subscribe();
    within(rx.wait_for(|s| f(s))).await.unwrap();
}

async fn wait_stats(rx: &mut watch::Receiver<SyncStats>, f: impl Fn(&SyncStats) -> bool) {
    within(rx.wait_for(|s| f(s))).await.unwrap();
}

fn big_image(fill: u8) -> ClipContent {
    ClipContent {
        text: None,
        html: None,
        image: Some(Image {
            width: 64,
            height: 48,
            rgba: (0..64 * 48 * 4)
                .map(|i| (i as u8).wrapping_add(fill))
                .collect(),
        }),
    }
}

fn rich(text: &str) -> ClipContent {
    ClipContent {
        text: Some(text.into()),
        html: Some(format!("<b>{text}</b>")),
        image: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_copied_on_the_mac_reaches_the_pc_without_echo() {
    let l = link().await;
    let (ms, ps) = streams(&l).await;
    let mac_cb = FakeClipboard::new();
    let pc_cb = FakeClipboard::new();
    let mac = ClipDriver::new(mac_cb.clone(), cfg(), true);
    let pc = ClipDriver::new(pc_cb.clone(), cfg(), false);
    let mut pc_stats = pc.stats();
    let (mac, pc) = (spawn(mac, ms), spawn(pc, ps));

    mac_cb.copy(Some(rich("hello \u{fc}ber")));
    wait_state(&pc_cb, |s| !s.writes.is_empty()).await;
    assert_eq!(pc_cb.state().content, Some(rich("hello \u{fc}ber")));

    // Another copy afterwards: it also arrives, and what was written on the PC does not come back.
    mac_cb.copy(Some(ClipContent::text("second")));
    // Until the driver finished writing and noted the counter.
    wait_stats(&mut pc_stats, |s| s.written == 2).await;
    // Something touches the PC's clipboard without changing it: the hash stops it.
    let reads = pc_cb.state().reads;
    pc_cb.touch();
    wait_state(&pc_cb, |s| s.reads > reads).await;

    let (mac, pc) = (mac.stop().await, pc.stop().await);
    assert_eq!(mac_cb.state().writes, Vec::<ClipContent>::new());
    assert_eq!(mac.sync().stats().offers_sent, 2);
    assert_eq!(mac.sync().stats().offers_received, 0);
    assert_eq!(pc.sync().stats().offers_sent, 0);
    assert_eq!(pc.sync().stats().written, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_converted_by_the_system_does_not_come_back_as_an_echo() {
    let l = link().await;
    let (ms, ps) = streams(&l).await;
    let mac_cb = FakeClipboard::new();
    // Windows stores the HTML with its CF_HTML header: reading it back is not equal.
    let pc_cb = FakeClipboard::new().with_rewrite(|mut c| {
        c.html = c.html.map(|h| format!("Version:0.9\r\n{h}"));
        c
    });
    let mac = spawn(ClipDriver::new(mac_cb.clone(), cfg(), true), ms);
    let pc = spawn(ClipDriver::new(pc_cb.clone(), cfg(), false), ps);

    mac_cb.copy(Some(rich("formatted")));
    wait_state(&pc_cb, |s| !s.writes.is_empty()).await;
    // One more copy from the Mac, in order behind any echo there might be.
    mac_cb.copy(Some(ClipContent::text("mark")));
    wait_state(&pc_cb, |s| s.writes.len() == 2).await;

    let (mac, pc) = (mac.stop().await, pc.stop().await);
    assert_eq!(pc.sync().stats().offers_sent, 0);
    assert_eq!(mac.sync().stats().offers_received, 0);
    assert!(mac_cb.state().writes.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_large_image_is_transferred_when_the_pc_takes_focus() {
    let l = link().await;
    let (ms, ps) = streams(&l).await;
    let mac_cb = FakeClipboard::new();
    let pc_cb = FakeClipboard::new();
    let (focus_tx, focus_rx) = watch::channel(false);
    let mac = spawn(ClipDriver::new(mac_cb.clone(), cfg(), true), ms);
    let pc_driver = ClipDriver::new(pc_cb.clone(), cfg(), true).with_focus(focus_rx);
    let mut pc_stats = pc_driver.stats();
    let pc = spawn(pc_driver, ps);

    let img = big_image(3);
    mac_cb.copy(Some(img.clone()));
    // The announcement arrives, but without focus it is not transferred.
    wait_stats(&mut pc_stats, |s| s.offers_received == 1).await;
    assert!(pc_cb.state().writes.is_empty());

    focus_tx.send_replace(true);
    wait_state(&pc_cb, |s| !s.writes.is_empty()).await;
    assert_eq!(pc_cb.state().content, Some(img));

    let (mac, pc) = (mac.stop().await, pc.stop().await);
    assert_eq!(pc.sync().stats().written, 1);
    assert_eq!(pc.sync().stats().rejected, 0);
    assert_eq!(mac.sync().stats().offers_received, 0);
    assert!(mac_cb.state().writes.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn copies_in_both_directions() {
    let l = link().await;
    let (ms, ps) = streams(&l).await;
    let mac_cb = FakeClipboard::new();
    let pc_cb = FakeClipboard::new();
    let mac = ClipDriver::new(mac_cb.clone(), cfg(), true);
    let pc = ClipDriver::new(pc_cb.clone(), cfg(), true);
    let (mut mac_stats, mut pc_stats) = (mac.stats(), pc.stats());
    let (mac, pc) = (spawn(mac, ms), spawn(pc, ps));

    mac_cb.copy(Some(ClipContent::text("from the mac")));
    // Copy only once the driver finished writing: a copy in the middle of
    // the write would be confused with it.
    wait_stats(&mut pc_stats, |s| s.written == 1).await;
    pc_cb.copy(Some(big_image(1)));
    wait_stats(&mut mac_stats, |s| s.written == 1).await;
    assert_eq!(mac_cb.state().content, Some(big_image(1)));
    mac_cb.copy(Some(rich("again")));
    wait_state(&pc_cb, |s| s.writes.len() == 2).await;
    assert_eq!(pc_cb.state().content, Some(rich("again")));

    let (mac, pc) = (mac.stop().await, pc.stop().await);
    assert_eq!(mac.sync().stats().offers_sent, 2);
    assert_eq!(pc.sync().stats().offers_sent, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_was_copied_without_a_link_is_announced_in_the_next_session() {
    let l = link().await;
    let mac_cb = FakeClipboard::new();
    let pc_cb = FakeClipboard::new();
    // What was there before starting is also an unannounced copy.
    mac_cb.copy(Some(ClipContent::text("from before")));
    let (ms, ps) = streams(&l).await;
    let mac = spawn(ClipDriver::new(mac_cb.clone(), cfg(), false), ms);
    let pc = spawn(ClipDriver::new(pc_cb.clone(), cfg(), true), ps);
    wait_state(&pc_cb, |s| s.writes.len() == 1).await;
    let (mac, pc) = (mac.stop().await, pc.stop().await);

    // Without a link, the Mac copies a large image.
    mac_cb.copy(Some(big_image(9)));
    let (ms, ps) = streams(&l).await;
    let mac = spawn(mac, ms);
    let pc = spawn(pc, ps);
    wait_state(&pc_cb, |s| s.writes.len() == 2).await;
    assert_eq!(pc_cb.state().content, Some(big_image(9)));
    let (_, pc) = (mac.stop().await, pc.stop().await);
    assert_eq!(pc.sync().stats().written, 2);
    assert!(mac_cb.state().writes.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_peer_that_closes_the_stream_ends_the_session() {
    let l = link().await;
    let (ms, ps) = streams(&l).await;
    let pc = spawn(ClipDriver::new(FakeClipboard::new(), cfg(), true), ps);
    drop(ms);
    let (_, end) = within(pc.task).await.unwrap();
    assert!(matches!(end, SyncEnd::Link(_)), "{end}");
}

/// Windows' clipboard busy with another app ("being held by another party"): the
/// read fails for a while. Each consecutive failure waits twice as long (T-087), it is not retried on
/// every polling turn, and as soon as it is released the copy reaches the other side.
mod ocupado {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use tandem_clipboard::Clipboard;

    use super::*;

    /// A clipboard that does not allow reading the first `busy` times.
    struct Busy {
        inner: FakeClipboard,
        busy: u32,
        attempts: Arc<AtomicU32>,
        at: Arc<Mutex<Vec<std::time::Instant>>>,
    }

    impl Clipboard for Busy {
        type Error = String;

        fn change_count(&mut self) -> Result<u64, String> {
            self.inner.change_count().map_err(|e| match e {})
        }

        fn read(&mut self) -> Result<Option<ClipContent>, String> {
            let n = self.attempts.fetch_add(1, Ordering::SeqCst);
            self.at.lock().unwrap().push(std::time::Instant::now());
            if n < self.busy {
                return Err("Clipboard occupied: being held by another party".into());
            }
            self.inner.read().map_err(|e| match e {})
        }

        fn write(&mut self, content: &ClipContent) -> Result<(), String> {
            self.inner.write(content).map_err(|e| match e {})
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_busy_read_retries_with_progressive_backoff() {
        let l = link().await;
        let (ms, ps) = streams(&l).await;
        let mac_cb = FakeClipboard::new();
        let pc_cb = FakeClipboard::new();
        let attempts = Arc::new(AtomicU32::new(0));
        let at = Arc::new(Mutex::new(Vec::new()));
        // Busy on the first 4 reads (the startup one and three more).
        let busy = Busy {
            inner: mac_cb.clone(),
            busy: 4,
            attempts: attempts.clone(),
            at: at.clone(),
        };
        let mac = ClipDriver::new(busy, cfg(), true);
        let pc = ClipDriver::new(pc_cb.clone(), cfg(), false);
        let (stop, rx) = oneshot::channel::<()>();
        let mac_task = tokio::spawn(async move {
            let mut mac = mac;
            let end = mac
                .run(ms, async {
                    let _ = rx.await;
                })
                .await;
            (mac, end)
        });
        let pc = spawn(pc, ps);

        mac_cb.copy(Some(rich("after the wait")));
        wait_state(&pc_cb, |s| !s.writes.is_empty()).await;
        assert_eq!(pc_cb.state().content, Some(rich("after the wait")));

        // 4 failures and a success, not a retry on every 5 ms turn; and each wait
        // is greater than the previous (5, 10, 20, 40 ms of base).
        let times = at.lock().unwrap().clone();
        assert!(times.len() <= 8, "{} lecturas", times.len());
        let gaps: Vec<Duration> = times.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            gaps[3] >= Duration::from_millis(35),
            "the fourth wait should be about 40 ms: {gaps:?}"
        );

        let _ = stop.send(());
        let _ = within(mac_task).await.unwrap();
        pc.stop().await;
    }
}
