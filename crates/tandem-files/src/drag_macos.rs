//! Arrastres en macOS (T-121).
//!
//! - [`MacDragDetector`] (origen): con el botón izquierdo apretado, lee de
//!   `NSPasteboard(name: .drag)` las URL de archivo que se están arrastrando.
//! - [`MacDragStarter`] (destino): abre una ventana invisible bajo el cursor y, desde ella,
//!   una `NSDraggingSession` con las URL de archivo ya copiadas. AppKit solo se toca desde
//!   el hilo principal, que es de quien aloja el agente (la UI de Tauri): por eso se le
//!   pasa una función que corre un cierre ahí ([`RunOnMain`]).

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSDragOperation, NSDraggingContext, NSDraggingItem,
    NSDraggingSession, NSDraggingSource, NSEvent, NSEventModifierFlags, NSEventType, NSPasteboard,
    NSPasteboardNameDrag, NSPasteboardTypeFileURL, NSPasteboardWriting, NSWindow,
    NSWindowStyleMask,
};
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGMouseButton};
use objc2_foundation::{
    NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use tokio::sync::oneshot;

use crate::drag::{DragDetector, DragEnd, DragStarter};
use crate::drag_os::{end_of, parse_file_url};
use crate::error::{FilesError, Result};

/// Corre un cierre en el hilo principal de la app.
pub type RunOnMain = Arc<dyn Fn(Box<dyn FnOnce(MainThreadMarker) + Send>) + Send + Sync>;

/// ¿Está apretado el botón izquierdo del mouse? Sin eso, lo que quede en el portapapeles de
/// arrastre es de un arrastre que ya terminó.
fn left_button_down() -> bool {
    CGEventSource::button_state(
        CGEventSourceStateID::CombinedSessionState,
        CGMouseButton::Left,
    )
}

/// [`DragDetector`] de macOS.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacDragDetector;

impl DragDetector for MacDragDetector {
    fn file_drag(&self) -> Option<Vec<PathBuf>> {
        if !left_button_down() {
            return None;
        }
        // SAFETY: `NSPasteboardNameDrag` es una constante del sistema; la lectura de un
        // pasteboard con nombre se puede hacer desde cualquier hilo.
        let board = unsafe { NSPasteboard::pasteboardWithName(NSPasteboardNameDrag) };
        let items = board.pasteboardItems()?;
        let files: Vec<PathBuf> = items
            .iter()
            // SAFETY: `NSPasteboardTypeFileURL` es una constante del sistema.
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
    // SAFETY: NSObject no pide nada especial a sus subclases; vive y se usa solo en el
    // hilo principal.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TandemDragSource"]
    #[ivars = Ivars]
    struct DragSource;

    unsafe impl NSObjectProtocol for DragSource {}

    unsafe impl NSDraggingSource for DragSource {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn source_mask(&self, _s: &NSDraggingSession, _c: NSDraggingContext) -> NSDragOperation {
            // El sistema copia al soltar: el origen sigue siendo el archivo de la carpeta temporal.
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
    /// El origen vivo del arrastre en curso: la sesión no garantiza retenerlo.
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
        // SAFETY: `init` de NSObject sin argumentos.
        unsafe { msg_send![super(this), init] }
    }
}

/// Abre la ventana invisible bajo el cursor y empieza la sesión con `files`.
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
    // SAFETY: ventana nueva en el hilo principal; sin buffer propio hasta mostrarla.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // La ventana ya la libera `close`: ella sola no debe soltarse al cerrarse.
    // SAFETY: propiedad de la ventana, todo en el hilo principal.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setIgnoresMouseEvents(false);
    window.orderFrontRegardless();
    let view = window.contentView().ok_or("la ventana no tiene vista")?;

    let mut items = Vec::with_capacity(files.len());
    for (i, file) in files.iter().enumerate() {
        let path = NSString::from_str(&file.to_string_lossy());
        let url = NSURL::fileURLWithPath(&path);
        let writer = ProtocolObject::<dyn NSPasteboardWriting>::from_retained(url);
        let item = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), &writer);
        // Cada archivo en su propia posición, para que se vean como varios.
        let offset = 6.0 * i as f64;
        let rect = NSRect::new(NSPoint::new(offset, -offset), NSSize::new(48.0, 48.0));
        // SAFETY: sin imagen propia, el sistema pone el ícono del archivo.
        unsafe { item.setDraggingFrame_contents(rect, None) };
        items.push(item);
    }

    // Un clic sintético en la ventana: la sesión necesita el evento que la inicia.
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
    .ok_or("no se pudo crear el evento")?;

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

/// [`DragStarter`] de macOS: la sesión corre en el hilo principal que da `run_on_main`.
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
            .map_err(|_| io("el hilo principal no atendió el arrastre".into()))?
            .map_err(io)?;
        let op = end_rx
            .await
            .map_err(|_| io("el arrastre terminó sin avisar".into()))?;
        Ok(end_of(true, op != NSDragOperation::None))
    }
}
