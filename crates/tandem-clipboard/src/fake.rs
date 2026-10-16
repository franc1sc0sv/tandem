//! Portapapeles simulado: la prueba "copia" desde fuera y ve lo que el driver
//! escribió.

use std::convert::Infallible;
use std::sync::Arc;

use tokio::sync::watch;

use crate::clipboard::Clipboard;
use crate::content::ClipContent;

/// Estado del portapapeles simulado.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeState {
    pub change_count: u64,
    pub content: Option<ClipContent>,
    /// Lo que escribió el driver, en orden.
    pub writes: Vec<ClipContent>,
    /// Cuántas veces leyó el driver el contenido.
    pub reads: u64,
}

/// Se clona para tener otra vista del mismo portapapeles.
#[derive(Clone)]
pub struct FakeClipboard {
    state: Arc<watch::Sender<FakeState>>,
    /// Lo que el "sistema" hace con lo que se escribe (convertir formatos, por
    /// ejemplo): al leerlo de vuelta no es igual.
    rewrite: Option<fn(ClipContent) -> ClipContent>,
}

impl Default for FakeClipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeClipboard {
    pub fn new() -> Self {
        Self {
            state: Arc::new(watch::Sender::new(FakeState::default())),
            rewrite: None,
        }
    }

    pub fn with_rewrite(mut self, rewrite: fn(ClipContent) -> ClipContent) -> Self {
        self.rewrite = Some(rewrite);
        self
    }

    /// El usuario copia algo en esta máquina.
    pub fn copy(&self, content: Option<ClipContent>) {
        self.state.send_modify(|s| {
            s.change_count += 1;
            s.content = content;
        });
    }

    /// Algo toca el portapapeles sin cambiar el contenido (otra app que lo
    /// vuelve a escribir igual).
    pub fn touch(&self) {
        self.state.send_modify(|s| s.change_count += 1);
    }

    pub fn state(&self) -> FakeState {
        self.state.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<FakeState> {
        self.state.subscribe()
    }
}

impl Clipboard for FakeClipboard {
    type Error = Infallible;

    fn change_count(&mut self) -> Result<u64, Infallible> {
        Ok(self.state.borrow().change_count)
    }

    fn read(&mut self) -> Result<Option<ClipContent>, Infallible> {
        let mut content = None;
        self.state.send_modify(|s| {
            s.reads += 1;
            content = s.content.clone();
        });
        Ok(content)
    }

    fn write(&mut self, content: &ClipContent) -> Result<(), Infallible> {
        let stored = match self.rewrite {
            Some(f) => f(content.clone()),
            None => content.clone(),
        };
        self.state.send_modify(|s| {
            s.change_count += 1;
            s.content = Some(stored);
            s.writes.push(content.clone());
        });
        Ok(())
    }
}
