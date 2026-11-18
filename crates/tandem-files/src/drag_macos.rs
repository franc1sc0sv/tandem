//! Drags on macOS (T-121).
//!
//! - [`MacDragDetector`] (origin): with the left button pressed, reads from
//!   `NSPasteboard(name: .drag)` the file URLs being dragged.
//! - [`MacDragStarter`] (destination): opens an invisible window under the cursor and, from it,
//!   an `NSDraggingSession` with the already copied file URLs. AppKit is only touched from
//!   the main thread, which belongs to whoever hosts the agent (the Tauri UI): so it is
//!   given a function that runs a closure there ([`RunOnMain`]).

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSDragOperation,
    NSDraggingContext, NSDraggingItem, NSDraggingSession, NSDraggingSource, NSEvent,
    NSEventModifierFlags, NSEventType, NSPasteboard, NSPasteboardNameDrag, NSPasteboardTypeFileURL,
    NSPasteboardWriting, NSWindow, NSWindowStyleMask,
};
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGMouseButton};
use objc2_foundation::{
    NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use tokio::sync::oneshot;

use crate::drag::{DragDetector, DragEnd, DragStarter};
use crate::drag_os::{end_of, parse_file_url};
use crate::error::{FilesError, Result};

/// Runs a closure on the app's main thread.
pub type RunOnMain = Arc<dyn Fn(Box<dyn FnOnce(MainThreadMarker) + Send>) + Send + Sync>;

/// [`RunOnMain`] over GCD's main queue: the closure runs on the process's main
/// thread, which has to be serving its run loop ([`run_app_loop`]). The marker is
/// obtained by checking the thread, not assuming it: if for some reason it were not the main one
/// the closure is discarded and `start` fails with "the drag was not served" instead of touching AppKit
/// from another thread.
pub fn main_queue() -> RunOnMain {
    Arc::new(|work| {
        DispatchQueue::main().exec_async(move || {
            if let Some(mtm) = MainThreadMarker::new() {
                work(mtm);
            }
        });
    })
}

/// Serves the AppKit run loop on the main thread until the process ends. Without an
/// icon in the Dock (`Accessory`): the agent is not an app with windows, it only needs the
/// events for the drag session. Call only from the main thread; on another thread it
/// does nothing.
pub fn run_app_loop() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.run();
}

/// Is the left mouse button pressed? Without that, whatever remains in the drag
/// pasteboard is from a drag that already ended.
fn left_button_down() -> bool {
    CGEventSource::button_state(
        CGEventSourceStateID::CombinedSessionState,
        CGMouseButton::Left,
    )
}

/// macOS [`DragDetector`].
#[derive(Debug, Default, Clone, Copy)]
pub struct MacDragDetector;

impl DragDetector for MacDragDetector {
    fn file_drag(&self) -> Option<Vec<PathBuf>> {
        if !left_button_down() {
            return None;
        }
        // SAFETY: `NSPasteboardNameDrag` is a system constant; reading a named
        // pasteboard can be done from any thread.
        let board = unsafe { NSPasteboard::pasteboardWithName(NSPasteboardNameDrag) };
        let items = board.pasteboardItems()?;
        let files: Vec<PathBuf> = items
            .iter()
            // SAFETY: `NSPasteboardTypeFileURL` is a system constant.
            .filter_map(|item| item.stringForType(unsafe { NSPasteboardTypeFileURL }))
            .filter_map(|url| parse_file_url(&url.to_string()))
            .collect();
        (!files.is_empty()).then_some(files)
    }
}

struct Ivars {
    end: RefCell<Option<oneshot::Sender<NSDragOperation>>>,
    window: RefCell<Option<Retained<NSWindow>>>,
}

define_class!(
    // SAFETY: NSObject asks nothing special of its subclasses; it lives and is used only on the
    // main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TandemDragSource"]
    #[ivars = Ivars]
    struct DragSource;

    // SAFETY: `NSObjectProtocol` requires no methods of its own and `DragSource` inherits from `NSObject`.
    unsafe impl NSObjectProtocol for DragSource {}

    // SAFETY: `DragSource` only implements methods with the `NSDraggingSource` signature and lives
    // on the main thread (`MainThreadOnly`), as the protocol requires.
    unsafe impl NSDraggingSource for DragSource {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn source_mask(&self, _s: &NSDraggingSession, _c: NSDraggingContext) -> NSDragOperation {
            // The system copies on drop: the origin remains the file in the temporary folder.
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn ended(&self, _s: &NSDraggingSession, _at: NSPoint, op: NSDragOperation) {
            if let Some(tx) = self.ivars().end.borrow_mut().take() {
                let _ = tx.send(op);
            }
            if let Some(window) = self.ivars().window.borrow_mut().take() {
                window.close();
            }
            ACTIVE.with(|a| a.borrow_mut().take());
        }
    }
);

thread_local! {
    /// The live origin of the drag in progress: the session does not guarantee retaining it.
    static ACTIVE: RefCell<Option<Retained<DragSource>>> = const { RefCell::new(None) };
}

impl DragSource {
    fn new(
        mtm: MainThreadMarker,
        end: oneshot::Sender<NSDragOperation>,
        window: Retained<NSWindow>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            end: RefCell::new(Some(end)),
            window: RefCell::new(Some(window)),
        });
        // SAFETY: `init` of NSObject with no arguments.
        unsafe { msg_send![super(this), init] }
    }
}

/// Opens the invisible window under the cursor and starts the session with `files`.
fn begin(
    mtm: MainThreadMarker,
    files: &[PathBuf],
    end: oneshot::Sender<NSDragOperation>,
) -> std::result::Result<(), String> {
    let at = NSEvent::mouseLocation();
    let frame = NSRect::new(
        NSPoint::new(at.x - 32.0, at.y - 32.0),
        NSSize::new(64.0, 64.0),
    );
    // SAFETY: new window on the main thread; no buffer of its own until shown.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // The window is already released by `close`: it must not release itself on closing.
    // SAFETY: window property, all on the main thread.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setIgnoresMouseEvents(false);
    window.orderFrontRegardless();
    let view = window.contentView().ok_or("the window has no view")?;

    let mut items = Vec::with_capacity(files.len());
    for (i, file) in files.iter().enumerate() {
        let path = NSString::from_str(&file.to_string_lossy());
        let url = NSURL::fileURLWithPath(&path);
        let writer = ProtocolObject::<dyn NSPasteboardWriting>::from_retained(url);
        let item = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), &writer);
        // Each file in its own position, so they look like several.
        let offset = 6.0 * i as f64;
        let rect = NSRect::new(NSPoint::new(offset, -offset), NSSize::new(48.0, 48.0));
        // SAFETY: with no image of its own, the system uses the file's icon.
        unsafe { item.setDraggingFrame_contents(rect, None) };
        items.push(item);
    }

    // A synthetic click in the window: the session needs the event that starts it.
    let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        NSEventType::LeftMouseDown,
        NSPoint::new(32.0, 32.0),
        NSEventModifierFlags::empty(),
        0.0,
        window.windowNumber(),
        None,
        0,
        1,
        1.0,
    )
    .ok_or("could not create the event")?;

    let source = DragSource::new(mtm, end, window);
    ACTIVE.with(|a| *a.borrow_mut() = Some(source.clone()));
    let source = ProtocolObject::<dyn NSDraggingSource>::from_ref(&*source);
    view.beginDraggingSessionWithItems_event_source(
        &NSArray::from_retained_slice(&items),
        &event,
        source,
    );
    Ok(())
}

/// macOS [`DragStarter`]: the session runs on the main thread given by `run_on_main`.
pub struct MacDragStarter {
    run_on_main: RunOnMain,
}

impl MacDragStarter {
    pub fn new(run_on_main: RunOnMain) -> Self {
        MacDragStarter { run_on_main }
    }
}

impl DragStarter for MacDragStarter {
    async fn start(&self, staged: Vec<PathBuf>) -> Result<DragEnd> {
        let (end_tx, end_rx) = oneshot::channel();
        let (started_tx, started_rx) = oneshot::channel::<std::result::Result<(), String>>();
        (self.run_on_main)(Box::new(move |mtm| {
            let _ = started_tx.send(begin(mtm, &staged, end_tx));
        }));
        let io = |why: String| FilesError::io(std::path::Path::new(""), std::io::Error::other(why));
        started_rx
            .await
            .map_err(|_| io("the main thread did not serve the drag".into()))?
            .map_err(io)?;
        let op = end_rx
            .await
            .map_err(|_| io("the drag ended without notice".into()))?;
        Ok(end_of(true, op != NSDragOperation::None))
    }
}

// The `start` future travels between runtime threads: checked at compile time.
const _: () = {
    fn is_send<T: Send>(_: &T) {}
    #[allow(dead_code)]
    fn assert_send(starter: &MacDragStarter) {
        is_send(&starter.start(Vec::new()));
        is_send(starter);
    }
};
