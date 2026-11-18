//! Normalized paths between Windows and Unix.
//!
//! On the wire a path is relative to the root of the copy, with `/` as separator and no
//! `.` or `..`. Whoever receives it joins it to its destination folder with its system's separator
//! and rejects what its system cannot store (on Windows, `:`, `\`, `?`, reserved
//! names like `CON`, trailing dots or spaces).

use std::path::{Component, Path, PathBuf};

use crate::error::{FilesError, Result};

const WINDOWS_FORBIDDEN: &[char] = &['<', '>', ':', '"', '|', '?', '*', '\\'];
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A file or folder name, without separators. `windows` says which rules to apply.
pub fn check_segment(seg: &str, windows: bool) -> std::result::Result<(), &'static str> {
    if seg.is_empty() {
        return Err("empty component");
    }
    if seg == "." || seg == ".." {
        return Err("`.` and `..` are not accepted");
    }
    if seg.contains('/') || seg.contains('\0') {
        return Err("the name carries a separator or a null byte");
    }
    if windows {
        if seg
            .chars()
            .any(|c| c.is_control() || WINDOWS_FORBIDDEN.contains(&c))
        {
            return Err("the name carries characters Windows does not accept");
        }
        if seg.ends_with('.') || seg.ends_with(' ') {
            return Err("Windows does not accept names ending in a dot or space");
        }
        let stem = seg.split('.').next().unwrap_or(seg).to_ascii_uppercase();
        if WINDOWS_RESERVED.contains(&stem.as_str()) {
            return Err("Windows reserved name");
        }
    }
    Ok(())
}

/// From a native relative path to the wire one (`a/b/c`).
pub fn to_wire(rel: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => {
                let s = s
                    .to_str()
                    .ok_or_else(|| FilesError::path(&rel.to_string_lossy(), "not UTF-8"))?;
                // A Unix name with `/` is impossible, but one with `\` or `:` is valid there
                // and will be rejected by whoever receives it on Windows.
                check_segment(s, false).map_err(|w| FilesError::path(s, w))?;
                parts.push(s);
            }
            Component::CurDir => {}
            _ => return Err(FilesError::path(&rel.to_string_lossy(), "not relative")),
        }
    }
    Ok(parts.join("/"))
}

/// From the wire path to a native one under `root`, with this system's rules.
pub fn from_wire(root: &Path, wire: &str) -> Result<PathBuf> {
    from_wire_for(root, wire, cfg!(windows))
}

pub fn from_wire_for(root: &Path, wire: &str, windows: bool) -> Result<PathBuf> {
    let mut out = root.to_path_buf();
    for seg in wire.split('/') {
        check_segment(seg, windows).map_err(|w| FilesError::path(wire, w))?;
        out.push(seg);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn the_wire_path_uses_slashes_and_joins_the_root() {
        let p = from_wire_for(Path::new("/dst"), "a b/\u{fc}ber/x.txt", false).unwrap();
        assert_eq!(p, Path::new("/dst/a b/\u{fc}ber/x.txt"));
        assert_eq!(
            to_wire(Path::new("a b/\u{fc}ber/x.txt")).unwrap(),
            "a b/\u{fc}ber/x.txt"
        );
    }

    #[test]
    fn rejects_what_escapes_the_root() {
        for bad in [
            "",
            "..",
            "a/../b",
            "/etc/passwd",
            "a//b",
            "./a",
            "a/",
            "a\0b",
        ] {
            assert!(
                from_wire_for(Path::new("/dst"), bad, false).is_err(),
                "{bad:?}"
            );
        }
        assert!(to_wire(Path::new("/abs")).is_err());
        assert!(to_wire(Path::new("../x")).is_err());
    }

    #[test]
    fn windows_rejects_what_it_cannot_store() {
        for bad in ["a:b", "a\\b", "CON", "con.txt", "x?", "fin.", "fin ", "a|b"] {
            assert!(
                from_wire_for(Path::new("C:\\d"), bad, true).is_err(),
                "{bad:?}"
            );
        }
        // On Unix the same names are valid, except those of the wire.
        for ok in ["a:b", "a\\b", "CON", "fin."] {
            assert!(from_wire_for(Path::new("/d"), ok, false).is_ok(), "{ok:?}");
        }
    }

    proptest! {
        /// Any path that comes out of `to_wire` comes back the same through `from_wire` and does not escape
        /// the root.
        #[test]
        fn round_trip_without_escaping(segs in proptest::collection::vec("[a-zA-Z0-9 \u{f1}\u{e1}\u{e9}\u{ed}_.-]{1,12}", 1..5)) {
            let segs: Vec<String> = segs.into_iter().filter(|s| s != "." && s != "..").collect();
            prop_assume!(!segs.is_empty());
            let rel: PathBuf = segs.iter().collect();
            let wire = to_wire(&rel).unwrap();
            let back = from_wire_for(Path::new("/root"), &wire, false).unwrap();
            prop_assert!(back.starts_with("/root"));
            prop_assert_eq!(back.strip_prefix("/root").unwrap(), rel.as_path());
        }
    }
}
