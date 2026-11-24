//! File drag when crossing the edge (T-121), on the input engine's side.
//!
//! With the button pressed, a system detector ([`FileDragBackend`]) says whether what is being
//! dragged is files. If so, the flag is turned on that lets the session cross
//! even if "Drag between machines" is off; when crossing, [`drag::run_origin`] copies the
//! files to the other side and waits for how it ends there. Releasing the button or returning to the edge before
//! the copy finishes cancels and the destination deletes what was copied.

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

/// How often the detector is asked while a button is pressed.
pub const POLL: Duration = Duration::from_millis(20);

/// Thickness of the detector's strip, in screen pixels.
pub const STRIP_THICKNESS: i32 = 4;

/// A machine's crossing edge: the strip goes inside the edge of the box that
/// wraps its screens, on the side facing the other.
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

/// Waits for `rx` to be `value`; `false` if the sender was dropped.
async fn until(rx: &mut watch::Receiver<bool>, value: bool) -> bool {
    rx.wait_for(|v| *v == value).await.is_ok()
}

/// Follows the edge strip with the current geometry.
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

/// While a button is pressed, it looks at whether files are dragged and, if the cursor crosses, carries them
/// to the other side.
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
            // The copy is canceled on releasing the button or if the cursor returns.
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
        tracing::debug!(?end, "file drag finished");
        // A single drag per press.
        if !until(&mut held, false).await {
            return;
        }
    }
}

/// With the button pressed: the files dragged when the cursor crosses, or `None` if the
/// button was released before (or the cursor was already outside without files).
async fn detect(
    sh: &Shared,
    hook: &Arc<dyn FileDragBackend>,
    held: &mut watch::Receiver<bool>,
    remote: &mut watch::Receiver<bool>,
) -> Option<Vec<PathBuf>> {
    let mut tick = tokio::time::interval(POLL);
    let mut files: Option<Vec<PathBuf>> = None;
    loop {
        // Crossed with files detected.
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
                // Once crossed, the detector stops seeing them (the cursor left the strip); the
                // earlier ones are kept. Without crossing, there is no file drag anymore.
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
    fn the_strip_follows_the_edge_facing_the_other_machine() {
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
    fn without_screens_there_is_no_strip() {
        assert_eq!(edge_target(&[], Side::Right), None);
    }

    proptest! {
        #[test]
        fn the_box_wraps_all_the_screens(
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
