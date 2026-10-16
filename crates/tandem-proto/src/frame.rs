//! Codificación `postcard` y framing por longitud.
//!
//! Cada trama es `[longitud: u32 LE][mensaje postcard]`. El enlace entrega bytes en
//! trozos arbitrarios; [`FrameDecoder`] los junta y devuelve tramas completas.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::ProtoError;
use crate::msg::{Hello, check_version};

const HEADER_LEN: usize = 4;

/// Tamaño máximo de un mensaje. Los mensajes de control son pequeños; el tope evita
/// que una longitud corrupta o maliciosa haga reservar memoria sin límite.
pub const MAX_FRAME_LEN: usize = 1 << 20;

/// Codifica un mensaje sin trama.
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, ProtoError> {
    postcard::to_stdvec(msg).map_err(ProtoError::Encode)
}

/// Decodifica un mensaje completo; sobrantes al final son un error, no se ignoran.
pub fn decode<T: DeserializeOwned>(payload: &[u8]) -> Result<T, ProtoError> {
    let (msg, rest) = postcard::take_from_bytes(payload).map_err(ProtoError::Decode)?;
    if rest.is_empty() {
        Ok(msg)
    } else {
        Err(ProtoError::TrailingBytes(rest.len()))
    }
}

/// Decodifica el [`Hello`] de apertura y rechaza otra versión del protocolo.
///
/// La versión se lee y se comprueba antes que el resto del mensaje: un Hello de
/// otra versión da [`ProtoError::IncompatibleVersion`] aunque su forma sea distinta.
pub fn decode_hello(payload: &[u8]) -> Result<Hello, ProtoError> {
    let (theirs, _) = postcard::take_from_bytes::<u16>(payload).map_err(ProtoError::Decode)?;
    check_version(theirs)?;
    decode(payload)
}

/// Codifica un mensaje con su trama, listo para escribir en el enlace.
pub fn encode_frame<T: Serialize>(msg: &T) -> Result<Vec<u8>, ProtoError> {
    let payload = encode(msg)?;
    let too_large = ProtoError::FrameTooLarge {
        len: payload.len(),
        max: MAX_FRAME_LEN,
    };
    if payload.len() > MAX_FRAME_LEN {
        return Err(too_large);
    }
    let len = u32::try_from(payload.len()).map_err(|_| too_large)?;
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Junta bytes del enlace y los separa en tramas.
#[derive(Debug)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    max_len: usize,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::with_max_len(MAX_FRAME_LEN)
    }

    pub fn with_max_len(max_len: usize) -> Self {
        FrameDecoder {
            buf: Vec::new(),
            max_len,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Bytes recibidos que todavía no forman una trama completa.
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    /// La siguiente trama completa, o `None` si faltan bytes.
    ///
    /// Tras [`ProtoError::FrameTooLarge`] el flujo no se puede resincronizar: hay que
    /// cerrar la conexión.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, ProtoError> {
        let Some(header) = self.buf.first_chunk::<HEADER_LEN>() else {
            return Ok(None);
        };
        let len = u32::from_le_bytes(*header) as usize;
        if len > self.max_len {
            return Err(ProtoError::FrameTooLarge {
                len,
                max: self.max_len,
            });
        }
        if self.buf.len() < HEADER_LEN + len {
            return Ok(None);
        }
        let payload = self.buf[HEADER_LEN..HEADER_LEN + len].to_vec();
        self.buf.drain(..HEADER_LEN + len);
        Ok(Some(payload))
    }

    /// Como [`FrameDecoder::next_frame`], ya decodificado como `T`.
    pub fn next_message<T: DeserializeOwned>(&mut self) -> Result<Option<T>, ProtoError> {
        self.next_frame()?.map(|p| decode(&p)).transpose()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use proptest::test_runner::{Config, RngSeed};

    use super::*;
    use crate::PROTOCOL_VERSION;
    use crate::msg::samples;
    use crate::msg::{ControlMsg, Peer};

    fn stream(msgs: &[ControlMsg]) -> Vec<u8> {
        msgs.iter().flat_map(|m| encode_frame(m).unwrap()).collect()
    }

    /// Alimenta `bytes` en trozos de los tamaños dados (cíclicamente) y junta lo decodificado.
    fn feed_in_chunks(bytes: &[u8], sizes: &[usize]) -> Vec<ControlMsg> {
        let mut dec = FrameDecoder::new();
        let mut out = Vec::new();
        let mut rest = bytes;
        let mut sizes = sizes.iter().copied().cycle();
        while !rest.is_empty() {
            let n = sizes.next().unwrap_or(rest.len()).clamp(1, rest.len());
            let (chunk, tail) = rest.split_at(n);
            rest = tail;
            dec.push(chunk);
            while let Some(m) = dec.next_message::<ControlMsg>().unwrap() {
                out.push(m);
            }
        }
        assert_eq!(dec.buffered(), 0);
        out
    }

    #[test]
    fn frame_header_is_u32_le_length() {
        let frame = encode_frame(&ControlMsg::Ping { nonce: 1 }).unwrap();
        let payload = encode(&ControlMsg::Ping { nonce: 1 }).unwrap();
        assert_eq!(frame[..4], (payload.len() as u32).to_le_bytes());
        assert_eq!(frame[4..], payload[..]);
    }

    #[test]
    fn byte_by_byte_and_every_two_way_split() {
        let msgs = samples::control();
        let bytes = stream(&msgs);
        assert_eq!(feed_in_chunks(&bytes, &[1]), msgs);
        for cut in 0..=bytes.len() {
            let mut dec = FrameDecoder::new();
            let mut out = Vec::new();
            for part in [&bytes[..cut], &bytes[cut..]] {
                dec.push(part);
                while let Some(m) = dec.next_message::<ControlMsg>().unwrap() {
                    out.push(m);
                }
            }
            assert_eq!(out, msgs, "corte en {cut}");
        }
    }

    #[test]
    fn incomplete_frame_waits_for_more_bytes() {
        let frame = encode_frame(&ControlMsg::GetState).unwrap();
        let mut dec = FrameDecoder::new();
        dec.push(&frame[..frame.len() - 1]);
        assert_eq!(dec.next_frame(), Ok(None));
        assert_eq!(dec.buffered(), frame.len() - 1);
        dec.push(&frame[frame.len() - 1..]);
        assert_eq!(
            dec.next_message::<ControlMsg>(),
            Ok(Some(ControlMsg::GetState))
        );
    }

    #[test]
    fn oversized_header_is_rejected_before_buffering_the_body() {
        let mut dec = FrameDecoder::with_max_len(16);
        dec.push(&17u32.to_le_bytes());
        assert_eq!(
            dec.next_frame(),
            Err(ProtoError::FrameTooLarge { len: 17, max: 16 })
        );
        let mut dec = FrameDecoder::new();
        dec.push(&u32::MAX.to_le_bytes());
        assert!(matches!(
            dec.next_frame(),
            Err(ProtoError::FrameTooLarge { .. })
        ));
    }

    #[test]
    fn encode_rejects_oversized_message() {
        let big = ControlMsg::StepResult {
            id: 0,
            outcome: crate::StepOutcome::Failed("x".repeat(MAX_FRAME_LEN)),
        };
        assert!(matches!(
            encode_frame(&big),
            Err(ProtoError::FrameTooLarge { .. })
        ));
    }

    #[test]
    fn trailing_bytes_are_an_error() {
        let mut payload = encode(&ControlMsg::GetState).unwrap();
        payload.push(0);
        assert_eq!(
            decode::<ControlMsg>(&payload),
            Err(ProtoError::TrailingBytes(1))
        );
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(matches!(
            decode::<ControlMsg>(&[0xFF]),
            Err(ProtoError::Decode(_))
        ));
    }

    #[test]
    fn hello_of_current_version_is_accepted() {
        for h in samples::hellos() {
            assert_eq!(decode_hello(&encode(&h).unwrap()), Ok(h));
        }
    }

    #[test]
    fn hello_of_other_version_is_rejected() {
        for theirs in [0, PROTOCOL_VERSION + 1, u16::MAX] {
            let h = Hello {
                version: theirs,
                peer: Peer::Ui,
            };
            assert_eq!(
                decode_hello(&encode(&h).unwrap()),
                Err(ProtoError::IncompatibleVersion {
                    ours: PROTOCOL_VERSION,
                    theirs,
                })
            );
        }
    }

    /// Un agente de antes de T-084 no anuncia capacidades: se lo rechaza en el saludo,
    /// con un error que lo dice.
    #[test]
    fn an_agent_without_capabilities_is_rejected_with_a_clear_version_error() {
        let old = Hello {
            version: crate::CAPABILITIES_SINCE - 1,
            peer: Peer::Agent(tandem_core::Machine::Pc),
        };
        let e = decode_hello(&encode(&old).unwrap()).unwrap_err();
        assert_eq!(
            e,
            ProtoError::IncompatibleVersion {
                ours: PROTOCOL_VERSION,
                theirs: 9,
            }
        );
        let text = e.to_string();
        assert!(text.contains("versión de protocolo incompatible"), "{text}");
        assert!(text.contains("sin anuncio de capacidades"), "{text}");
        assert!(text.contains("la misma versión de Tandem"), "{text}");
    }

    #[test]
    fn hello_of_other_version_with_another_shape_is_still_a_version_error() {
        // Un Hello futuro que cambió todo menos el primer campo.
        let future = encode(&(PROTOCOL_VERSION + 1, "otro formato", [9u8; 5])).unwrap();
        assert_eq!(
            decode_hello(&future),
            Err(ProtoError::IncompatibleVersion {
                ours: PROTOCOL_VERSION,
                theirs: PROTOCOL_VERSION + 1,
            })
        );
    }

    #[test]
    fn empty_hello_is_a_decode_error() {
        assert!(matches!(decode_hello(&[]), Err(ProtoError::Decode(_))));
    }

    #[test]
    fn zero_length_frame_is_an_empty_payload_not_a_stall() {
        let mut dec = FrameDecoder::new();
        dec.push(&[0, 0, 0, 0]);
        dec.push(&encode_frame(&ControlMsg::GetState).unwrap());
        assert_eq!(dec.next_frame(), Ok(Some(Vec::new())));
        // La siguiente trama sigue alineada.
        assert_eq!(
            dec.next_message::<ControlMsg>(),
            Ok(Some(ControlMsg::GetState))
        );
        // Y un mensaje vacío no es un mensaje válido.
        assert!(matches!(
            decode::<ControlMsg>(&[]),
            Err(ProtoError::Decode(_))
        ));
    }

    #[test]
    fn oversized_frame_error_is_sticky() {
        // Tras FrameTooLarge no hay resincronización: el error se repite, no se
        // interpreta el cuerpo como tramas nuevas.
        let mut dec = FrameDecoder::with_max_len(8);
        dec.push(&9u32.to_le_bytes());
        dec.push(&encode_frame(&ControlMsg::GetState).unwrap());
        for _ in 0..3 {
            assert_eq!(
                dec.next_frame(),
                Err(ProtoError::FrameTooLarge { len: 9, max: 8 })
            );
        }
    }

    #[test]
    fn frame_of_exactly_max_len_is_accepted() {
        let mut dec = FrameDecoder::with_max_len(3);
        dec.push(&3u32.to_le_bytes());
        dec.push(&[1, 2, 3]);
        assert_eq!(dec.next_frame(), Ok(Some(vec![1, 2, 3])));
        assert_eq!(dec.buffered(), 0);
    }

    fn control_msg() -> impl Strategy<Value = ControlMsg> {
        prop_oneof![
            any::<u64>().prop_map(|nonce| ControlMsg::Ping { nonce }),
            any::<u64>().prop_map(|nonce| ControlMsg::Pong { nonce }),
            proptest::sample::select(samples::control()),
            (any::<u64>(), ".{0,300}").prop_map(|(id, e)| ControlMsg::StepResult {
                id,
                outcome: crate::StepOutcome::Failed(e),
            }),
        ]
    }

    // Semilla fija: las particiones son aleatorias pero la corrida es reproducible.
    proptest! {
        #![proptest_config(Config {
            cases: 512,
            rng_seed: RngSeed::Fixed(0x7A4D_E002),
            failure_persistence: None,
            ..Config::default()
        })]

        #[test]
        fn any_partition_decodes_the_same_messages(
            msgs in proptest::collection::vec(control_msg(), 0..40),
            sizes in proptest::collection::vec(1usize..64, 1..50),
        ) {
            let bytes = stream(&msgs);
            prop_assert_eq!(feed_in_chunks(&bytes, &sizes), msgs);
        }

        /// Bytes cualesquiera del otro lado nunca hacen entrar en pánico al
        /// decodificador ni a ningún tipo de mensaje: siempre `Ok` o `Err`.
        #[test]
        fn garbage_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            let _ = decode::<ControlMsg>(&bytes);
            let _ = decode::<crate::UiRequest>(&bytes);
            let _ = decode::<crate::AgentToUi>(&bytes);
            let _ = decode_hello(&bytes);
            let mut dec = FrameDecoder::with_max_len(64);
            dec.push(&bytes);
            // Cada trama consume al menos la cabecera: el bucle termina.
            for _ in 0..=bytes.len() / HEADER_LEN {
                match dec.next_message::<ControlMsg>() {
                    Ok(None) | Err(ProtoError::FrameTooLarge { .. }) => break,
                    Ok(Some(_)) | Err(_) => {}
                }
            }
            prop_assert!(dec.buffered() <= bytes.len());
        }

        /// Un mensaje válido seguido de basura: el válido sale entero y la basura
        /// queda para la siguiente trama, sin contaminar al anterior.
        #[test]
        fn valid_frame_survives_trailing_garbage(
            msg in control_msg(),
            tail in proptest::collection::vec(any::<u8>(), 0..64),
        ) {
            let mut dec = FrameDecoder::new();
            dec.push(&encode_frame(&msg).unwrap());
            dec.push(&tail);
            prop_assert_eq!(dec.next_message::<ControlMsg>(), Ok(Some(msg)));
            prop_assert_eq!(dec.buffered(), tail.len());
        }
    }
}
