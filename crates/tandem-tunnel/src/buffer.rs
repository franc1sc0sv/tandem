//! Búfer circular de salida con posición absoluta por byte.
//!
//! Cada byte de un flujo tiene su número (0, 1, 2…) desde que el flujo nació. El búfer
//! guarda los últimos `capacity`: quien lee pide «desde el byte N» y recibe exactamente lo
//! que sigue, o el aviso de que una parte ya se descartó. Lo usan los dos lados: la destino
//! para lo que producen los procesos y el origen para lo que ya recibió (su espejo).

use std::collections::VecDeque;

/// Lo que guarda cada flujo por omisión (DESIGN §8: 8 MiB por proceso).
pub const DEFAULT_CAPACITY: usize = 8 * 1024 * 1024;

/// Lo que hay a partir de una posición.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// `bytes` empieza en `offset`.
    Data { offset: u64, bytes: Vec<u8> },
    /// `from..to` ya se descartó; lo siguiente que hay empieza en `to`.
    Lost { from: u64, to: u64 },
    /// Todavía no hay nada desde ahí.
    Pending,
}

#[derive(Debug, Clone)]
pub struct OutputBuffer {
    capacity: usize,
    /// Posición del primer byte guardado.
    start: u64,
    data: VecDeque<u8>,
    /// Bytes descartados (por falta de lugar o por un hueco que avisó el otro lado).
    dropped: u64,
}

impl OutputBuffer {
    /// `capacity` 0 se toma como 1: un búfer que no guarda nada no sirve para reanudar.
    pub fn new(capacity: usize) -> Self {
        OutputBuffer {
            capacity: capacity.max(1),
            start: 0,
            data: VecDeque::new(),
            dropped: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Posición del primer byte que todavía se puede leer.
    pub fn start(&self) -> u64 {
        self.start
    }

    /// Posición del próximo byte (el total escrito, contando lo descartado).
    pub fn end(&self) -> u64 {
        self.start + self.data.len() as u64
    }

    /// Cuántos bytes se descartaron sin que nadie los leyera aquí.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Agrega al final; si no entra, descarta lo más viejo.
    pub fn push(&mut self, bytes: &[u8]) {
        // Lo que no entra ni con el búfer vacío se descarta sin copiarlo.
        let skip = bytes.len().saturating_sub(self.capacity);
        if skip > 0 {
            self.discard_front(self.data.len());
            self.start += skip as u64;
            self.dropped += skip as u64;
        }
        let bytes = &bytes[skip..];
        let over = (self.data.len() + bytes.len()).saturating_sub(self.capacity);
        self.discard_front(over);
        self.data.extend(bytes);
    }

    fn discard_front(&mut self, n: usize) {
        self.data.drain(..n);
        self.start += n as u64;
        self.dropped += n as u64;
    }

    /// Hasta `max` bytes desde `from`.
    pub fn read(&self, from: u64, max: usize) -> Read {
        if from < self.start {
            return Read::Lost {
                from,
                to: self.start,
            };
        }
        if from >= self.end() || max == 0 {
            return Read::Pending;
        }
        // `from - start` < `data.len()`, que es un `usize`.
        let skip = usize::try_from(from - self.start).unwrap_or(usize::MAX);
        let n = max.min(self.data.len() - skip);
        Read::Data {
            offset: from,
            bytes: self.data.range(skip..skip + n).copied().collect(),
        }
    }

    /// Lo que llega del otro lado en `offset` (el espejo del origen): se queda solo con lo
    /// que todavía no tenía, así que un reenvío no duplica nada. Devuelve cuántos bytes
    /// nuevos agregó. Si hay un hueco antes de `offset`, falla sin tocar nada: quien llama
    /// tiene que pedir desde [`Self::end`] o aceptar el hueco con [`Self::skip_to`].
    pub fn accept(&mut self, offset: u64, bytes: &[u8]) -> Result<usize, Gap> {
        let end = self.end();
        if offset > end {
            return Err(Gap { at: end, offset });
        }
        let have = usize::try_from(end - offset).unwrap_or(usize::MAX);
        let new = bytes.get(have..).unwrap_or_default();
        self.push(new);
        Ok(new.len())
    }

    /// Salta hasta `to` dando por perdido lo que falta: lo guardado se suelta (el búfer es
    /// contiguo) y queda contado en [`Self::dropped`].
    pub fn skip_to(&mut self, to: u64) {
        let end = self.end();
        if to <= end {
            return;
        }
        // Lo guardado se suelta pero no se cuenta: ya se recibió, no se perdió.
        self.dropped += to - end;
        self.data.clear();
        self.start = to;
    }
}

/// Llegaron bytes que empiezan después del final de lo que se tiene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("hueco en el flujo: se tenía hasta el byte {at} y llegó el {offset}")]
pub struct Gap {
    pub at: u64,
    pub offset: u64,
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn guarda_los_ultimos_y_avisa_lo_perdido() {
        let mut b = OutputBuffer::new(4);
        b.push(b"abcdef");
        assert_eq!((b.start(), b.end(), b.dropped()), (2, 6, 2));
        assert_eq!(b.read(0, 10), Read::Lost { from: 0, to: 2 });
        assert_eq!(
            b.read(3, 10),
            Read::Data {
                offset: 3,
                bytes: b"def".to_vec()
            }
        );
        assert_eq!(b.read(6, 10), Read::Pending);
    }

    #[test]
    fn aceptar_no_duplica_lo_que_ya_estaba() {
        let mut b = OutputBuffer::new(64);
        assert_eq!(b.accept(0, b"hola"), Ok(4));
        assert_eq!(b.accept(2, b"la mundo"), Ok(6));
        assert_eq!(b.accept(0, b"ho"), Ok(0));
        assert_eq!(b.accept(20, b"x"), Err(Gap { at: 10, offset: 20 }));
        assert_eq!(
            b.read(0, 64),
            Read::Data {
                offset: 0,
                bytes: b"hola mundo".to_vec()
            }
        );
    }

    #[test]
    fn saltar_un_hueco_cuenta_solo_lo_que_falta() {
        let mut b = OutputBuffer::new(64);
        b.push(b"abc");
        b.skip_to(10);
        assert_eq!((b.start(), b.end(), b.dropped()), (10, 10, 7));
        assert_eq!(b.read(3, 1), Read::Lost { from: 3, to: 10 });
        assert_eq!(b.accept(10, b"z"), Ok(1));
    }

    proptest! {
        /// Lo que se puede leer siempre es exactamente la cola de lo escrito, nunca más de
        /// la capacidad, y lo descartado más lo guardado es el total.
        #[test]
        fn lo_guardado_es_la_cola_de_lo_escrito(
            cap in 1usize..64,
            writes in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..40), 0..20),
        ) {
            let mut b = OutputBuffer::new(cap);
            let mut all = Vec::new();
            for w in &writes {
                b.push(w);
                all.extend_from_slice(w);
            }
            prop_assert_eq!(b.end(), all.len() as u64);
            prop_assert!(b.end() - b.start() <= cap as u64);
            prop_assert_eq!(b.dropped(), b.start());
            let tail = &all[usize::try_from(b.start()).unwrap()..];
            match b.read(b.start(), usize::MAX) {
                Read::Data { bytes, .. } => prop_assert_eq!(bytes.as_slice(), tail),
                Read::Pending => prop_assert!(tail.is_empty()),
                Read::Lost { .. } => prop_assert!(false, "desde start nunca hay pérdida"),
            }
        }

        /// Recibir pedazos que se solapan (reenvíos tras un corte) da el original.
        #[test]
        fn los_reenvios_solapados_dan_el_original(
            data in prop::collection::vec(any::<u8>(), 0..300),
            cuts in prop::collection::vec((0usize..300, 0usize..300), 0..30),
        ) {
            let mut b = OutputBuffer::new(1024);
            for (a, len) in cuts {
                let from = a.min(usize::try_from(b.end()).unwrap()).min(data.len());
                let to = (from + len).min(data.len());
                b.accept(from as u64, &data[from..to]).unwrap();
            }
            let at = usize::try_from(b.end()).unwrap();
            b.accept(at as u64, &data[at..]).unwrap();
            prop_assert_eq!(b.read(0, usize::MAX), if data.is_empty() {
                Read::Pending
            } else {
                Read::Data { offset: 0, bytes: data.clone() }
            });
        }
    }
}
