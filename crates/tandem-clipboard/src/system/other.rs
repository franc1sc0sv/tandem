//! Linux y demás: no son destinos de Tandem, pero el crate compila y el
//! contador funciona (más caro: lee y compara el contenido cada vez).

use super::{ClipboardError, read_arboard};
use crate::content::ContentHash;

pub(super) struct Counter {
    count: u64,
    last: Option<ContentHash>,
}

impl Counter {
    pub(super) fn new() -> Self {
        Self {
            count: 0,
            last: None,
        }
    }

    pub(super) fn get(&mut self, cb: &mut arboard::Clipboard) -> Result<u64, ClipboardError> {
        let hash = read_arboard(cb)?.map(|c| c.hash());
        if hash != self.last {
            self.last = hash;
            self.count += 1;
        }
        Ok(self.count)
    }
}
