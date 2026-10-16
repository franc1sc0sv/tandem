//! Portapapeles entre dos agentes del mismo proceso, por loopback con QUIC real
//! y portapapeles simulados.

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
/// Tope para que una prueba rota falle en vez de colgarse.
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
            // Trozos chicos: una imagen de prueba ya son muchos.
            chunk_len: 4096,
            ..SyncConfig::default()
        },
    }
}

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, f)
        .await
        .expect("la prueba superó el tiempo máximo")
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

/// El stream del portapapeles: lo abre la Mac (quien conectó) y lo acepta la PC.
async fn streams(l: &Link) -> (ControlStream, ControlStream) {
    let mac_hello = Hello::new(Peer::Agent(Machine::Mac));
    let pc_hello = Hello::new(Peer::Agent(Machine::Pc));
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
        // Si el otro lado se detuvo antes, esta sesión ya pudo terminar por el
        // stream cerrado.
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
async fn texto_copiado_en_la_mac_llega_a_la_pc_sin_eco() {
    let l = link().await;
    let (ms, ps) = streams(&l).await;
    let mac_cb = FakeClipboard::new();
    let pc_cb = FakeClipboard::new();
    let mac = ClipDriver::new(mac_cb.clone(), cfg(), true);
    let pc = ClipDriver::new(pc_cb.clone(), cfg(), false);
    let mut pc_stats = pc.stats();
    let (mac, pc) = (spawn(mac, ms), spawn(pc, ps));

    mac_cb.copy(Some(rich("hola ñandú")));
    wait_state(&pc_cb, |s| !s.writes.is_empty()).await;
    assert_eq!(pc_cb.state().content, Some(rich("hola ñandú")));

    // Otra copia después: también llega, y lo escrito en la PC no vuelve.
    mac_cb.copy(Some(ClipContent::text("segunda")));
    // Hasta que el driver terminó de escribir y anotó el contador.
    wait_stats(&mut pc_stats, |s| s.written == 2).await;
    // Algo toca el portapapeles de la PC sin cambiarlo: el hash lo frena.
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
async fn la_escritura_convertida_por_el_sistema_no_vuelve_como_eco() {
    let l = link().await;
    let (ms, ps) = streams(&l).await;
    let mac_cb = FakeClipboard::new();
    // Windows guarda el HTML con su cabecera CF_HTML: al leerlo no es igual.
    let pc_cb = FakeClipboard::new().with_rewrite(|mut c| {
        c.html = c.html.map(|h| format!("Version:0.9\r\n{h}"));
        c
    });
    let mac = spawn(ClipDriver::new(mac_cb.clone(), cfg(), true), ms);
    let pc = spawn(ClipDriver::new(pc_cb.clone(), cfg(), false), ps);

    mac_cb.copy(Some(rich("con formato")));
    wait_state(&pc_cb, |s| !s.writes.is_empty()).await;
    // Una copia más de la Mac, en orden detrás de cualquier eco que hubiera.
    mac_cb.copy(Some(ClipContent::text("marca")));
    wait_state(&pc_cb, |s| s.writes.len() == 2).await;

    let (mac, pc) = (mac.stop().await, pc.stop().await);
    assert_eq!(pc.sync().stats().offers_sent, 0);
    assert_eq!(mac.sync().stats().offers_received, 0);
    assert!(mac_cb.state().writes.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn imagen_grande_se_transfiere_cuando_la_pc_toma_el_foco() {
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
    // El anuncio llega, pero sin el foco no se transfiere.
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
async fn copias_en_los_dos_sentidos() {
    let l = link().await;
    let (ms, ps) = streams(&l).await;
    let mac_cb = FakeClipboard::new();
    let pc_cb = FakeClipboard::new();
    let mac = ClipDriver::new(mac_cb.clone(), cfg(), true);
    let pc = ClipDriver::new(pc_cb.clone(), cfg(), true);
    let (mut mac_stats, mut pc_stats) = (mac.stats(), pc.stats());
    let (mac, pc) = (spawn(mac, ms), spawn(pc, ps));

    mac_cb.copy(Some(ClipContent::text("de la mac")));
    // Copiar recién cuando el driver terminó de escribir: una copia en medio de
    // la escritura se confundiría con ella.
    wait_stats(&mut pc_stats, |s| s.written == 1).await;
    pc_cb.copy(Some(big_image(1)));
    wait_stats(&mut mac_stats, |s| s.written == 1).await;
    assert_eq!(mac_cb.state().content, Some(big_image(1)));
    mac_cb.copy(Some(rich("otra vez")));
    wait_state(&pc_cb, |s| s.writes.len() == 2).await;
    assert_eq!(pc_cb.state().content, Some(rich("otra vez")));

    let (mac, pc) = (mac.stop().await, pc.stop().await);
    assert_eq!(mac.sync().stats().offers_sent, 2);
    assert_eq!(pc.sync().stats().offers_sent, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lo_copiado_sin_enlace_se_anuncia_en_la_sesion_siguiente() {
    let l = link().await;
    let mac_cb = FakeClipboard::new();
    let pc_cb = FakeClipboard::new();
    // Lo que había antes de arrancar también es una copia sin anunciar.
    mac_cb.copy(Some(ClipContent::text("de antes")));
    let (ms, ps) = streams(&l).await;
    let mac = spawn(ClipDriver::new(mac_cb.clone(), cfg(), false), ms);
    let pc = spawn(ClipDriver::new(pc_cb.clone(), cfg(), true), ps);
    wait_state(&pc_cb, |s| s.writes.len() == 1).await;
    let (mac, pc) = (mac.stop().await, pc.stop().await);

    // Sin enlace, la Mac copia una imagen grande.
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
async fn el_par_que_cierra_el_stream_termina_la_sesion() {
    let l = link().await;
    let (ms, ps) = streams(&l).await;
    let pc = spawn(ClipDriver::new(FakeClipboard::new(), cfg(), true), ps);
    drop(ms);
    let (_, end) = within(pc.task).await.unwrap();
    assert!(matches!(end, SyncEnd::Link(_)), "{end}");
}
