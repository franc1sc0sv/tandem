//! Arrastre de archivos al cruzar el borde (T-121), del lado del motor de entrada.
//!
//! Con el botón apretado, un detector del sistema ([`FileDragBackend`]) dice si lo que se
//! arrastra son archivos. Si es así se enciende la bandera que deja cruzar a la sesión
//! aunque «Arrastrar entre máquinas» esté apagado; al cruzar, [`drag::run_origin`] copia los
//! archivos al otro lado y espera cómo termina allí. Soltar el botón o volver al borde antes
//! de terminar la copia cancela y el destino borra lo copiado.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tandem_files::FilesError;
use tandem_files::drag::{DragDetector, Upload};
use tandem_files::drag_os::{PxRect, StripEdge};
use tandem_input::geometry::{Layout, Rect, Side};
use tokio::sync::watch;

use super::Shared;
use crate::agent::Agent;
use crate::drag::{self, FileDragBackend};

/// Cada cuánto se pregunta al detector mientras hay un botón apretado.
pub const POLL: Duration = Duration::from_millis(20);

/// Grosor de la franja del detector, en píxeles de pantalla.
pub const STRIP_THICKNESS: i32 = 4;

/// El borde de cruce de una máquina: la franja va por dentro del borde de la caja que
/// envuelve sus pantallas, del lado que mira a la otra.
pub fn edge_target(screens: &[Rect], facing: Side) -> Option<(PxRect, StripEdge)> {
    let first = screens.first()?;
    let (mut l, mut t) = (first.x, first.y);
    let (mut r, mut b) = (first.x + first.width as i32, first.y + first.height as i32);
    for s in &screens[1..] {
        l = l.min(s.x);
        t = t.min(s.y);
        r = r.max(s.x + s.width as i32);
        b = b.max(s.y + s.height as i32);
    }
    let edge = match facing {
        Side::Left => StripEdge::Left,
        Side::Right => StripEdge::Right,
        Side::Up => StripEdge::Top,
        Side::Down => StripEdge::Bottom,
    };
    Some(((l, t, r - l, b - t), edge))
}

struct HookDetector(Arc<dyn FileDragBackend>);

impl DragDetector for HookDetector {
    fn file_drag(&self) -> Option<Vec<PathBuf>> {
        self.0.file_drag()
    }
}

struct HookUpload<'a> {
    hook: &'a dyn FileDragBackend,
    agent: &'a Agent,
}

impl Upload for HookUpload<'_> {
    async fn upload(&self, local: &Path, remote: &str) -> Result<(), FilesError> {
        self.hook.upload(self.agent, local, remote).await
    }
}

/// Espera a que `rx` valga `value`; `false` si el emisor se soltó.
async fn until(rx: &mut watch::Receiver<bool>, value: bool) -> bool {
    rx.wait_for(|v| *v == value).await.is_ok()
}

/// Sigue la franja del borde con la geometría de ahora.
pub(super) async fn follow_edge(sh: Arc<Shared>, hook: Arc<dyn FileDragBackend>) {
    let mut layout = sh.layout.subscribe();
    loop {
        let target = {
            let l: &Layout = &layout.borrow_and_update();
            l.facing(sh.me).and_then(|side| {
                let rects: Vec<Rect> = l.screens(sh.me).iter().map(|s| s.rect).collect();
                edge_target(&rects, side)
            })
        };
        hook.set_edge(target);
        if layout.changed().await.is_err() {
            hook.set_edge(None);
            return;
        }
    }
}

/// Mientras hay un botón apretado, mira si se arrastran archivos y, si el cursor cruza, los
/// lleva al otro lado.
pub(super) async fn follow(sh: Arc<Shared>, hook: Arc<dyn FileDragBackend>) {
    let (mut held, mut remote) = (sh.held.subscribe(), sh.remote.subscribe());
    loop {
        if !until(&mut held, true).await {
            return;
        }
        let Some(files) = detect(&sh, &hook, &mut held, &mut remote).await else {
            sh.file_drag.store(false, Ordering::Relaxed);
            continue;
        };
        let cancelled = async {
            // La copia se cancela al soltar el botón o si el cursor vuelve.
            tokio::select! {
                _ = until(&mut held, false) => {}
                _ = until(&mut remote, false) => {}
            }
        };
        let uploader = HookUpload {
            hook: hook.as_ref(),
            agent: &sh.agent,
        };
        let end = drag::run_origin(
            &sh.agent,
            HookDetector(hook.clone()),
            uploader,
            &files,
            cancelled,
        )
        .await;
        hook.finish(&sh.agent).await;
        sh.file_drag.store(false, Ordering::Relaxed);
        tracing::debug!(?end, "arrastre de archivos terminado");
        // Un solo arrastre por pulsación.
        if !until(&mut held, false).await {
            return;
        }
    }
}

/// Con el botón apretado: los archivos arrastrados cuando el cursor cruza, o `None` si se
/// soltó el botón antes (o el cursor ya estaba afuera sin archivos).
async fn detect(
    sh: &Shared,
    hook: &Arc<dyn FileDragBackend>,
    held: &mut watch::Receiver<bool>,
    remote: &mut watch::Receiver<bool>,
) -> Option<Vec<PathBuf>> {
    let mut tick = tokio::time::interval(POLL);
    let mut files: Option<Vec<PathBuf>> = None;
    loop {
        // Cruzó con archivos detectados.
        if *remote.borrow() && files.is_some() {
            return files;
        }
        if !*held.borrow() {
            return None;
        }
        tokio::select! {
            _ = tick.tick() => match hook.file_drag() {
                Some(f) => {
                    files = Some(f);
                    sh.file_drag.store(true, Ordering::Relaxed);
                }
                // Cruzado, el detector deja de verlos (el cursor salió de la franja); se
                // conservan los de antes. Sin cruzar, ya no hay arrastre de archivos.
                None if !*remote.borrow() => {
                    files = None;
                    sh.file_drag.store(false, Ordering::Relaxed);
                }
                None => {}
            },
            r = remote.changed() => if r.is_err() { return None },
            h = held.changed() => if h.is_err() { return None },
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect::new(x, y, w, h)
    }

    #[test]
    fn la_franja_sigue_el_borde_que_mira_a_la_otra_maquina() {
        let screens = [rect(0, 0, 1920, 1080), rect(1920, 0, 1280, 1024)];
        assert_eq!(
            edge_target(&screens, Side::Right),
            Some(((0, 0, 3200, 1080), StripEdge::Right))
        );
        assert_eq!(
            edge_target(&screens, Side::Left),
            Some(((0, 0, 3200, 1080), StripEdge::Left))
        );
    }

    #[test]
    fn sin_pantallas_no_hay_franja() {
        assert_eq!(edge_target(&[], Side::Right), None);
    }

    proptest! {
        #[test]
        fn la_caja_envuelve_todas_las_pantallas(
            rects in proptest::collection::vec((-3000i32..3000, -3000i32..3000, 1u32..4000, 1u32..4000), 1..5)
        ) {
            let screens: Vec<Rect> = rects.iter().map(|&(x, y, w, h)| rect(x, y, w, h)).collect();
            let ((x, y, w, h), _) = edge_target(&screens, Side::Right).unwrap();
            for s in &screens {
                prop_assert!(s.x >= x && s.y >= y);
                prop_assert!(s.x + s.width as i32 <= x + w && s.y + s.height as i32 <= y + h);
            }
        }
    }
}
