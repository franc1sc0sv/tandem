//! The live cursor (T-128): the position the UI paints as a dot over each
//! desktop and the edge crossings that flash.

use serde::{Deserialize, Serialize};
use tandem_core::Machine;

/// Maximum frequency of cursor samples, per sender.
pub const MAX_HZ: u32 = 30;

/// A cursor position, in the native coordinates of its machine (the
/// [`ScreenRect`](crate::ScreenRect)).
///
/// Integers: the UI paints a dot, and this way the message can be compared. Samples come
/// from the machine that has focus: it is the one that knows where the cursor is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorSample {
    /// The machine where the cursor is.
    pub machine: Machine,
    /// Id of the screen that contains it (the nearest one if it falls in a gap); empty if the
    /// screens are not known.
    pub screen: String,
    pub x: i32,
    pub y: i32,
}

#[cfg(test)]
pub(crate) mod samples {
    use super::*;

    pub fn samples() -> Vec<CursorSample> {
        vec![
            CursorSample {
                machine: Machine::A,
                screen: "mac-1".into(),
                x: 1919,
                y: 540,
            },
            CursorSample {
                machine: Machine::B,
                screen: String::new(),
                x: -3440,
                y: i32::MAX,
            },
        ]
    }
}
