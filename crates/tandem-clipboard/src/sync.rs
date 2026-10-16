//! Máquina de estados de la sincronización, sin E/S: recibe lo que pasa (el
//! portapapeles local cambió, llegó un mensaje, cambió el foco) y devuelve qué
//! hacer ([`Action`]). Es la misma en los dos agentes.
//!
//! **Supresión de eco.** Se recuerda el hash de lo que tiene el portapapeles
//! local (`current`), sea porque se vio cambiar o porque se escribió lo que llegó
//! del otro lado. Un cambio local con ese mismo hash no se anuncia: es nuestra
//! propia escritura que vuelve. Y una oferta del otro lado con el hash que ya
//! tenemos no se escribe.

use std::time::{SystemTime, UNIX_EPOCH};

use tandem_proto::MAX_FRAME_LEN;

use crate::content::{ClipContent, ContentHash};
use crate::msg::ClipMsg;

/// Límites de la transferencia.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncConfig {
    /// Hasta este tamaño (codificado) el contenido va dentro del anuncio.
    pub inline_max: usize,
    /// Tamaño de cada `Chunk`. Trozos chicos dejan pasar otros mensajes entre
    /// medio.
    pub chunk_len: usize,
    /// Lo más grande que se manda o se acepta: un tope a la memoria que puede
    /// pedir el otro lado.
    pub max_len: usize,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            inline_max: 64 * 1024,
            chunk_len: 256 * 1024,
            // Una captura 5K en RGBA son ~60 MB.
            max_len: 128 * 1024 * 1024,
        }
    }
}

impl SyncConfig {
    /// Lo que va en una trama tiene que caber en ella, con margen para la
    /// cabecera del mensaje.
    fn clamped(self) -> Self {
        let frame_room = MAX_FRAME_LEN - 1024;
        Self {
            inline_max: self.inline_max.min(frame_room),
            chunk_len: self.chunk_len.clamp(1, frame_room),
            max_len: self.max_len,
        }
    }
}

/// Lo que la máquina de estados pide hacer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Mandar al otro agente.
    Send(ClipMsg),
    /// Escribir en el portapapeles local lo que llegó del otro lado.
    Write(ClipContent),
}

/// Contadores, para diagnóstico y pruebas.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// Anuncios de copias locales mandados.
    pub offers_sent: u64,
    /// Anuncios recibidos del otro lado.
    pub offers_received: u64,
    /// Contenidos del otro lado escritos en el portapapeles local.
    pub written: u64,
    /// Ofertas o transferencias descartadas por inválidas (hash, tamaño).
    pub rejected: u64,
}

/// La copia local que el otro lado puede pedir, ya codificada.
#[derive(Debug, Clone)]
struct Local {
    id: u64,
    hash: ContentHash,
    bytes: Vec<u8>,
}

/// Una copia del otro lado anunciada y todavía no escrita.
#[derive(Debug, Clone)]
struct Remote {
    id: u64,
    hash: ContentHash,
    len: usize,
    requested: bool,
    buf: Vec<u8>,
}

/// Transferencia de la copia local en curso.
#[derive(Debug, Clone)]
struct Outgoing {
    id: u64,
    pos: usize,
}

#[derive(Debug, Clone)]
pub struct ClipSync {
    cfg: SyncConfig,
    focused: bool,
    /// Hash de lo que tiene el portapapeles local, visto o escrito por nosotros.
    current: Option<ContentHash>,
    /// La copia vigente entre los dos lados, propia o ajena: `(id, hash)` de la
    /// última que ganó. Solo se acepta una oferta mayor.
    winner: Option<(u64, ContentHash)>,
    /// Reloj de pared en microsegundos; piso de los ids de las copias nuevas.
    clock: fn() -> u64,
    local: Option<Local>,
    remote: Option<Remote>,
    outgoing: Option<Outgoing>,
    stats: SyncStats,
}

impl ClipSync {
    /// `focused`: si esta máquina tiene ahora el teclado y el mouse.
    pub fn new(cfg: SyncConfig, focused: bool) -> Self {
        Self::with_clock(cfg, focused, wall_micros)
    }

    pub(crate) fn with_clock(cfg: SyncConfig, focused: bool, clock: fn() -> u64) -> Self {
        Self {
            cfg: cfg.clamped(),
            focused,
            current: None,
            winner: None,
            clock,
            local: None,
            remote: None,
            outgoing: None,
            stats: SyncStats::default(),
        }
    }

    pub fn stats(&self) -> SyncStats {
        self.stats
    }

    pub fn focused(&self) -> bool {
        self.focused
    }

    /// Hay una copia del otro lado anunciada que todavía no se escribió.
    pub fn has_pending(&self) -> bool {
        self.remote.is_some()
    }

    /// Hay trozos de la copia local por mandar ([`Self::next_chunk`]).
    pub fn is_sending(&self) -> bool {
        self.outgoing.is_some()
    }

    /// Empieza una sesión (enlace nuevo) con lo que tiene ahora el portapapeles
    /// local. Si cambió mientras no había enlace, se anuncia; si no, se vuelve a
    /// anunciar la copia local vigente, por si su anuncio se perdió con el enlace
    /// anterior (el otro lado ignora lo que ya tiene). Lo pendiente del otro lado
    /// se conserva (su dueño lo sigue teniendo), pero la transferencia a medias
    /// se descarta y se vuelve a pedir.
    pub fn start(&mut self, local: Option<ClipContent>) -> Vec<Action> {
        self.begin(Some(local))
    }

    /// Como [`Self::start`], cuando no se pudo leer el portapapeles local: se
    /// sigue con lo último que se sabía de él.
    pub fn resume(&mut self) -> Vec<Action> {
        self.begin(None)
    }

    fn begin(&mut self, local: Option<Option<ClipContent>>) -> Vec<Action> {
        self.outgoing = None;
        if let Some(r) = &mut self.remote {
            r.requested = false;
            r.buf = Vec::new();
        }
        let mut out = local.map_or_else(Vec::new, |l| self.local_changed(l));
        if out.is_empty()
            && let Some(offer) = self.offer_again()
        {
            out.push(Action::Send(offer));
        }
        out.extend(self.request_if_needed());
        out
    }

    fn offer_again(&mut self) -> Option<ClipMsg> {
        let local = self
            .local
            .as_ref()
            .filter(|l| Some(l.hash) == self.current && Some((l.id, l.hash)) == self.winner)?;
        let content = if local.bytes.len() <= self.cfg.inline_max {
            Some(tandem_proto::decode::<ClipContent>(&local.bytes).ok()?)
        } else {
            None
        };
        self.stats.offers_sent += 1;
        Some(ClipMsg::Offer {
            id: local.id,
            hash: local.hash,
            len: local.bytes.len() as u64,
            content,
        })
    }

    /// El portapapeles local cambió; `content` es lo que tiene ahora (`None` si
    /// está vacío o solo tiene formatos que no se sincronizan, como archivos).
    pub fn local_changed(&mut self, content: Option<ClipContent>) -> Vec<Action> {
        let content = content.and_then(ClipContent::normalized);
        let hash = content.as_ref().map(ClipContent::hash);
        if hash == self.current {
            // Nuestra propia escritura, o un cambio que no cambió nada.
            return Vec::new();
        }
        self.current = hash;
        // Lo copiado ahora es más nuevo que lo que hubiera del otro lado.
        self.remote = None;
        self.outgoing = None;
        self.local = None;
        let (Some(content), Some(hash)) = (content, hash) else {
            return Vec::new();
        };
        let bytes = match tandem_proto::encode(&content) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("portapapeles: no se pudo codificar: {e}");
                return Vec::new();
            }
        };
        if bytes.len() > self.cfg.max_len {
            tracing::warn!(
                len = bytes.len(),
                "portapapeles: la copia supera el máximo, no se sincroniza"
            );
            return Vec::new();
        }
        // Reloj de Lamport con piso en el reloj de pared: una copia hecha después
        // de ver la del otro lado siempre es mayor, y un agente reiniciado (que
        // olvidó todo) no queda por debajo de lo que el otro recuerda.
        let id = self
            .winner
            .map_or(0, |(id, _)| id.saturating_add(1))
            .max((self.clock)());
        self.winner = Some((id, hash));
        let inline = (bytes.len() <= self.cfg.inline_max).then_some(content);
        let offer = ClipMsg::Offer {
            id,
            hash,
            len: bytes.len() as u64,
            content: inline,
        };
        self.local = Some(Local { id, hash, bytes });
        self.stats.offers_sent += 1;
        vec![Action::Send(offer)]
    }

    /// Cambió el foco del teclado y el mouse. Al llegar a esta máquina se pide
    /// lo pendiente: es cuando el usuario puede pegar.
    pub fn set_focus(&mut self, focused: bool) -> Vec<Action> {
        self.focused = focused;
        self.request_if_needed()
    }

    pub fn on_message(&mut self, msg: ClipMsg) -> Vec<Action> {
        match msg {
            ClipMsg::Offer {
                id,
                hash,
                len,
                content,
            } => self.on_offer(id, hash, len, content),
            ClipMsg::Request { id } => self.on_request(id),
            ClipMsg::Chunk { id, data, last } => self.on_chunk(id, data, last),
            ClipMsg::Unavailable { id } => {
                if self.remote.as_ref().is_some_and(|r| r.id == id) {
                    self.remote = None;
                }
                Vec::new()
            }
        }
    }

    /// El siguiente trozo de la transferencia en curso.
    pub fn next_chunk(&mut self) -> Option<ClipMsg> {
        let out = self.outgoing.as_mut()?;
        let Some(local) = self.local.as_ref().filter(|l| l.id == out.id) else {
            self.outgoing = None;
            return None;
        };
        let end = (out.pos + self.cfg.chunk_len).min(local.bytes.len());
        let data = local.bytes[out.pos..end].to_vec();
        out.pos = end;
        let last = end == local.bytes.len();
        let id = out.id;
        if last {
            self.outgoing = None;
        }
        Some(ClipMsg::Chunk { id, data, last })
    }

    fn on_offer(
        &mut self,
        id: u64,
        hash: ContentHash,
        len: u64,
        content: Option<ClipContent>,
    ) -> Vec<Action> {
        self.stats.offers_received += 1;
        if Some((id, hash)) <= self.winner {
            // Más vieja que la vigente: una copia que ya vimos (reanunciada tras
            // un cambio de enlace) o que se cruzó con la nuestra y perdió. El
            // otro lado compara igual y se queda con la misma.
            return Vec::new();
        }
        self.winner = Some((id, hash));
        // Lo que se ofrecía de aquí ya no se va a pedir.
        self.local = None;
        self.outgoing = None;
        self.remote = None;
        if Some(hash) == self.current {
            // Ya lo tenemos: la misma copia de los dos lados.
            return Vec::new();
        }
        if let Some(content) = content {
            return match content.normalized() {
                Some(c) if c.hash() == hash => self.write(c),
                _ => self.reject("oferta con contenido que no coincide con su hash"),
            };
        }
        let Ok(len) = usize::try_from(len) else {
            return self.reject("oferta demasiado grande");
        };
        if len > self.cfg.max_len {
            return self.reject("oferta demasiado grande");
        }
        self.remote = Some(Remote {
            id,
            hash,
            len,
            requested: false,
            buf: Vec::new(),
        });
        self.request_if_needed()
    }

    fn on_request(&mut self, id: u64) -> Vec<Action> {
        if self.local.as_ref().is_some_and(|l| l.id == id) {
            self.outgoing = Some(Outgoing { id, pos: 0 });
            Vec::new()
        } else {
            vec![Action::Send(ClipMsg::Unavailable { id })]
        }
    }

    fn on_chunk(&mut self, id: u64, data: Vec<u8>, last: bool) -> Vec<Action> {
        let Some(remote) = self.remote.as_mut().filter(|r| r.id == id && r.requested) else {
            // De una transferencia que ya no interesa.
            return Vec::new();
        };
        if remote.buf.len() + data.len() > remote.len {
            self.remote = None;
            return self.reject("la transferencia supera el tamaño anunciado");
        }
        if remote.buf.is_empty() {
            remote.buf.reserve_exact(remote.len);
        }
        remote.buf.extend_from_slice(&data);
        if !last {
            return Vec::new();
        }
        let Some(remote) = self.remote.take() else {
            return Vec::new();
        };
        if remote.buf.len() != remote.len {
            return self.reject("la transferencia terminó antes del tamaño anunciado");
        }
        match tandem_proto::decode::<ClipContent>(&remote.buf)
            .ok()
            .and_then(ClipContent::normalized)
        {
            Some(c) if c.hash() == remote.hash => self.write(c),
            _ => self.reject("la transferencia no coincide con su hash"),
        }
    }

    fn write(&mut self, content: ClipContent) -> Vec<Action> {
        self.current = Some(content.hash());
        self.stats.written += 1;
        vec![Action::Write(content)]
    }

    fn reject(&mut self, why: &str) -> Vec<Action> {
        tracing::warn!("portapapeles: {why}");
        self.stats.rejected += 1;
        Vec::new()
    }

    fn request_if_needed(&mut self) -> Vec<Action> {
        match &mut self.remote {
            Some(r) if self.focused && !r.requested => {
                r.requested = true;
                vec![Action::Send(ClipMsg::Request { id: r.id })]
            }
            _ => Vec::new(),
        }
    }
}

fn wall_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests;
