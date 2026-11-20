//! What the agent's AppKit host answers when it is asked to quit (T-333).
//!
//! `tandemd` lives in `Tandem.app/Contents/MacOS` and serves `NSApplication.run` for the
//! drags (T-121), so LaunchServices treats it as part of the app and sends it the Quit Apple
//! event too: System Settings' "Quit & Reopen" after a privacy change, or
//! `osascript -e 'tell application id "…" to quit'`. AppKit's default answer is `exit(0)`,
//! which skips the orderly shutdown (the state is not saved) and is a successful exit that the
//! LaunchAgent's `KeepAlive` does not restart. launchd supervises the agent and stops it with
//! SIGTERM, so a plain Quit is ignored. Only the end of the session is let through, so the
//! agent never blocks a logout, restart or shutdown.
//!
//! The decision is portable and tested here; `drag_macos` only reads the Apple event.

use std::fmt;

/// A classic Mac `FourCharCode` (`OSType`, `AEKeyword`…) from its four characters.
pub const fn four_cc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

/// `kCoreEventClass`.
pub const CORE_EVENT_CLASS: u32 = four_cc(b"aevt");
/// `kAEQuitApplication`.
pub const QUIT_APPLICATION: u32 = four_cc(b"quit");
/// `kAEQuitReason`: the optional parameter loginwindow adds when the session ends.
pub const QUIT_REASON: u32 = four_cc(b"why?");

/// The `kAEQuitReason` values that mean the session is ending.
const SESSION_END: [(&[u8; 4], SessionEnd); 6] = [
    (b"logo", SessionEnd::LogOut),   // kAELogOut
    (b"rlgo", SessionEnd::LogOut),   // kAEReallyLogOut
    (b"rrst", SessionEnd::Restart),  // kAEShowRestartDialog
    (b"rest", SessionEnd::Restart),  // kAERestart
    (b"rsdn", SessionEnd::ShutDown), // kAEShowShutdownDialog
    (b"shut", SessionEnd::ShutDown), // kAEShutDown
];

/// Why AppKit asks the agent whether it may terminate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitRequest {
    /// `terminate:` with no Apple event being handled.
    NoAppleEvent,
    /// The Apple event being handled, with its `kAEQuitReason` if it has one.
    AppleEvent {
        class: u32,
        id: u32,
        reason: Option<u32>,
    },
}

/// How the user's session is ending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    LogOut,
    Restart,
    ShutDown,
}

/// The answer to `applicationShouldTerminate:`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitDecision {
    /// `NSTerminateCancel`: the agent keeps running; launchd stops it.
    Ignore,
    /// `NSTerminateNow`: the session is ending, and cancelling would block it.
    Allow(SessionEnd),
}

/// Ignores every request except a Quit Apple event sent because the session is ending.
pub fn decide(request: QuitRequest) -> QuitDecision {
    let QuitRequest::AppleEvent {
        class: CORE_EVENT_CLASS,
        id: QUIT_APPLICATION,
        reason: Some(reason),
    } = request
    else {
        return QuitDecision::Ignore;
    };
    SESSION_END
        .iter()
        .find(|(code, _)| four_cc(code) == reason)
        .map_or(QuitDecision::Ignore, |&(_, end)| QuitDecision::Allow(end))
}

/// Writes a `FourCharCode` as its four characters when they are printable, else in hex.
struct FourCc(u32);

impl fmt::Display for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.0.to_be_bytes();
        if bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
            write!(f, "'{}'", String::from_utf8_lossy(&bytes))
        } else {
            write!(f, "{:#010x}", self.0)
        }
    }
}

impl fmt::Display for QuitRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            QuitRequest::NoAppleEvent => f.write_str("no Apple event"),
            QuitRequest::AppleEvent { class, id, reason } => {
                write!(f, "Apple event {}/{}", FourCc(class), FourCc(id))?;
                match reason {
                    Some(reason) => write!(f, ", reason {}", FourCc(reason)),
                    None => f.write_str(", no reason"),
                }
            }
        }
    }
}

impl fmt::Display for SessionEnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SessionEnd::LogOut => "log out",
            SessionEnd::Restart => "restart",
            SessionEnd::ShutDown => "shut down",
        })
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn quit(reason: Option<&[u8; 4]>) -> QuitRequest {
        QuitRequest::AppleEvent {
            class: CORE_EVENT_CLASS,
            id: QUIT_APPLICATION,
            reason: reason.map(four_cc),
        }
    }

    #[test]
    fn four_cc_matches_the_carbon_constants() {
        // kCoreEventClass and kAEQuitApplication from AERegistry.h.
        assert_eq!(CORE_EVENT_CLASS, 0x6165_7674);
        assert_eq!(QUIT_APPLICATION, 0x7175_6974);
        assert_eq!(QUIT_REASON, 0x7768_793f);
    }

    #[test]
    fn a_plain_quit_apple_event_is_ignored() {
        // System Settings' "Quit & Reopen" and `osascript … to quit` carry no reason.
        assert_eq!(decide(quit(None)), QuitDecision::Ignore);
    }

    #[test]
    fn terminate_without_an_apple_event_is_ignored() {
        assert_eq!(decide(QuitRequest::NoAppleEvent), QuitDecision::Ignore);
    }

    #[test]
    fn the_end_of_the_session_is_allowed() {
        for (code, end) in [
            (b"logo", SessionEnd::LogOut),
            (b"rlgo", SessionEnd::LogOut),
            (b"rrst", SessionEnd::Restart),
            (b"rest", SessionEnd::Restart),
            (b"rsdn", SessionEnd::ShutDown),
            (b"shut", SessionEnd::ShutDown),
        ] {
            assert_eq!(
                decide(quit(Some(code))),
                QuitDecision::Allow(end),
                "{code:?}"
            );
        }
    }

    #[test]
    fn a_session_reason_on_another_event_is_ignored() {
        let open = QuitRequest::AppleEvent {
            class: CORE_EVENT_CLASS,
            id: four_cc(b"oapp"),
            reason: Some(four_cc(b"shut")),
        };
        assert_eq!(decide(open), QuitDecision::Ignore);
    }

    #[test]
    fn the_request_is_readable_in_the_log() {
        assert_eq!(
            quit(None).to_string(),
            "Apple event 'aevt'/'quit', no reason"
        );
        assert_eq!(
            quit(Some(b"rlgo")).to_string(),
            "Apple event 'aevt'/'quit', reason 'rlgo'"
        );
        let odd = QuitRequest::AppleEvent {
            class: 1,
            id: QUIT_APPLICATION,
            reason: None,
        };
        assert_eq!(odd.to_string(), "Apple event 0x00000001/'quit', no reason");
        assert_eq!(QuitRequest::NoAppleEvent.to_string(), "no Apple event");
    }

    proptest! {
        #[test]
        fn only_session_reasons_let_the_agent_quit(class: u32, id: u32, reason: Option<u32>) {
            let request = QuitRequest::AppleEvent { class, id, reason };
            let session = class == CORE_EVENT_CLASS
                && id == QUIT_APPLICATION
                && reason.is_some_and(|r| SESSION_END.iter().any(|(c, _)| four_cc(c) == r));
            prop_assert_eq!(decide(request) != QuitDecision::Ignore, session);
        }
    }
}
