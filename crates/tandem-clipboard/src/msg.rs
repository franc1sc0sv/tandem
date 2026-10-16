//! Lo que viaja por el stream del portapapeles. El protocolo es simétrico: los
//! dos agentes anuncian sus copias y piden las del otro.
//!
//! 1. Quien copia manda [`ClipMsg::Offer`] con el hash y el tamaño. Si el
//!    contenido es chico va ahí mismo y el otro lo escribe al llegar.
//! 2. Si no, la transferencia es perezosa: el otro lado lo pide con
//!    [`ClipMsg::Request`] solo cuando hace falta (tiene el foco del teclado y
//!    el mouse, así que el usuario puede pegar), y llega en [`ClipMsg::Chunk`]s.
//! 3. Si para entonces el dueño ya copió otra cosa, responde
//!    [`ClipMsg::Unavailable`]. Si copia otra cosa a mitad de una
//!    transferencia, la corta y manda el `Offer` nuevo, que la reemplaza.

use serde::{Deserialize, Serialize};

use crate::content::{ClipContent, ContentHash};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipMsg {
    /// El portapapeles de quien lo manda cambió.
    Offer {
        /// Identifica esta copia en los pedidos y la ordena: es un reloj de
        /// Lamport (mayor que todo lo visto del otro lado), con el hash como
        /// desempate. Gana la oferta mayor, así dos copias cruzadas terminan
        /// igual en los dos lados sin reloj común.
        id: u64,
        hash: ContentHash,
        /// Largo de la codificación del contenido, la que viaja en los `Chunk`.
        len: u64,
        /// El contenido mismo, si es chico.
        content: Option<ClipContent>,
    },
    /// Pedido del contenido anunciado con `id`.
    Request { id: u64 },
    /// Un trozo de la codificación `postcard` del contenido, en orden. `last`
    /// marca el final.
    Chunk { id: u64, data: Vec<u8>, last: bool },
    /// La copia `id` ya no existe (el dueño copió otra cosa).
    Unavailable { id: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Image;

    #[test]
    fn mensajes_ida_y_vuelta() {
        let content = ClipContent {
            text: Some("ñandú".into()),
            html: Some("<b>ñandú</b>".into()),
            image: Some(Image {
                width: 1,
                height: 1,
                rgba: vec![1, 2, 3, 4],
            }),
        };
        let msgs = [
            ClipMsg::Offer {
                id: 1,
                hash: content.hash(),
                len: 99,
                content: Some(content.clone()),
            },
            ClipMsg::Offer {
                id: u64::MAX,
                hash: content.hash(),
                len: u64::MAX,
                content: None,
            },
            ClipMsg::Request { id: 3 },
            ClipMsg::Chunk {
                id: 3,
                data: vec![0, 255, 7],
                last: true,
            },
            ClipMsg::Unavailable { id: 4 },
        ];
        for m in msgs {
            let frame = tandem_proto::encode(&m).unwrap();
            assert_eq!(tandem_proto::decode::<ClipMsg>(&frame).unwrap(), m);
        }
    }
}
