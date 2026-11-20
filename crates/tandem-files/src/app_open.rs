//! What the agent's AppKit host does when someone opens Tandem from the system (T-350).
//!
//! `tandemd` lives in `Tandem.app/Contents/MacOS` and serves `NSApplication.run` (T-121), so
//! LaunchServices can take it for the running instance of the app: opening Tandem from Finder,
//! Launchpad, the Dock or `open -a` then only sends the reopen Apple event (`aevt/rapp`) to the
//! agent, which has no window, instead of launching `tandem-ui`. The agent passes the request on:
//! it launches the UI beside it if no UI is connected to its IPC, or asks the connected one to
//! show its main window.
//!
//! The decision is portable and tested here; `drag_macos` only reads the Apple event and the
//! agent does the launching or the showing.

use crate::app_quit::{CORE_EVENT_CLASS, QUIT_APPLICATION, four_cc};

/// `kAEOpenApplication`.
pub const OPEN_APPLICATION: u32 = four_cc(b"oapp");
/// `kAEReopenApplication`: the app is already running and is opened again.
pub const REOPEN_APPLICATION: u32 = four_cc(b"rapp");

/// The Apple event AppKit is handling when it asks the delegate to reopen
/// (`applicationShouldHandleReopen:hasVisibleWindows:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenRequest {
    /// The callback came with no Apple event being handled.
    NoAppleEvent,
    AppleEvent {
        class: u32,
        id: u32,
    },
}

/// What the agent does with an [`OpenRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenDecision {
    /// Not a request to open the app: nothing changes.
    Nothing,
    /// No UI is connected: launch the bundle's `tandem-ui`.
    LaunchUi,
    /// A UI is connected: it brings its main window to the front.
    ShowUi,
}

/// An open or reopen request brings the UI up; anything else, a Quit above all, does nothing.
/// No answer terminates the agent: that is only [`crate::app_quit::decide`]'s, and only at the
/// end of the session.
pub fn decide(request: OpenRequest, ui_connected: bool) -> OpenDecision {
    let opens = match request {
        // AppKit's reopen callback without an event: still the user opening the app.
        OpenRequest::NoAppleEvent => true,
        OpenRequest::AppleEvent { class, id } => {
            class == CORE_EVENT_CLASS && (id == REOPEN_APPLICATION || id == OPEN_APPLICATION)
        }
    };
    match (opens, ui_connected) {
        (false, _) => OpenDecision::Nothing,
        (true, false) => OpenDecision::LaunchUi,
        (true, true) => OpenDecision::ShowUi,
    }
}

impl OpenRequest {
    /// The same event as a [`crate::app_quit::QuitRequest`], with no quit reason: what the
    /// invariant tests compare against.
    pub fn as_quit_request(self) -> crate::app_quit::QuitRequest {
        match self {
            OpenRequest::NoAppleEvent => crate::app_quit::QuitRequest::NoAppleEvent,
            OpenRequest::AppleEvent { class, id } => crate::app_quit::QuitRequest::AppleEvent {
                class,
                id,
                reason: None,
            },
        }
    }

    /// Is it the Quit Apple event?
    pub fn is_quit(self) -> bool {
        matches!(
            self,
            OpenRequest::AppleEvent {
                class: CORE_EVENT_CLASS,
                id: QUIT_APPLICATION
            }
        )
    }
}

impl std::fmt::Display for OpenRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_quit_request().fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::app_quit::{self, QuitDecision};

    fn event(id: &[u8; 4]) -> OpenRequest {
        OpenRequest::AppleEvent {
            class: CORE_EVENT_CLASS,
            id: four_cc(id),
        }
    }

    #[test]
    fn four_cc_matches_the_carbon_constants() {
        // kAEOpenApplication and kAEReopenApplication from AERegistry.h.
        assert_eq!(OPEN_APPLICATION, 0x6f61_7070);
        assert_eq!(REOPEN_APPLICATION, 0x7261_7070);
    }

    #[test]
    fn reopening_without_a_ui_launches_it() {
        assert_eq!(decide(event(b"rapp"), false), OpenDecision::LaunchUi);
        assert_eq!(decide(event(b"oapp"), false), OpenDecision::LaunchUi);
        assert_eq!(
            decide(OpenRequest::NoAppleEvent, false),
            OpenDecision::LaunchUi
        );
    }

    #[test]
    fn reopening_with_a_ui_shows_it() {
        assert_eq!(decide(event(b"rapp"), true), OpenDecision::ShowUi);
        assert_eq!(
            decide(OpenRequest::NoAppleEvent, true),
            OpenDecision::ShowUi
        );
    }

    #[test]
    fn quit_and_other_events_do_nothing() {
        for ui in [false, true] {
            assert_eq!(decide(event(b"quit"), ui), OpenDecision::Nothing);
            assert_eq!(decide(event(b"odoc"), ui), OpenDecision::Nothing);
            let other_class = OpenRequest::AppleEvent {
                class: four_cc(b"misc"),
                id: REOPEN_APPLICATION,
            };
            assert_eq!(decide(other_class, ui), OpenDecision::Nothing);
        }
    }

    #[test]
    fn the_request_is_readable_in_the_log() {
        assert_eq!(
            event(b"rapp").to_string(),
            "Apple event 'aevt'/'rapp', no reason"
        );
    }

    fn any_request() -> impl Strategy<Value = OpenRequest> {
        let known = prop_oneof![
            Just(OPEN_APPLICATION),
            Just(REOPEN_APPLICATION),
            Just(QUIT_APPLICATION),
            any::<u32>(),
        ];
        let class = prop_oneof![Just(CORE_EVENT_CLASS), any::<u32>()];
        prop_oneof![
            Just(OpenRequest::NoAppleEvent),
            (class, known).prop_map(|(class, id)| OpenRequest::AppleEvent { class, id }),
        ]
    }

    proptest! {
        #[test]
        fn a_quit_is_never_turned_into_a_launch(ui: bool, request in any_request()) {
            if request.is_quit() {
                prop_assert_eq!(decide(request, ui), OpenDecision::Nothing);
            }
        }

        #[test]
        fn an_open_request_never_terminates_the_agent(ui: bool, request in any_request()) {
            // Whatever reaches the reopen callback, the Quit answer for the same event keeps the agent.
            if decide(request, ui) != OpenDecision::Nothing {
                prop_assert_eq!(app_quit::decide(request.as_quit_request()), QuitDecision::Ignore);
            }
        }

        #[test]
        fn the_ui_state_only_picks_launch_or_show(request in any_request()) {
            let (without, with) = (decide(request, false), decide(request, true));
            prop_assert_eq!(without == OpenDecision::Nothing, with == OpenDecision::Nothing);
            if without != OpenDecision::Nothing {
                prop_assert_eq!(without, OpenDecision::LaunchUi);
                prop_assert_eq!(with, OpenDecision::ShowUi);
            }
        }
    }
}
