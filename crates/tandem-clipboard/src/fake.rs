//! Simulated clipboard: the test "copies" from outside and sees what the driver
//! wrote.

use std::convert::Infallible;
use std::sync::Arc;

use tokio::sync::watch;

use crate::clipboard::Clipboard;
use crate::content::ClipContent;

/// State of the simulated clipboard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeState {
    pub change_count: u64,
    pub content: Option<ClipContent>,
    /// What the driver wrote, in order.
    pub writes: Vec<ClipContent>,
    /// How many times the driver read the content.
    pub reads: u64,
}

/// Cloned to have another view of the same clipboard.
#[derive(Clone)]
pub struct FakeClipboard {
    state: Arc<watch::Sender<FakeState>>,
    /// What the "system" does with what is written (convert formats, for
    /// example): reading it back is not equal.
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

    /// The user copies something on this machine.
    pub fn copy(&self, content: Option<ClipContent>) {
        self.state.send_modify(|s| {
            s.change_count += 1;
            s.content = content;
        });
    }

    /// Something touches the clipboard without changing the content (another app that
    /// writes it again the same).
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
