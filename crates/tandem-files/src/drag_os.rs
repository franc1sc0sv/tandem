//! The portable part of operating system drags (T-121): pure decisions that the
//! Windows and macOS implementations share, to test them on Linux.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use crate::drag::{DragDetector, DragEnd};

/// What `IDropSource::QueryContinueDrag` does on each turn of the Windows drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropAction {
    Continue,
    Drop,
    Cancel,
}

/// Esc cancels; releasing the left button drops; otherwise the drag continues.
pub fn drop_action(escape: bool, left_down: bool) -> DropAction {
    if escape {
        DropAction::Cancel
    } else if !left_down {
        DropAction::Drop
    } else {
        DropAction::Continue
    }
}

/// How the drag ended: dropping where nobody accepts the copy (empty effect or operation)
/// is the same as canceling.
pub fn end_of(dropped: bool, accepted: bool) -> DragEnd {
    if dropped && accepted {
        DragEnd::Dropped
    } else {
        DragEnd::Cancelled
    }
}

/// The path of a `file://` from the macOS drag pasteboard, with the `%xx` resolved.
/// `None` if it is not a local file or the text is not valid UTF-8.
pub fn parse_file_url(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // `file://localhost/path` and `file:///path` are the same; another host is not local.
    let path = match rest.strip_prefix("localhost") {
        Some(p) => p,
        None => rest,
    };
    if !path.starts_with('/') {
        return None;
    }
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = path.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let path = String::from_utf8(out).ok()?;
    // Folders arrive with a trailing "/".
    let trimmed = path.trim_end_matches('/');
    Some(PathBuf::from(if trimmed.is_empty() {
        "/"
    } else {
        trimmed
    }))
}

/// What the edge's invisible drag target knows about the drag in progress. Written by
/// the window thread (Windows) and read by the agent through [`DragDetector`]; the test
/// double writes the same without a window.
#[derive(Debug, Clone, Default)]
pub struct DragProbe(Arc<Mutex<Option<Vec<PathBuf>>>>);

impl DragProbe {
    /// A drag with `paths` entered (CF_HDROP). Without paths it is not a file drag.
    pub fn enter(&self, paths: Vec<PathBuf>) {
        self.set((!paths.is_empty()).then_some(paths));
    }

    /// The drag left the edge, was dropped or was canceled.
    pub fn leave(&self) {
        self.set(None);
    }

    fn set(&self, v: Option<Vec<PathBuf>>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = v;
    }
}

impl DragDetector for DragProbe {
    fn file_drag(&self) -> Option<Vec<PathBuf>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Screen edge through which it crosses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripEdge {
    Left,
    Right,
    Top,
    Bottom,
}

/// Rectangle in screen pixels `(x, y, width, height)`.
pub type PxRect = (i32, i32, i32, i32);

/// The thin strip, flush inside the `edge` of `bounds`, where the drag target window
/// lives. Inside: outside the virtual desktop nobody receives a drop.
pub fn strip_rect(bounds: PxRect, edge: StripEdge, thickness: i32) -> PxRect {
    let (x, y, w, h) = bounds;
    let t = thickness.clamp(1, w.min(h).max(1));
    match edge {
        StripEdge::Left => (x, y, t, h),
        StripEdge::Right => (x + w - t, y, t, h),
        StripEdge::Top => (x, y, w, t),
        StripEdge::Bottom => (x, y + h - t, w, t),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn esc_cancels_and_releasing_the_button_drops() {
        assert_eq!(drop_action(true, true), DropAction::Cancel);
        assert_eq!(drop_action(true, false), DropAction::Cancel);
        assert_eq!(drop_action(false, false), DropAction::Drop);
        assert_eq!(drop_action(false, true), DropAction::Continue);
    }

    #[test]
    fn dropping_where_nobody_accepts_is_canceling() {
        assert_eq!(end_of(true, true), DragEnd::Dropped);
        assert_eq!(end_of(true, false), DragEnd::Cancelled);
        assert_eq!(end_of(false, true), DragEnd::Cancelled);
    }

    #[test]
    fn file_urls_become_paths() {
        assert_eq!(
            parse_file_url("file:///Users/ana/My%20photo%C3%BC.png"),
            Some(PathBuf::from("/Users/ana/My photo\u{fc}.png"))
        );
        assert_eq!(
            parse_file_url("file://localhost/tmp/x"),
            Some(PathBuf::from("/tmp/x"))
        );
        assert_eq!(
            parse_file_url("file:///Users/ana/folder/"),
            Some(PathBuf::from("/Users/ana/folder"))
        );
        assert_eq!(parse_file_url("file:///"), Some(PathBuf::from("/")));
    }

    #[test]
    fn what_is_not_a_local_file_is_discarded() {
        for bad in [
            "https://example.com/a",
            "file://other-host/a",
            "file:///a%zz",
            "file:///a%2",
            "file:///a%FF",
            "",
        ] {
            assert_eq!(parse_file_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_probe_reflects_the_file_drag() {
        let p = DragProbe::default();
        assert_eq!(p.file_drag(), None);
        p.enter(vec![PathBuf::from("C:/a.txt")]);
        assert_eq!(p.file_drag(), Some(vec![PathBuf::from("C:/a.txt")]));
        let other = p.clone();
        other.leave();
        assert_eq!(p.file_drag(), None, "the copies share state");
    }

    #[test]
    fn a_drag_without_files_does_not_count() {
        let p = DragProbe::default();
        p.enter(Vec::new());
        assert_eq!(p.file_drag(), None);
    }

    #[test]
    fn the_strip_stays_inside_the_edge() {
        let b = (100, 50, 1920, 1080);
        assert_eq!(strip_rect(b, StripEdge::Left, 4), (100, 50, 4, 1080));
        assert_eq!(strip_rect(b, StripEdge::Right, 4), (2016, 50, 4, 1080));
        assert_eq!(strip_rect(b, StripEdge::Top, 4), (100, 50, 1920, 4));
        assert_eq!(strip_rect(b, StripEdge::Bottom, 4), (100, 1126, 1920, 4));
    }

    proptest! {
        #[test]
        fn the_strip_always_fits_on_the_screen(
            x in -3000i32..3000, y in -3000i32..3000,
            w in 1i32..5000, h in 1i32..5000, t in -5i32..50, e in 0u8..4,
        ) {
            let edge = [StripEdge::Left, StripEdge::Right, StripEdge::Top, StripEdge::Bottom][e as usize];
            let (sx, sy, sw, sh) = strip_rect((x, y, w, h), edge, t);
            prop_assert!(sw >= 1 && sh >= 1);
            prop_assert!(sx >= x && sy >= y && sx + sw <= x + w && sy + sh <= y + h);
        }
    }

    /// What macOS encodes: everything except unreserved characters ends up as `%xx`.
    fn encode(path: &str) -> String {
        let mut out = String::from("file://");
        for b in path.bytes() {
            if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        out
    }

    proptest! {
        #[test]
        fn encode_and_read_returns_the_same_path(
            parts in proptest::collection::vec("[^/\\x00]{1,12}", 1..5)
        ) {
            let path = format!("/{}", parts.join("/"));
            let expected = path.trim_end_matches('/').to_owned();
            prop_assert_eq!(parse_file_url(&encode(&path)), Some(PathBuf::from(expected)));
        }
    }
}
