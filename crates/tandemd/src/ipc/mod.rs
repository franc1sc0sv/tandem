//! API local para la UI: Unix socket en macOS (y Linux) y named pipe en Windows,
//! accesibles solo para el usuario actual (DESIGN §9).
//!
//! Habla los mensajes UI ↔ agente de `tandem-proto`: cada lado manda su [`Hello`]
//! (el agente primero), y después la UI manda [`UiRequest`] y el agente contesta con
//! [`AgentToUi`]. El agente además empuja [`AgentToUi::State`] cada vez que el estado cambia
//! y [`AgentToUi::Txn`] con el avance de cada cambio de modo, [`AgentToUi::ManualSwitch`]
//! cuando hay que cambiar a mano la entrada de un monitor, [`AgentToUi::Power`] cuando
//! la PC se da por apagada sin que nadie la apagara, [`AgentToUi::Peer`] cuando cambia
//! el par o el enlace con él, [`AgentToUi::Pairing`] con cada paso del emparejamiento,
//! [`AgentToUi::Input`] cuando cambia el teclado y mouse (roles, sesión, permisos
//! que faltan), [`AgentToUi::Probe`] con las sondas de calibración que mostrar y
//! [`AgentToUi::Calibration`] con el avance de «Agregar monitor».
//!
//! El transporte va detrás de [`Listener`]: la lógica se prueba con [`mem`] y el
//! socket o pipe reales en las pruebas de integración.

use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tandem_core::{Machine, State};
use tandem_proto::{
    AgentToUi, FrameDecoder, Hello, Peer, ProtoError, Reply, UiCommand, UiRequest, decode,
    decode_hello, encode_frame,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;

use crate::agent::Agent;
use crate::display::manual_event_to_ui;

pub mod mem;
#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::{UnixIpcListener as PlatformListener, bind, connect, default_endpoint};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{PipeListener as PlatformListener, bind, connect, default_endpoint};

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("error de E/S en la IPC: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Proto(#[from] ProtoError),
    #[error("el otro lado cerró la conexión")]
    Closed,
    #[error("se esperaba {expected}, se conectó {got:?}")]
    UnexpectedPeer { expected: &'static str, got: Peer },
    #[error("respuesta inesperada: {0:?}")]
    UnexpectedReply(Reply),
    #[error("ya hay un tandemd escuchando en {0}")]
    AlreadyRunning(PathBuf),
    #[error("{0} existe y no es un socket; no se borra")]
    NotASocket(PathBuf),
}

/// Fuente de conexiones de la UI.
pub trait Listener: Send {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// La siguiente conexión ya aceptada (y, en el socket real, del usuario correcto).
    fn accept(&mut self) -> impl Future<Output = io::Result<Self::Stream>> + Send;
}

/// Mensajes con trama de `tandem-proto` sobre un flujo de bytes.
#[derive(Debug)]
pub struct Framed<S> {
    io: S,
    decoder: FrameDecoder,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Framed<S> {
    pub fn new(io: S) -> Self {
        Framed {
            io,
            decoder: FrameDecoder::new(),
        }
    }

    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<(), IpcError> {
        self.io.write_all(&encode_frame(msg)?).await?;
        self.io.flush().await?;
        Ok(())
    }

    /// `None` si el otro lado cerró limpio, entre dos mensajes.
    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<Option<T>, IpcError> {
        match self.recv_frame().await? {
            Some(p) => Ok(Some(decode(&p)?)),
            None => Ok(None),
        }
    }

    /// Lee el [`Hello`] de apertura, rechazando otra versión del protocolo.
    pub async fn recv_hello(&mut self) -> Result<Hello, IpcError> {
        let p = self.recv_frame().await?.ok_or(IpcError::Closed)?;
        Ok(decode_hello(&p)?)
    }

    /// Se puede cancelar (por ejemplo en un `select!`) sin perder bytes: lo leído se
    /// guarda en el decodificador antes del siguiente punto de espera.
    async fn recv_frame(&mut self) -> Result<Option<Vec<u8>>, IpcError> {
        let mut buf = [0u8; 4096];
        loop {
            if let Some(frame) = self.decoder.next_frame()? {
                return Ok(Some(frame));
            }
            let n = self.io.read(&mut buf).await?;
            if n == 0 {
                return if self.decoder.buffered() == 0 {
                    Ok(None)
                } else {
                    Err(io::Error::from(io::ErrorKind::UnexpectedEof).into())
                };
            }
            self.decoder.push(&buf[..n]);
        }
    }
}

/// Atiende una conexión de la UI hasta que se cierra o llega el apagado.
pub async fn serve_connection<S>(
    io: S,
    agent: Arc<Agent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), IpcError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut conn = Framed::new(io);
    conn.send(&Hello::new(Peer::Agent(agent.machine()))).await?;
    let hello = conn.recv_hello().await?;
    if hello.peer != Peer::Ui {
        return Err(IpcError::UnexpectedPeer {
            expected: "la UI",
            got: hello.peer,
        });
    }
    let mut states = agent.subscribe();
    states.mark_unchanged();
    let mut txns = agent.subscribe_txns();
    let mut manual = agent.subscribe_manual();
    let mut probes = agent.subscribe_probes();
    let mut calibration = agent.subscribe_calibration();
    let mut power = agent.subscribe_power();
    let mut peer = agent.subscribe_peer();
    peer.mark_unchanged();
    let mut pairing = agent.subscribe_pairing();
    pairing.mark_unchanged();
    let mut input = agent.subscribe_input();
    input.mark_unchanged();
    loop {
        tokio::select! {
            req = conn.recv::<UiRequest>() => {
                let Some(req) = req? else { return Ok(()) };
                tracing::debug!(id = req.id, command = ?req.command, "pedido de la UI");
                let reply = agent.respond(req.command).await;
                conn.send(&AgentToUi::Reply { id: req.id, reply }).await?;
            }
            changed = states.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let state = states.borrow_and_update().clone();
                conn.send(&AgentToUi::State(state)).await?;
            }
            event = txns.recv() => match event {
                Ok(event) => conn.send(&AgentToUi::Txn(event)).await?,
                // La UI vuelve a sincronizarse con el siguiente estado empujado.
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("la UI se atrasó y perdió {n} avisos de transacción");
                }
                Err(RecvError::Closed) => return Ok(()),
            },
            event = manual.recv() => match event {
                Ok(event) => conn.send(&AgentToUi::ManualSwitch(manual_event_to_ui(event))).await?,
                // Un pedido perdido vence con su paso, y la UI lo ve en la transacción.
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("la UI se atrasó y perdió {n} pedidos de cambio manual");
                }
                Err(RecvError::Closed) => return Ok(()),
            },
            event = probes.recv() => match event {
                Ok(event) => conn.send(&AgentToUi::Probe(event)).await?,
                // Una sonda perdida vence sola y cuenta como no vista.
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("la UI se atrasó y perdió {n} avisos de sondas de calibración");
                }
                Err(RecvError::Closed) => return Ok(()),
            },
            event = calibration.recv() => match event {
                Ok(event) => conn.send(&AgentToUi::Calibration(event)).await?,
                // El resultado final llega igual; solo se pierde parte del avance.
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("la UI se atrasó y perdió {n} avisos de calibración");
                }
                Err(RecvError::Closed) => return Ok(()),
            },
            event = power.recv() => match event {
                Ok(event) => conn.send(&AgentToUi::Power(event)).await?,
                // El estado empujado ya dice que la PC está apagada.
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("la UI se atrasó y perdió {n} avisos de energía");
                }
                Err(RecvError::Closed) => return Ok(()),
            },
            changed = peer.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let status = peer.borrow_and_update().clone();
                conn.send(&AgentToUi::Peer(status)).await?;
            }
            changed = pairing.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let status = pairing.borrow_and_update().clone();
                conn.send(&AgentToUi::Pairing(status)).await?;
            }
            changed = input.changed() => {
                if changed.is_err() {
                    return Ok(());
                }
                let status = input.borrow_and_update().clone();
                conn.send(&AgentToUi::Input(status)).await?;
            }
            () = stopped(&mut shutdown) => return Ok(()),
        }
    }
}

/// Termina cuando se pide el apagado o desaparece quien lo pediría.
async fn stopped(shutdown: &mut watch::Receiver<bool>) {
    // El `Ref` que devuelve `wait_for` no es `Send`: se suelta aquí mismo.
    let _ = shutdown.wait_for(|stop| *stop).await.map(|_| ());
}

/// Cliente de la IPC: lo usan las pruebas y, más adelante, la UI.
#[derive(Debug)]
pub struct IpcClient<S> {
    conn: Framed<S>,
    agent: Machine,
    next_id: u64,
    /// Avisos que llegaron mientras se esperaba una respuesta.
    pending: VecDeque<AgentToUi>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> IpcClient<S> {
    pub async fn handshake(io: S) -> Result<Self, IpcError> {
        let mut conn = Framed::new(io);
        conn.send(&Hello::new(Peer::Ui)).await?;
        let hello = conn.recv_hello().await?;
        let Peer::Agent(agent) = hello.peer else {
            return Err(IpcError::UnexpectedPeer {
                expected: "un agente",
                got: hello.peer,
            });
        };
        Ok(IpcClient {
            conn,
            agent,
            next_id: 0,
            pending: VecDeque::new(),
        })
    }

    /// Máquina del agente al otro lado.
    pub fn agent(&self) -> Machine {
        self.agent
    }

    pub async fn request(&mut self, command: UiCommand) -> Result<Reply, IpcError> {
        let id = self.next_id;
        self.next_id += 1;
        self.conn.send(&UiRequest { id, command }).await?;
        loop {
            match self
                .conn
                .recv::<AgentToUi>()
                .await?
                .ok_or(IpcError::Closed)?
            {
                AgentToUi::Reply { id: got, reply } if got == id => return Ok(reply),
                other => self.pending.push_back(other),
            }
        }
    }

    pub async fn get_state(&mut self) -> Result<State, IpcError> {
        match self.request(UiCommand::GetState).await? {
            Reply::State(s) => Ok(s),
            other => Err(IpcError::UnexpectedReply(other)),
        }
    }

    /// El siguiente aviso espontáneo del agente; `None` si cerró.
    pub async fn next_event(&mut self) -> Result<Option<AgentToUi>, IpcError> {
        if let Some(ev) = self.pending.pop_front() {
            return Ok(Some(ev));
        }
        self.conn.recv().await
    }
}

#[cfg(test)]
mod tests {
    use tandem_proto::PROTOCOL_VERSION;
    use tokio::io::duplex;

    use super::*;
    use tandem_proto::TxnEvent;

    use crate::agent::tests::agent;
    use crate::orchestrator::STEP_NOT_IMPLEMENTED;

    fn start(dir: &std::path::Path) -> (Arc<Agent>, tokio::io::DuplexStream, watch::Sender<bool>) {
        let agent = Arc::new(agent(dir));
        let (client, server) = duplex(64);
        let (stop, stop_rx) = watch::channel(false);
        let a = agent.clone();
        tokio::spawn(async move { serve_connection(server, a, stop_rx).await });
        (agent, client, stop)
    }

    #[tokio::test]
    async fn client_gets_state() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        assert_eq!(c.agent(), Machine::Mac);
        assert_eq!(c.get_state().await.unwrap(), agent.state());
    }

    #[tokio::test]
    async fn peer_status_is_answered_and_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        assert_eq!(
            c.request(UiCommand::GetPeer).await.unwrap(),
            Reply::Peer(tandem_proto::PeerStatus::Unpaired)
        );
        let paired = tandem_proto::PeerStatus::Paired {
            name: "PC".into(),
            link: Some(tandem_proto::Link::Usb4),
        };
        agent.set_peer_status(paired.clone());
        assert_eq!(c.next_event().await.unwrap(), Some(AgentToUi::Peer(paired)));
    }

    #[tokio::test]
    async fn pairing_status_is_answered_and_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        assert_eq!(
            c.request(UiCommand::GetPairing).await.unwrap(),
            Reply::Pairing(tandem_proto::PairingStatus::Idle)
        );
        let comparing = tandem_proto::PairingStatus::Comparing {
            code: "012345".into(),
            peer: "PC".into(),
            confirmed: false,
        };
        agent.set_pairing_status(comparing.clone());
        assert_eq!(
            c.next_event().await.unwrap(),
            Some(AgentToUi::Pairing(comparing))
        );
    }

    #[tokio::test]
    async fn input_status_is_answered_and_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        let initial = tandem_proto::InputStatus::new(Machine::Mac);
        assert_eq!(
            c.request(UiCommand::GetInput).await.unwrap(),
            Reply::Input(initial.clone())
        );
        let missing = tandem_proto::InputStatus {
            missing: vec![tandem_proto::InputPermission::Accessibility],
            ..initial
        };
        agent.set_input_status(missing.clone());
        assert_eq!(
            c.next_event().await.unwrap(),
            Some(AgentToUi::Input(missing))
        );
    }

    #[tokio::test]
    async fn mode_change_progress_is_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        let r = c
            .request(UiCommand::SetMode(tandem_core::Mode::SoloPc))
            .await
            .unwrap();
        let Reply::Accepted { txn } = r else {
            panic!("se esperaba Accepted: {r:?}");
        };
        // Sin backends, el primer paso falla con un motivo claro y todo vuelve atrás.
        let mut events = Vec::new();
        loop {
            match c.next_event().await.unwrap().unwrap() {
                AgentToUi::Txn(e) => {
                    let end = matches!(e, TxnEvent::RolledBack { .. });
                    events.push(e);
                    if end {
                        break;
                    }
                }
                other => panic!("aviso inesperado: {other:?}"),
            }
        }
        assert!(matches!(&events[0], TxnEvent::Started { txn: t, .. } if *t == txn));
        assert_eq!(events[1], TxnEvent::StepStarted { txn, index: 0 });
        assert!(matches!(
            &events[2],
            TxnEvent::StepFailed { index: 0, error, .. } if error.contains(STEP_NOT_IMPLEMENTED)
        ));
        assert_eq!(events[3], TxnEvent::RollingBack { txn });
        assert_eq!(
            events[4],
            TxnEvent::RolledBack {
                txn,
                state: agent.state()
            }
        );
        assert_eq!(c.get_state().await.unwrap(), agent.state());
    }

    #[tokio::test]
    async fn state_changes_are_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        // Tras el saludo, la suscripción del servidor ya existe si respondió un pedido.
        c.get_state().await.unwrap();
        let mut s = agent.state();
        s.pc_on = true;
        agent.set_state(s.clone()).unwrap();
        assert_eq!(c.next_event().await.unwrap(), Some(AgentToUi::State(s)));
    }

    #[tokio::test]
    async fn power_events_are_pushed() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        c.get_state().await.unwrap();
        let ev = tandem_proto::PowerEvent::PcLost {
            reason: "sin enlaces".into(),
            stranded: vec!["m34".into()],
        };
        agent.emit_power(ev.clone());
        assert_eq!(c.next_event().await.unwrap(), Some(AgentToUi::Power(ev)));
    }

    /// T-078: la sonda llega a la UI, el clic vuelve por la IPC y la sonda se quita.
    #[tokio::test]
    async fn probes_are_pushed_and_answered() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        c.get_state().await.unwrap();
        let probe = tandem_proto::CalibrationProbe {
            monitor: tandem_proto::ProbeMonitor {
                edid: None,
                name: "Odyssey G5".into(),
            },
            input: tandem_core::Input::Hdmi,
            ask: tandem_proto::ProbeAsk::UserSwitch,
            timeout_ms: 60_000,
        };
        let shown = tokio::spawn({
            let (agent, probe) = (agent.clone(), probe.clone());
            async move { agent.probes().show(&probe, false).await }
        });
        let Some(AgentToUi::Probe(tandem_proto::ProbeEvent::Show {
            id,
            probe: got,
            place,
        })) = c.next_event().await.unwrap()
        else {
            panic!("se esperaba la sonda");
        };
        assert_eq!(got, probe);
        assert_eq!(place, tandem_proto::ProbePlace::Monitor { screen: None });
        assert_eq!(
            c.request(UiCommand::AnswerProbe { id, seen: true })
                .await
                .unwrap(),
            Reply::Done
        );
        assert_eq!(shown.await.unwrap(), tandem_proto::ProbeOutcome::Seen);
        assert_eq!(
            c.next_event().await.unwrap(),
            Some(AgentToUi::Probe(tandem_proto::ProbeEvent::Hide { id }))
        );
        // Contestar una que ya no está se rechaza.
        assert!(matches!(
            c.request(UiCommand::AnswerProbe { id, seen: true })
                .await
                .unwrap(),
            Reply::Rejected(_)
        ));
    }

    #[tokio::test]
    async fn pushed_state_during_request_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = IpcClient::handshake(io).await.unwrap();
        c.get_state().await.unwrap();
        let mut s = agent.state();
        s.pc_on = true;
        agent.set_state(s.clone()).unwrap();
        // El aviso llega antes que la respuesta; no se pierde.
        assert_eq!(c.get_state().await.unwrap(), s);
        assert_eq!(c.next_event().await.unwrap(), Some(AgentToUi::State(s)));
    }

    #[tokio::test]
    async fn server_rejects_other_protocol_version() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (client, server) = duplex(64);
        let (_stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve_connection(server, agent, stop_rx));
        let mut c = Framed::new(client);
        c.send(&Hello {
            version: PROTOCOL_VERSION + 1,
            peer: Peer::Ui,
        })
        .await
        .unwrap();
        let err = task.await.unwrap().unwrap_err();
        assert!(
            matches!(err, IpcError::Proto(ProtoError::IncompatibleVersion { .. })),
            "{err}"
        );
    }

    #[tokio::test]
    async fn server_rejects_an_agent_as_ui() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (client, server) = duplex(64);
        let (_stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve_connection(server, agent, stop_rx));
        let mut c = Framed::new(client);
        c.send(&Hello::new(Peer::Agent(Machine::Pc))).await.unwrap();
        let err = task.await.unwrap().unwrap_err();
        assert!(matches!(err, IpcError::UnexpectedPeer { .. }), "{err}");
    }

    #[tokio::test]
    async fn client_rejects_a_ui_as_agent() {
        let (client, server) = duplex(64);
        tokio::spawn(async move {
            let mut s = Framed::new(server);
            s.send(&Hello::new(Peer::Ui)).await.unwrap();
            let _ = s.recv_hello().await;
        });
        let err = IpcClient::handshake(client).await.unwrap_err();
        assert!(matches!(err, IpcError::UnexpectedPeer { .. }), "{err}");
    }

    #[tokio::test]
    async fn shutdown_ends_the_connection() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (client, server) = duplex(64);
        let (stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve_connection(server, agent, stop_rx));
        let mut c = IpcClient::handshake(client).await.unwrap();
        c.get_state().await.unwrap();
        stop.send_replace(true);
        task.await.unwrap().unwrap();
        assert!(matches!(c.next_event().await, Ok(None)));
    }

    // QA ronda 1: una UI puede mandar varios pedidos sin esperar respuesta; cada
    // respuesta lleva su id y salen en orden.
    #[tokio::test]
    async fn pipelined_requests_are_answered_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, io, _stop) = start(dir.path());
        let mut c = Framed::new(io);
        c.send(&Hello::new(Peer::Ui)).await.unwrap();
        assert_eq!(
            c.recv_hello().await.unwrap().peer,
            Peer::Agent(Machine::Mac)
        );
        let ids = [7u64, 3, u64::MAX, 3];
        for id in ids {
            c.send(&UiRequest {
                id,
                command: UiCommand::GetState,
            })
            .await
            .unwrap();
        }
        for id in ids {
            assert_eq!(
                c.recv::<AgentToUi>().await.unwrap(),
                Some(AgentToUi::Reply {
                    id,
                    reply: Reply::State(agent.state())
                })
            );
        }
    }

    #[tokio::test]
    async fn garbage_request_ends_only_that_connection() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (stop, stop_rx) = watch::channel(false);
        let (bad_client, bad_server) = duplex(64);
        let bad = tokio::spawn(serve_connection(bad_server, agent.clone(), stop_rx.clone()));
        let (good_client, good_server) = duplex(64);
        tokio::spawn(serve_connection(good_server, agent.clone(), stop_rx));

        let mut b = Framed::new(bad_client);
        b.send(&Hello::new(Peer::Ui)).await.unwrap();
        b.recv_hello().await.unwrap();
        // Una trama bien formada cuyo contenido no es un `UiRequest`.
        b.io.write_all(&[3, 0, 0, 0, 0xFF, 0xFF, 0xFF])
            .await
            .unwrap();
        let err = bad.await.unwrap().unwrap_err();
        assert!(
            matches!(err, IpcError::Proto(ProtoError::Decode(_))),
            "{err}"
        );

        // La otra UI sigue atendida.
        let mut g = IpcClient::handshake(good_client).await.unwrap();
        assert_eq!(g.get_state().await.unwrap(), agent.state());
        drop(stop);
    }

    #[tokio::test]
    async fn oversized_frame_from_ui_is_rejected_without_reading_it() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (client, server) = duplex(64);
        let (_stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve_connection(server, agent, stop_rx));
        let mut c = Framed::new(client);
        c.send(&Hello::new(Peer::Ui)).await.unwrap();
        c.recv_hello().await.unwrap();
        // Solo la cabecera: anuncia 4 GiB y el agente corta sin esperar el cuerpo.
        c.io.write_all(&u32::MAX.to_le_bytes()).await.unwrap();
        let err = task.await.unwrap().unwrap_err();
        assert!(
            matches!(err, IpcError::Proto(ProtoError::FrameTooLarge { .. })),
            "{err}"
        );
    }

    #[tokio::test]
    async fn request_before_hello_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Arc::new(agent(dir.path()));
        let (client, server) = duplex(64);
        let (_stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(serve_connection(server, agent, stop_rx));
        let mut c = Framed::new(client);
        c.recv_hello().await.unwrap();
        // Un pedido en lugar del saludo: no se atiende.
        c.send(&UiRequest {
            id: 0,
            command: UiCommand::GetState,
        })
        .await
        .unwrap();
        assert!(task.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn clean_close_between_messages_is_none() {
        let (a, b) = duplex(64);
        drop(b);
        let mut f = Framed::new(a);
        assert!(matches!(f.recv::<UiRequest>().await, Ok(None)));
    }

    #[tokio::test]
    async fn close_mid_frame_is_an_error() {
        let (a, mut b) = duplex(64);
        b.write_all(&[10, 0, 0, 0, 1]).await.unwrap();
        drop(b);
        let mut f = Framed::new(a);
        assert!(matches!(f.recv::<UiRequest>().await, Err(IpcError::Io(_))));
    }

    #[tokio::test]
    async fn hello_on_closed_stream_is_closed() {
        let (a, b) = duplex(64);
        drop(b);
        let mut f = Framed::new(a);
        assert!(matches!(f.recv_hello().await, Err(IpcError::Closed)));
    }

    #[tokio::test]
    async fn request_on_closed_connection_fails() {
        let (client, server) = duplex(64);
        tokio::spawn(async move {
            let mut s = Framed::new(server);
            s.send(&Hello::new(Peer::Agent(Machine::Pc))).await.unwrap();
            let _ = s.recv_hello().await;
            let _ = s.recv::<UiRequest>().await;
        });
        let mut c = IpcClient::handshake(client).await.unwrap();
        assert!(matches!(c.get_state().await, Err(IpcError::Closed)));
    }
}
