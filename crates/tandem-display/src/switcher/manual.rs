//! Backend manual: para monitores sin DDC (o sin perfil probado), el usuario cambia la
//! entrada con los botones del monitor y lo confirma en la UI.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use tandem_core::{Input, Machine, MonitorId};
use tokio::sync::{broadcast, oneshot};

use super::{MonitorSwitcher, SwitchError, SwitchFuture, SwitchRequest};

/// Lo que la UI le muestra al usuario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualPrompt {
    /// Con este número se contesta en [`ManualSwitcher::answer`].
    pub id: u64,
    pub monitor: MonitorId,
    pub label: String,
    pub to: Machine,
    pub input: Input,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualEvent {
    Requested(ManualPrompt),
    /// El pedido ya no espera respuesta (contestado, vencido o cancelado): la UI lo quita.
    Closed {
        id: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no hay un cambio manual pendiente con el número {0}")]
pub struct UnknownPrompt(pub u64);

/// Avisos guardados para una UI lenta.
const EVENT_BUFFER: usize = 32;

pub struct ManualSwitcher {
    events: broadcast::Sender<ManualEvent>,
    pending: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    next: AtomicU64,
}

impl Default for ManualSwitcher {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ManualSwitcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManualSwitcher").finish_non_exhaustive()
    }
}

impl ManualSwitcher {
    pub fn new() -> Self {
        ManualSwitcher {
            events: broadcast::channel(EVENT_BUFFER).0,
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        }
    }

    /// Pedidos al usuario y su cierre, para la UI.
    pub fn subscribe(&self) -> broadcast::Receiver<ManualEvent> {
        self.events.subscribe()
    }

    /// La respuesta del usuario: `done` si ya cambió la entrada, `false` si no puede.
    pub fn answer(&self, id: u64, done: bool) -> Result<(), UnknownPrompt> {
        let tx = self.lock().remove(&id).ok_or(UnknownPrompt(id))?;
        // Si justo venció, nadie espera la respuesta; el cierre ya lo avisa el pedido.
        let _ = tx.send(done);
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, oneshot::Sender<bool>>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Quita el pedido y avisa a la UI al terminar, también si el paso venció y se soltó
/// el futuro: si no, la UI seguiría pidiendo un cambio que ya nadie espera.
struct Pending<'a> {
    owner: &'a ManualSwitcher,
    id: u64,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.owner.lock().remove(&self.id);
        let _ = self.owner.events.send(ManualEvent::Closed { id: self.id });
    }
}

impl MonitorSwitcher for ManualSwitcher {
    fn switch<'a>(&'a self, req: &'a SwitchRequest) -> SwitchFuture<'a> {
        Box::pin(async move {
            let monitor = req.monitor.id.clone();
            let id = self.next.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = oneshot::channel();
            self.lock().insert(id, tx);
            let _pending = Pending { owner: self, id };
            let prompt = ManualPrompt {
                id,
                monitor: monitor.clone(),
                label: req.monitor.label.clone(),
                to: req.to,
                input: req.input,
            };
            // Sin nadie mirando, esperar solo haría vencer el paso más tarde.
            if self.events.send(ManualEvent::Requested(prompt)).is_err() {
                return Err(SwitchError::NoUi(monitor));
            }
            match rx.await {
                Ok(true) => Ok(()),
                Ok(false) | Err(_) => Err(SwitchError::Declined(monitor)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::switcher::tests::request;

    async fn next_prompt(rx: &mut broadcast::Receiver<ManualEvent>) -> ManualPrompt {
        match rx.recv().await.unwrap() {
            ManualEvent::Requested(p) => p,
            other => panic!("se esperaba un pedido: {other:?}"),
        }
    }

    #[tokio::test]
    async fn asks_the_ui_and_waits_for_confirmation() {
        let s = Arc::new(ManualSwitcher::new());
        let mut ui = s.subscribe();
        let task = tokio::spawn({
            let s = s.clone();
            async move { s.switch(&request("m32", Machine::Pc)).await }
        });
        let p = next_prompt(&mut ui).await;
        assert_eq!(
            p,
            ManualPrompt {
                id: p.id,
                monitor: "m32".into(),
                label: "32\"".into(),
                to: Machine::Pc,
                input: Input::DisplayPort,
            }
        );
        // Sin respuesta, el cambio sigue esperando.
        tokio::task::yield_now().await;
        assert!(!task.is_finished());

        s.answer(p.id, true).unwrap();
        assert_eq!(task.await.unwrap(), Ok(()));
        assert_eq!(ui.recv().await.unwrap(), ManualEvent::Closed { id: p.id });
        // Ya contestado, no se puede contestar otra vez.
        assert_eq!(s.answer(p.id, true), Err(UnknownPrompt(p.id)));
    }

    #[tokio::test]
    async fn the_user_can_decline() {
        let s = Arc::new(ManualSwitcher::new());
        let mut ui = s.subscribe();
        let task = tokio::spawn({
            let s = s.clone();
            async move { s.switch(&request("m32", Machine::Mac)).await }
        });
        let p = next_prompt(&mut ui).await;
        assert_eq!(p.input, Input::Hdmi);
        s.answer(p.id, false).unwrap();
        assert_eq!(
            task.await.unwrap(),
            Err(SwitchError::Declined("m32".into()))
        );
    }

    #[tokio::test]
    async fn without_ui_fails_at_once() {
        let s = ManualSwitcher::new();
        let err = s.switch(&request("m32", Machine::Pc)).await.unwrap_err();
        assert_eq!(err, SwitchError::NoUi("m32".into()));
        assert!(s.lock().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_timed_out_request_is_closed_for_the_ui() {
        let s = ManualSwitcher::new();
        let mut ui = s.subscribe();
        let req = request("m32", Machine::Pc);
        let r = tokio::time::timeout(Duration::from_secs(10), s.switch(&req)).await;
        assert!(r.is_err(), "tenía que vencer");
        let p = next_prompt(&mut ui).await;
        assert_eq!(ui.recv().await.unwrap(), ManualEvent::Closed { id: p.id });
        // La respuesta tardía no encuentra el pedido.
        assert_eq!(s.answer(p.id, true), Err(UnknownPrompt(p.id)));
    }

    #[tokio::test]
    async fn each_request_gets_its_own_number() {
        let s = Arc::new(ManualSwitcher::new());
        let mut ui = s.subscribe();
        let spawn = |m: &'static str| {
            let s = s.clone();
            tokio::spawn(async move { s.switch(&request(m, Machine::Pc)).await })
        };
        let (a, b) = (spawn("m32"), spawn("m34"));
        let (p1, p2) = (next_prompt(&mut ui).await, next_prompt(&mut ui).await);
        assert_ne!(p1.id, p2.id);
        let (first, second) = if p1.monitor == MonitorId::from("m32") {
            (p1, p2)
        } else {
            (p2, p1)
        };
        s.answer(second.id, false).unwrap();
        s.answer(first.id, true).unwrap();
        assert_eq!(a.await.unwrap(), Ok(()));
        assert_eq!(b.await.unwrap(), Err(SwitchError::Declined("m34".into())));
    }
}
