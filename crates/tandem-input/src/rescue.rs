//! Window rescue (T-360): while a screen of this machine belongs to a monitor that shows the
//! other machine, the windows on it are moved to a screen somebody sees.
//!
//! Windows keeps the DP display of such a monitor on its desktop (the monitor keeps DP alive
//! while it shows HDMI), and it cannot be detached: the PC is the only DDC/CI path to the
//! monitor and a detached display has no `HMONITOR`. macOS turns the display off (T-341) and
//! moves the windows itself, so there it is a no-op.
//!
//! The decision logic is here and portable; the system calls are behind [`WindowMover`]
//! (`windows::rescue` for the real one, [`FakeWindowMover`] for the tests).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use tandem_core::DisplayKey;

use crate::geometry::Rect;

/// A top-level window as the system numbers it (the `HWND` on Windows).
pub type WindowId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowState {
    Normal,
    Maximized,
    Minimized,
}

/// A user top-level window: where it is now and where it goes when restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: WindowId,
    /// Where it is now (what is on screen, or the minimized icon's last place).
    pub rect: Rect,
    /// Its restore rectangle (`rcNormalPosition`).
    pub normal: Rect,
    pub state: ShowState,
}

/// A screen of this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RescueScreen {
    pub rect: Rect,
    /// Without the taskbar.
    pub work: Rect,
    pub key: Option<DisplayKey>,
    pub primary: bool,
}

/// What to leave a window as: its restore rectangle and whether it is shown, maximized or
/// minimized (a maximized one re-maximizes on the screen its rectangle is on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub normal: Rect,
    pub state: ShowState,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MoveError {
    /// The window belongs to a process this one may not touch (an elevated one, UIPI).
    #[error("access denied")]
    Denied,
    #[error("the window is gone")]
    Gone,
    /// The window's thread does not answer (a hung app): placing it now would block the
    /// agent, so it is tried again on a later turn.
    #[error("the window does not answer")]
    Busy,
    #[error("{0}")]
    Other(String),
}

/// Listing and placing the windows of the desktop.
pub trait WindowMover: Send + Sync + std::fmt::Debug {
    fn screens(&self) -> Result<Vec<RescueScreen>, String>;
    /// The windows that may be moved: the ones that are visible, not cloaked, not tool windows,
    /// not owned, not the shell and not this process's own.
    fn windows(&self) -> Result<Vec<WindowInfo>, String>;
    /// Places a window and returns how it was left (read back from the system).
    fn place(&self, id: WindowId, to: Placement) -> Result<WindowInfo, MoveError>;
}

/// The real mover of this system: Win32 on Windows, none elsewhere (macOS moves its windows).
pub fn system_window_mover() -> Option<Arc<dyn WindowMover>> {
    #[cfg(windows)]
    {
        Some(Arc::new(crate::windows::rescue::Win32WindowMover))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

fn overlap(a: &Rect, b: &Rect) -> i64 {
    let w = (i64::from(a.x) + i64::from(a.width)).min(i64::from(b.x) + i64::from(b.width))
        - i64::from(a.x).max(i64::from(b.x));
    let h = (i64::from(a.y) + i64::from(a.height)).min(i64::from(b.y) + i64::from(b.height))
        - i64::from(a.y).max(i64::from(b.y));
    if w > 0 && h > 0 { w * h } else { 0 }
}

/// The screen a rectangle is mostly on, if it touches any.
fn screen_of(r: &Rect, screens: &[RescueScreen]) -> Option<usize> {
    let (i, area) = screens
        .iter()
        .enumerate()
        .map(|(i, s)| (i, overlap(r, &s.rect)))
        .max_by_key(|(i, a)| (*a, std::cmp::Reverse(*i)))?;
    (area > 0).then_some(i)
}

/// `r` taken to `work`: the size clamped to it and the place kept relative to `from`'s corner,
/// pushed inside if it overflows. The result is always inside `work`.
pub fn fit(r: Rect, from: Rect, work: Rect) -> Rect {
    let w = r.width.min(work.width);
    let h = r.height.min(work.height);
    let place = |pos: i32, origin: i32, start: i32, room: u32, size: u32| -> i32 {
        let rel = i64::from(pos) - i64::from(origin);
        let max = i64::from(start) + i64::from(room - size);
        (i64::from(start) + rel).clamp(i64::from(start), max) as i32
    };
    Rect::new(
        place(r.x, from.x, work.x, work.width, w),
        place(r.y, from.y, work.y, work.height, h),
        w,
        h,
    )
}

#[derive(Debug, Clone, Copy)]
struct Moved {
    orig: WindowInfo,
    origin: Option<DisplayKey>,
    after: WindowInfo,
}

#[derive(Debug, Default)]
struct Inner {
    moved: BTreeMap<WindowId, Moved>,
    /// Windows that failed to move: not tried again, and logged once.
    failed: BTreeSet<WindowId>,
}

/// Moves the windows off the screens of monitors that show the other machine and puts them back.
#[derive(Debug)]
pub struct WindowRescue {
    mover: Arc<dyn WindowMover>,
    inner: Mutex<Inner>,
}

impl WindowRescue {
    pub fn new(mover: Arc<dyn WindowMover>) -> Self {
        WindowRescue {
            mover,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// One turn. `hidden` is the EDID identity of the monitors that show the other machine
    /// (the same signal as the displays' guard); a screen of it is not seen by anybody.
    pub fn reconcile(&self, hidden: &BTreeSet<DisplayKey>) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if hidden.is_empty() && inner.moved.is_empty() {
            inner.failed.clear();
            return;
        }
        let (screens, windows) = match (self.mover.screens(), self.mover.windows()) {
            (Ok(s), Ok(w)) => (s, w),
            (Err(e), _) | (_, Err(e)) => {
                tracing::debug!("window rescue: cannot read the desktop: {e}");
                return;
            }
        };
        let is_hidden = |s: &RescueScreen| s.key.is_some_and(|k| hidden.contains(&k));
        let alive: BTreeMap<WindowId, WindowInfo> = windows.iter().map(|w| (w.id, *w)).collect();
        inner.failed.retain(|id| alive.contains_key(id));

        // What we moved: gone or touched by the user, forgotten; the rest goes home when its
        // screen is seen again.
        let records: Vec<(WindowId, Moved)> = inner.moved.iter().map(|(i, m)| (*i, *m)).collect();
        for (id, m) in records {
            let Some(now) = alive.get(&id) else {
                inner.moved.remove(&id);
                continue;
            };
            if !untouched(now, &m.after) {
                inner.moved.remove(&id);
                continue;
            }
            let home_seen = m
                .origin
                .is_some_and(|k| screens.iter().any(|s| s.key == Some(k) && !is_hidden(s)));
            if home_seen {
                inner.moved.remove(&id);
                let back = Placement {
                    normal: m.orig.normal,
                    state: m.orig.state,
                };
                if let Err(e) = self.mover.place(id, back) {
                    tracing::info!(window = id, "a window stays where it was moved: {e}");
                }
            }
        }

        let visible: Vec<&RescueScreen> = screens.iter().filter(|s| !is_hidden(s)).collect();
        let Some(target) = visible
            .iter()
            .find(|s| s.primary)
            .or_else(|| visible.first())
            .copied()
        else {
            return;
        };
        for w in &windows {
            if inner.moved.contains_key(&w.id) || inner.failed.contains(&w.id) {
                continue;
            }
            let basis = if w.state == ShowState::Minimized {
                w.normal
            } else {
                w.rect
            };
            let Some(i) = screen_of(&basis, &screens) else {
                continue;
            };
            let from = screens[i];
            if !is_hidden(&from) {
                continue;
            }
            let to = Placement {
                normal: fit(w.normal, from.rect, target.work),
                state: w.state,
            };
            match self.mover.place(w.id, to) {
                Ok(after) => {
                    inner.moved.insert(
                        w.id,
                        Moved {
                            orig: *w,
                            origin: from.key,
                            after,
                        },
                    );
                }
                Err(MoveError::Gone) => {}
                // Not a failure: the next turn tries it again, once the app answers.
                Err(MoveError::Busy) => {
                    tracing::debug!(window = w.id, "a window does not answer; rescued later");
                }
                Err(e) => {
                    tracing::info!(window = w.id, "a window cannot be rescued: {e}");
                    inner.failed.insert(w.id);
                }
            }
        }
    }
}

/// The window is as we left it: the user has not moved, resized, maximized or minimized it.
fn untouched(now: &WindowInfo, after: &WindowInfo) -> bool {
    now.state == after.state
        && if now.state == ShowState::Minimized {
            now.normal == after.normal
        } else {
            now.rect == after.rect
        }
}

/// A test double: a desktop with screens and windows that records what is placed.
#[derive(Debug, Default)]
pub struct FakeWindowMover {
    inner: Mutex<FakeDesktop>,
}

#[derive(Debug, Default)]
struct FakeDesktop {
    screens: Vec<RescueScreen>,
    windows: Vec<WindowInfo>,
    denied: BTreeSet<WindowId>,
    busy: BTreeSet<WindowId>,
    calls: Vec<(WindowId, Placement)>,
}

impl FakeWindowMover {
    pub fn new(screens: Vec<RescueScreen>, windows: Vec<WindowInfo>) -> Self {
        FakeWindowMover {
            inner: Mutex::new(FakeDesktop {
                screens,
                windows,
                ..FakeDesktop::default()
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeDesktop> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Placing this window fails with access denied (an elevated process).
    pub fn deny(&self, id: WindowId) {
        self.lock().denied.insert(id);
    }

    /// This window's app is hung (`set` true) or answers again (`false`).
    pub fn hang(&self, id: WindowId, set: bool) {
        let mut d = self.lock();
        if set {
            d.busy.insert(id);
        } else {
            d.busy.remove(&id);
        }
    }

    pub fn calls(&self) -> Vec<(WindowId, Placement)> {
        self.lock().calls.clone()
    }

    pub fn window(&self, id: WindowId) -> Option<WindowInfo> {
        self.lock().windows.iter().find(|w| w.id == id).copied()
    }

    pub fn add(&self, w: WindowInfo) {
        self.lock().windows.push(w);
    }

    pub fn close(&self, id: WindowId) {
        self.lock().windows.retain(|w| w.id != id);
    }

    /// The user drags a window.
    pub fn user_move(&self, id: WindowId, rect: Rect) {
        if let Some(w) = self.lock().windows.iter_mut().find(|w| w.id == id) {
            w.rect = rect;
            w.normal = rect;
            w.state = ShowState::Normal;
        }
    }
}

impl WindowMover for FakeWindowMover {
    fn screens(&self) -> Result<Vec<RescueScreen>, String> {
        Ok(self.lock().screens.clone())
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, String> {
        Ok(self.lock().windows.clone())
    }

    fn place(&self, id: WindowId, to: Placement) -> Result<WindowInfo, MoveError> {
        let mut d = self.lock();
        d.calls.push((id, to));
        if d.denied.contains(&id) {
            return Err(MoveError::Denied);
        }
        if d.busy.contains(&id) {
            return Err(MoveError::Busy);
        }
        let screens = d.screens.clone();
        let w = d
            .windows
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or(MoveError::Gone)?;
        w.normal = to.normal;
        w.state = to.state;
        // A maximized window fills the work area of the screen its rectangle is on.
        w.rect = match (to.state, screen_of(&to.normal, &screens)) {
            (ShowState::Maximized, Some(i)) => screens[i].work,
            _ => to.normal,
        };
        Ok(*w)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const KEY_PC: DisplayKey = DisplayKey {
        vendor: 1,
        product: 1,
        serial: 1,
    };
    const KEY_ODYSSEY: DisplayKey = DisplayKey {
        vendor: 2,
        product: 2,
        serial: 2,
    };

    fn screens() -> Vec<RescueScreen> {
        vec![
            RescueScreen {
                rect: Rect::new(-2560, 0, 2560, 1440),
                work: Rect::new(-2560, 0, 2560, 1400),
                key: Some(KEY_ODYSSEY),
                primary: false,
            },
            RescueScreen {
                rect: Rect::new(0, 0, 3440, 1440),
                work: Rect::new(0, 0, 3440, 1400),
                key: Some(KEY_PC),
                primary: true,
            },
        ]
    }

    fn win(id: u64, r: Rect) -> WindowInfo {
        WindowInfo {
            id,
            rect: r,
            normal: r,
            state: ShowState::Normal,
        }
    }

    fn hide() -> BTreeSet<DisplayKey> {
        BTreeSet::from([KEY_ODYSSEY])
    }

    fn setup(windows: Vec<WindowInfo>) -> (Arc<FakeWindowMover>, WindowRescue) {
        let fake = Arc::new(FakeWindowMover::new(screens(), windows));
        (fake.clone(), WindowRescue::new(fake))
    }

    #[test]
    fn a_window_on_the_hidden_screen_moves_to_the_primary_one() {
        let (fake, r) = setup(vec![
            win(1, Rect::new(-2000, 100, 800, 600)),
            win(2, Rect::new(100, 100, 800, 600)),
        ]);
        r.reconcile(&hide());
        let w = fake.window(1).unwrap();
        assert_eq!(w.rect, Rect::new(560, 100, 800, 600));
        assert_eq!(fake.window(2).unwrap().rect, Rect::new(100, 100, 800, 600));
        assert_eq!(fake.calls().len(), 1);
    }

    #[test]
    fn mostly_on_decides_the_screen() {
        // 900 of 1000 px on the hidden screen.
        let (fake, r) = setup(vec![win(1, Rect::new(-900, 0, 1000, 500))]);
        r.reconcile(&hide());
        assert_eq!(fake.calls().len(), 1);
        // 100 of 1000 px on it.
        let (fake, r) = setup(vec![win(1, Rect::new(-100, 0, 1000, 500))]);
        r.reconcile(&hide());
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn a_big_window_is_clamped_to_the_target() {
        let (fake, r) = setup(vec![win(1, Rect::new(-2500, 0, 5000, 3000))]);
        r.reconcile(&hide());
        assert_eq!(fake.window(1).unwrap().rect, Rect::new(0, 0, 3440, 1400));
    }

    #[test]
    fn a_maximized_window_is_maximized_again_on_the_target() {
        let mut w = win(1, Rect::new(-2560, 0, 2560, 1400));
        w.normal = Rect::new(-2000, 100, 800, 600);
        w.state = ShowState::Maximized;
        let (fake, r) = setup(vec![w]);
        r.reconcile(&hide());
        let now = fake.window(1).unwrap();
        assert_eq!(now.state, ShowState::Maximized);
        assert_eq!(now.rect, Rect::new(0, 0, 3440, 1400));
        assert_eq!(now.normal, Rect::new(560, 100, 800, 600));
    }

    #[test]
    fn a_minimized_window_gets_its_restore_rect_on_the_target() {
        // T3 Code: minimized, restore rect on the hidden screen.
        let mut w = win(1, Rect::new(-32000, -32000, 160, 28));
        w.normal = Rect::new(-1403, 0, 1294, 1399);
        w.state = ShowState::Minimized;
        let (fake, r) = setup(vec![w]);
        r.reconcile(&hide());
        let now = fake.window(1).unwrap();
        assert_eq!(now.state, ShowState::Minimized);
        assert_eq!(now.normal, Rect::new(1157, 0, 1294, 1399));
        // Not touched again while it stays like that.
        r.reconcile(&hide());
        assert_eq!(fake.calls().len(), 1);
    }

    #[test]
    fn they_go_back_when_the_screen_is_seen_again() {
        let mut max = win(2, Rect::new(-2560, 0, 2560, 1400));
        max.normal = Rect::new(-2000, 100, 800, 600);
        max.state = ShowState::Maximized;
        let orig = [win(1, Rect::new(-2000, 100, 800, 600)), max];
        let (fake, r) = setup(orig.to_vec());
        r.reconcile(&hide());
        r.reconcile(&hide());
        assert_eq!(fake.calls().len(), 2);
        r.reconcile(&BTreeSet::new());
        assert_eq!(fake.window(1), Some(orig[0]));
        assert_eq!(fake.window(2), Some(orig[1]));
        // Once: nothing more to do.
        r.reconcile(&BTreeSet::new());
        assert_eq!(fake.calls().len(), 4);
    }

    #[test]
    fn a_window_the_user_moved_is_not_put_back() {
        let (fake, r) = setup(vec![win(1, Rect::new(-2000, 100, 800, 600))]);
        r.reconcile(&hide());
        fake.user_move(1, Rect::new(300, 300, 700, 500));
        r.reconcile(&BTreeSet::new());
        assert_eq!(fake.window(1).unwrap().rect, Rect::new(300, 300, 700, 500));
        assert_eq!(fake.calls().len(), 1);
    }

    #[test]
    fn a_closed_window_is_forgotten() {
        let (fake, r) = setup(vec![win(1, Rect::new(-2000, 100, 800, 600))]);
        r.reconcile(&hide());
        fake.close(1);
        r.reconcile(&BTreeSet::new());
        assert_eq!(fake.calls().len(), 1);
    }

    #[test]
    fn new_windows_are_caught_on_later_turns() {
        let (fake, r) = setup(vec![]);
        r.reconcile(&hide());
        fake.add(win(7, Rect::new(-1000, 0, 500, 500)));
        r.reconcile(&hide());
        assert_eq!(fake.window(7).unwrap().rect.x, 1560);
    }

    #[test]
    fn a_denied_window_is_tried_once() {
        let (fake, r) = setup(vec![win(1, Rect::new(-2000, 100, 800, 600))]);
        fake.deny(1);
        r.reconcile(&hide());
        r.reconcile(&hide());
        r.reconcile(&hide());
        assert_eq!(fake.calls().len(), 1);
        assert_eq!(fake.window(1).unwrap().rect.x, -2000);
    }

    #[test]
    fn a_hung_window_is_rescued_once_it_answers() {
        let (fake, r) = setup(vec![win(1, Rect::new(-2000, 100, 800, 600))]);
        fake.hang(1, true);
        r.reconcile(&hide());
        r.reconcile(&hide());
        assert_eq!(fake.window(1).unwrap().rect.x, -2000);
        fake.hang(1, false);
        r.reconcile(&hide());
        assert_ne!(fake.window(1).unwrap().rect.x, -2000);
        assert_eq!(fake.calls().len(), 3);
    }

    #[test]
    fn with_no_visible_screen_nothing_moves() {
        let (fake, r) = setup(vec![win(1, Rect::new(-2000, 100, 800, 600))]);
        r.reconcile(&BTreeSet::from([KEY_ODYSSEY, KEY_PC]));
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn without_a_primary_the_first_visible_screen_takes_them() {
        let mut s = screens();
        s[1].primary = false;
        s.push(RescueScreen {
            rect: Rect::new(3440, 0, 1920, 1080),
            work: Rect::new(3440, 0, 1920, 1040),
            key: None,
            primary: false,
        });
        let fake = Arc::new(FakeWindowMover::new(
            s,
            vec![win(1, Rect::new(-2000, 0, 500, 500))],
        ));
        WindowRescue::new(fake.clone()).reconcile(&hide());
        assert!(fake.window(1).unwrap().rect.x >= 0);
        assert!(fake.window(1).unwrap().rect.right() <= 3440.0);
    }

    #[test]
    fn a_screen_without_identity_is_never_hidden() {
        let mut s = screens();
        s[0].key = None;
        let fake = Arc::new(FakeWindowMover::new(
            s,
            vec![win(1, Rect::new(-2000, 0, 500, 500))],
        ));
        WindowRescue::new(fake.clone()).reconcile(&hide());
        assert!(fake.calls().is_empty());
    }

    proptest! {
        #[test]
        fn fit_always_lands_inside_the_work_area(
            x in -10_000i32..10_000, y in -10_000i32..10_000,
            w in 1u32..8000, h in 1u32..8000,
            wx in -5000i32..5000, wy in -5000i32..5000,
            ww in 1u32..6000, wh in 1u32..6000,
        ) {
            let work = Rect::new(wx, wy, ww, wh);
            let r = fit(Rect::new(x, y, w, h), Rect::new(-2560, 0, 2560, 1440), work);
            prop_assert!(r.x >= work.x && r.y >= work.y);
            prop_assert!(r.right() <= work.right() && r.bottom() <= work.bottom());
        }

        #[test]
        fn a_rescued_window_ends_inside_a_visible_work_area(
            x in -2560i32..-1, y in 0i32..1400,
            w in 1u32..6000, h in 1u32..3000,
            maximized in any::<bool>(),
        ) {
            // Entirely on the hidden screen: a window mostly there.
            let w = w.min(x.unsigned_abs());
            let mut win = win(1, Rect::new(x, y, w, h));
            if maximized { win.state = ShowState::Maximized; }
            let (fake, r) = setup(vec![win]);
            r.reconcile(&hide());
            let got = fake.window(1).unwrap();
            let visible = screens()[1].work;
            prop_assert!(got.normal.x >= visible.x && got.normal.right() <= visible.right());
            prop_assert!(got.normal.y >= visible.y && got.normal.bottom() <= visible.bottom());
            prop_assert!(got.rect.x >= visible.x && got.rect.right() <= visible.right());
        }
    }
}
