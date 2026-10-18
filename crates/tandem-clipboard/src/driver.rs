//! Driver: conecta [`ClipSync`] con el portapapeles del sistema y con un stream
//! confiable de `tandem-link`.
//!
//! Cada `run` dura lo que dura un enlace. El stream lo abre y lo acepta el
//! cableado del agente (es un stream propio del portapapeles, aparte del de la
//! entrada, para que una imagen grande no retrase las teclas); el driver lo parte
//! en sus dos mitades para seguir leyendo mientras manda una transferencia.
//! El estado de la sincronización sobrevive entre sesiones.

use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;

use tandem_link::transport::{ControlSender, ControlStream, TransportError};
use tokio::sync::{mpsc, watch};
use tokio::time::{self, MissedTickBehavior};

use crate::clipboard::Clipboard;
use crate::content::ClipContent;
use crate::msg::ClipMsg;
use crate::sync::{Action, ClipSync, SyncConfig, SyncStats};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipConfig {
    /// Cada cuánto se mira el contador de cambios del sistema.
    pub poll: Duration,
    pub sync: SyncConfig,
}

impl Default for ClipConfig {
    fn default() -> Self {
        Self {
            // Copiar y cambiar de máquina para pegar lleva más que esto.
            poll: Duration::from_millis(250),
            sync: SyncConfig::default(),
        }
    }
}

/// Por qué terminó una sesión.
#[derive(Debug, thiserror::Error)]
pub enum SyncEnd {
    #[error("detenida: el enlace cambió")]
    Stopped,
    #[error("enlace: {0}")]
    Link(#[from] TransportError),
}

/// Espera progresiva cuando otra app tiene el portapapeles abierto y no se puede leer
/// (en Windows, «being held by another party»): sin esto, cada vuelta del sondeo reintenta
/// y vuelve a quejarse en el log (T-087).
///
/// Cada fallo seguido espera el doble que el anterior (con `base` de partida, hasta
/// [`ReadBackoff::MAX`]) y solo el primero de la racha se avisa en el log.
#[derive(Debug, Default)]
pub struct ReadBackoff {
    failures: u32,
    retry_at: Option<Instant>,
}

impl ReadBackoff {
    /// Lo máximo que se espera entre dos intentos.
    pub const MAX: Duration = Duration::from_secs(5);

    /// Si ya toca volver a intentar.
    pub fn ready(&self, now: Instant) -> bool {
        self.retry_at.is_none_or(|at| now >= at)
    }

    /// La lectura falló. `true` si es la primera de la racha: la única que se avisa.
    pub fn failed(&mut self, now: Instant, base: Duration) -> bool {
        let first = self.failures == 0;
        let shift = self.failures.min(16);
        self.failures = self.failures.saturating_add(1);
        let wait = base
            .checked_mul(1u32 << shift)
            .map_or(Self::MAX, |d| d.min(Self::MAX));
        self.retry_at = Some(now + wait);
        first
    }

    /// La lectura salió. `Some(n)` si cerraba una racha de `n` fallos.
    pub fn succeeded(&mut self) -> Option<u32> {
        self.retry_at = None;
        match std::mem::take(&mut self.failures) {
            0 => None,
            n => Some(n),
        }
    }
}

pub struct ClipDriver<C> {
    sync: ClipSync,
    clipboard: C,
    /// Último valor visto del contador del sistema.
    last_count: Option<u64>,
    focus: Option<watch::Receiver<bool>>,
    stats: watch::Sender<SyncStats>,
    cfg: ClipConfig,
}

impl<C: Clipboard> ClipDriver<C> {
    /// `focused`: si esta máquina tiene ahora el teclado y el mouse.
    pub fn new(clipboard: C, cfg: ClipConfig, focused: bool) -> Self {
        Self {
            sync: ClipSync::new(cfg.sync, focused),
            clipboard,
            last_count: None,
            focus: None,
            stats: watch::Sender::new(SyncStats::default()),
            cfg,
        }
    }

    /// Foco en caliente: `true` mientras el teclado y el mouse están en esta
    /// máquina. Es cuando se piden las copias grandes del otro lado.
    pub fn with_focus(mut self, mut focus: watch::Receiver<bool>) -> Self {
        let now = *focus.borrow_and_update();
        // Sin sesión no hay nada que mandar.
        let _ = self.sync.set_focus(now);
        self.focus = Some(focus);
        self
    }

    /// Contadores de la sincronización, al día mientras corre.
    pub fn stats(&self) -> watch::Receiver<SyncStats> {
        self.stats.subscribe()
    }

    pub fn sync(&self) -> &ClipSync {
        &self.sync
    }

    pub fn clipboard(&self) -> &C {
        &self.clipboard
    }

    pub async fn run(&mut self, stream: ControlStream, stop: impl Future<Output = ()>) -> SyncEnd {
        let end = self.serve(stream, stop).await;
        self.stats.send_replace(self.sync.stats());
        tracing::info!("sesión del portapapeles terminada: {end}");
        end
    }

    async fn serve(&mut self, stream: ControlStream, stop: impl Future<Output = ()>) -> SyncEnd {
        let (mut tx, mut rx) = stream.split();
        let (inbox_tx, mut inbox) = mpsc::unbounded_channel();
        // La lectura va aparte: mientras se manda una imagen y el control de
        // flujo frena la escritura, lo que llega del otro lado se sigue leyendo.
        let reader = async move {
            loop {
                match rx.recv::<ClipMsg>().await {
                    Ok(Some(msg)) => {
                        if inbox_tx.send(msg).is_err() {
                            return SyncEnd::Stopped;
                        }
                    }
                    Ok(None) => return TransportError::ControlClosed.into(),
                    Err(e) => return e.into(),
                }
            }
        };

        let Self {
            sync,
            clipboard,
            last_count,
            focus,
            stats,
            cfg,
        } = self;
        let mut backoff = ReadBackoff::default();
        let main = async {
            if let Some(f) = focus.as_mut() {
                let _ = sync.set_focus(*f.borrow_and_update());
            }
            let acts = match read_local(clipboard, last_count, &mut backoff, cfg.poll) {
                Some(local) => sync.start(local),
                None => sync.resume(),
            };
            apply(clipboard, last_count, &mut tx, acts).await?;
            stats.send_replace(sync.stats());

            let mut poll = time::interval(cfg.poll);
            poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    biased;
                    msg = inbox.recv() => {
                        // Si el lector terminó, el otro brazo del select lo dice.
                        let Some(msg) = msg else { std::future::pending::<()>().await; continue };
                        let acts = sync.on_message(msg);
                        apply(clipboard, last_count, &mut tx, acts).await?;
                        stats.send_replace(sync.stats());
                    }
                    focused = next_focus(focus) => match focused {
                        Some(f) => {
                            let acts = sync.set_focus(f);
                            apply(clipboard, last_count, &mut tx, acts).await?;
                        stats.send_replace(sync.stats());
                        }
                        None => *focus = None,
                    },
                    _ = poll.tick() => {
                        if backoff.ready(Instant::now())
                            && changed(clipboard, last_count)
                            && let Some(local) = read_local(clipboard, last_count, &mut backoff, cfg.poll)
                        {
                            let acts = sync.local_changed(local);
                            apply(clipboard, last_count, &mut tx, acts).await?;
                        stats.send_replace(sync.stats());
                        }
                    }
                    () = std::future::ready(()), if sync.is_sending() => {
                        // Un trozo por vuelta: entre trozos se atiende lo demás.
                        if let Some(chunk) = sync.next_chunk() {
                            tx.send(&chunk).await?;
                        }
                    }
                }
            }
        };
        let main = async {
            let r: Result<std::convert::Infallible, TransportError> = main.await;
            match r {
                Err(e) => SyncEnd::Link(e),
                Ok(never) => match never {},
            }
        };

        tokio::pin!(stop);
        tokio::select! {
            biased;
            () = &mut stop => SyncEnd::Stopped,
            end = reader => end,
            end = main => end,
        }
    }
}

async fn next_focus(focus: &mut Option<watch::Receiver<bool>>) -> Option<bool> {
    match focus {
        Some(rx) => match rx.changed().await {
            Ok(()) => Some(*rx.borrow_and_update()),
            Err(_) => None,
        },
        None => std::future::pending().await,
    }
}

/// Si el contador del sistema se movió desde la última vez.
fn changed<C: Clipboard>(clipboard: &mut C, last: &Option<u64>) -> bool {
    match clipboard.change_count() {
        Ok(n) => Some(n) != *last,
        Err(e) => {
            tracing::debug!("portapapeles: contador: {e}");
            false
        }
    }
}

/// Lee el contenido y deja anotado el contador que le corresponde. El contador
/// se toma antes de leer: si cambia mientras tanto, la próxima vuelta lo ve.
/// `None` si no se pudo leer (otra app lo tiene abierto): se reintenta en la
/// próxima vuelta.
fn read_local<C: Clipboard>(
    clipboard: &mut C,
    last: &mut Option<u64>,
    backoff: &mut ReadBackoff,
    base: Duration,
) -> Option<Option<ClipContent>> {
    match clipboard.change_count() {
        Ok(n) => *last = Some(n),
        Err(e) => tracing::debug!("portapapeles: contador: {e}"),
    }
    match clipboard.read() {
        Ok(c) => {
            if let Some(n) = backoff.succeeded() {
                tracing::info!("portapapeles: se pudo leer de nuevo tras {n} intentos");
            }
            Some(c)
        }
        Err(e) => {
            // Un solo aviso por racha: el resto, a depuración, con espera creciente.
            if backoff.failed(Instant::now(), base) {
                tracing::warn!("portapapeles: no se pudo leer ({e}); se reintenta con espera");
            } else {
                tracing::debug!("portapapeles: sigue sin poder leerse: {e}");
            }
            *last = None;
            None
        }
    }
}

async fn apply<C: Clipboard>(
    clipboard: &mut C,
    last: &mut Option<u64>,
    tx: &mut ControlSender,
    actions: Vec<Action>,
) -> Result<(), TransportError> {
    for action in actions {
        match action {
            Action::Send(msg) => tx.send(&msg).await?,
            Action::Write(content) => {
                if let Err(e) = clipboard.write(&content) {
                    tracing::error!("portapapeles: no se pudo escribir: {e}");
                }
                // Nuestra propia escritura no es un cambio del usuario: ni se lee.
                // Si el sistema la convirtió y otra cosa la vuelve a tocar, el
                // hash igual la frena en `ClipSync`. Una copia del usuario justo
                // entre la escritura y esta lectura se perdería, pero son
                // microsegundos y el usuario no está copiando en esta máquina
                // mientras pega lo que vino de la otra.
                match clipboard.change_count() {
                    Ok(n) => *last = Some(n),
                    Err(e) => tracing::debug!("portapapeles: contador: {e}"),
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: Duration = Duration::from_millis(250);

    #[test]
    fn waits_double_each_time_up_to_the_cap() {
        let t0 = Instant::now();
        let mut b = ReadBackoff::default();
        assert!(b.ready(t0));
        let mut waits = Vec::new();
        for _ in 0..8 {
            b.failed(t0, BASE);
            // El primer instante en que ya toca: busca por intervalos de 50 ms.
            let w = (0..200u64)
                .map(|i| Duration::from_millis(i * 50))
                .find(|d| b.ready(t0 + *d))
                .unwrap();
            waits.push(w.as_millis());
        }
        assert_eq!(waits, [250, 500, 1000, 2000, 4000, 5000, 5000, 5000]);
    }

    #[test]
    fn only_the_first_failure_of_a_streak_is_reported() {
        let t0 = Instant::now();
        let mut b = ReadBackoff::default();
        assert!(b.failed(t0, BASE));
        assert!(!b.failed(t0, BASE));
        assert!(!b.failed(t0, BASE));
        assert_eq!(b.succeeded(), Some(3));
        // Una racha nueva vuelve a avisar una vez.
        assert!(b.failed(t0, BASE));
        assert_eq!(b.succeeded(), Some(1));
        assert_eq!(b.succeeded(), None);
        assert!(b.ready(t0));
    }

    #[test]
    fn a_huge_streak_never_overflows() {
        let t0 = Instant::now();
        let mut b = ReadBackoff::default();
        for _ in 0..200 {
            b.failed(t0, Duration::from_secs(3600));
        }
        assert!(!b.ready(t0 + Duration::from_secs(4)));
        assert!(b.ready(t0 + ReadBackoff::MAX));
    }
}
