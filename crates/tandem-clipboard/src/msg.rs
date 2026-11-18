//! What travels over the clipboard stream. The protocol is symmetric: the
//! two agents announce their copies and request the other's.
//!
//! 1. Whoever copies sends [`ClipMsg::Offer`] with the hash and the size. If the
//!    content is small it goes right there and the other writes it on arrival.
//! 2. If not, the transfer is lazy: the other side requests it with
//!    [`ClipMsg::Request`] only when needed (it has the keyboard and
//!    mouse focus, so the user can paste), and it arrives in [`ClipMsg::Chunk`]s.
//! 3. If by then the owner has copied something else, it answers
//!    [`ClipMsg::Unavailable`]. If it copies something else midway through a
//!    transfer, it cuts it and sends the new `Offer`, which replaces it.

use serde::{Deserialize, Serialize};

use crate::content::{ClipContent, ContentHash};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipMsg {
    /// The clipboard of whoever sends it changed.
    Offer {
        /// Identifies this copy in the requests and orders it: it is a Lamport
        /// clock (greater than everything seen from the other side), with the hash as
        /// tie-breaker. The greater offer wins, so two crossed copies end up
        /// the same on both sides without a common clock.
        id: u64,
        hash: ContentHash,
        /// Length of the content encoding, the one that travels in the `Chunk`s.
        len: u64,
        /// The content itself, if it is small.
        content: Option<ClipContent>,
    },
    /// Request for the content announced with `id`.
    Request { id: u64 },
    /// A chunk of the content's `postcard` encoding, in order. `last`
    /// marks the end.
    Chunk { id: u64, data: Vec<u8>, last: bool },
    /// The copy `id` no longer exists (the owner copied something else).
    Unavailable { id: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Image;

    #[test]
    fn messages_round_trip() {
        let content = ClipContent {
            text: Some("\u{fc}ber".into()),
            html: Some("<b>\u{fc}ber</b>".into()),
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
