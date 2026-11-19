//! What the session agent and the helper say to each other over the helper's pipe.
//!
//! One request, one reply, in order. Each message is a frame: a little-endian `u32` length
//! and the message in postcard. The first request is always [`Request::Hello`] with
//! [`PROTOCOL_VERSION`]: the service's copy of `tandemd.exe` is only refreshed by an elevated
//! install, so after an app update the two ends can be from different releases. The version
//! only changes when these messages do.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};
use tandem_input::windows::emulate::{EmulateError, WinInput};

pub const PROTOCOL_VERSION: u32 = 1;

/// Upper bound of a frame: a batch of `INPUT` records is a few hundred bytes.
pub const MAX_FRAME: usize = 64 * 1024;

/// A Win32 `INPUT` record on the wire (the portable [`WinInput`] has no serde).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireInput {
    Keyboard {
        vk: u16,
        scan: u16,
        flags: u32,
        extra_info: u64,
    },
    Mouse {
        dx: i32,
        dy: i32,
        data: u32,
        flags: u32,
        extra_info: u64,
    },
}

impl From<&WinInput> for WireInput {
    fn from(i: &WinInput) -> Self {
        match *i {
            WinInput::Keyboard {
                vk,
                scan,
                flags,
                extra_info,
            } => WireInput::Keyboard {
                vk,
                scan,
                flags,
                extra_info: extra_info as u64,
            },
            WinInput::Mouse {
                dx,
                dy,
                data,
                flags,
                extra_info,
            } => WireInput::Mouse {
                dx,
                dy,
                data,
                flags,
                extra_info: extra_info as u64,
            },
        }
    }
}

impl From<&WireInput> for WinInput {
    fn from(i: &WireInput) -> Self {
        match *i {
            WireInput::Keyboard {
                vk,
                scan,
                flags,
                extra_info,
            } => WinInput::Keyboard {
                vk,
                scan,
                flags,
                extra_info: extra_info as usize,
            },
            WireInput::Mouse {
                dx,
                dy,
                data,
                flags,
                extra_info,
            } => WinInput::Mouse {
                dx,
                dy,
                data,
                flags,
                extra_info: extra_info as usize,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    Hello {
        protocol: u32,
    },
    /// One `SendInput` call with these records, on the desktop that has the input.
    Inject(Vec<WireInput>),
    /// Ctrl+Alt+Del: the helper passes it to the service, which calls `SendSAS`.
    Sas,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    Ok,
    /// `SendInput` took `sent` of `expected` records.
    Blocked {
        sent: u32,
        expected: u32,
    },
    /// Another error, as text.
    Failed(String),
    /// The other end speaks another protocol version.
    Version {
        protocol: u32,
    },
}

impl Reply {
    /// The reply to an injection, from what `SendInput` did.
    pub fn from_send(r: Result<(), EmulateError>) -> Self {
        match r {
            Ok(()) => Reply::Ok,
            Err(EmulateError::Blocked { sent, expected }) => Reply::Blocked {
                sent: u32::try_from(sent).unwrap_or(u32::MAX),
                expected: u32::try_from(expected).unwrap_or(u32::MAX),
            },
            Err(e) => Reply::Failed(e.to_string()),
        }
    }
}

pub fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let body =
        postcard::to_stdvec(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if body.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("frame of {} bytes", body.len()),
        ));
    }
    let len =
        u32::try_from(body.len()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // One write per frame: on a message-mode pipe that is one message.
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&body);
    w.write_all(&frame)?;
    w.flush()
}

/// `Ok(None)` if the other end closed cleanly between frames.
pub fn read_frame<T: for<'de> Deserialize<'de>>(r: &mut impl Read) -> io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes"),
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    postcard::from_bytes(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn round_trip<T>(msg: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let mut buf = Vec::new();
        write_frame(&mut buf, msg).unwrap();
        read_frame(&mut buf.as_slice()).unwrap().unwrap()
    }

    #[test]
    fn requests_and_replies_round_trip() {
        let inject = Request::Inject(vec![
            WireInput::Keyboard {
                vk: 0,
                scan: 0x1D,
                flags: 8,
                extra_info: 0x544E_444D,
            },
            WireInput::Mouse {
                dx: -5,
                dy: 65_535,
                data: (-120i32) as u32,
                flags: 0xC001,
                extra_info: 0x544E_444D,
            },
        ]);
        for r in [inject, Request::Hello { protocol: 1 }, Request::Sas] {
            assert_eq!(round_trip(&r), r);
        }
        for r in [
            Reply::Ok,
            Reply::Blocked {
                sent: 0,
                expected: 3,
            },
            Reply::Failed("no".into()),
            Reply::Version { protocol: 9 },
        ] {
            assert_eq!(round_trip(&r), r);
        }
    }

    #[test]
    fn a_clean_close_is_none_and_a_cut_frame_is_an_error() {
        let empty: &[u8] = &[];
        assert_eq!(read_frame::<Reply>(&mut { empty }).unwrap(), None);
        let mut buf = Vec::new();
        write_frame(&mut buf, &Request::Sas).unwrap();
        buf.pop();
        assert!(read_frame::<Request>(&mut buf.as_slice()).is_err());
    }

    #[test]
    fn oversized_frames_are_refused_before_reading_them() {
        let mut buf = ((MAX_FRAME + 1) as u32).to_le_bytes().to_vec();
        buf.extend_from_slice(&[0; 8]);
        let err = read_frame::<Request>(&mut buf.as_slice()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn send_results_map_to_replies() {
        assert_eq!(Reply::from_send(Ok(())), Reply::Ok);
        assert_eq!(
            Reply::from_send(Err(EmulateError::Blocked {
                sent: 1,
                expected: 2
            })),
            Reply::Blocked {
                sent: 1,
                expected: 2
            }
        );
        assert!(matches!(
            Reply::from_send(Err(EmulateError::NoDesktop)),
            Reply::Failed(_)
        ));
    }

    fn any_input() -> impl Strategy<Value = WinInput> {
        prop_oneof![
            (any::<u16>(), any::<u16>(), any::<u32>()).prop_map(|(vk, scan, flags)| {
                WinInput::Keyboard {
                    vk,
                    scan,
                    flags,
                    extra_info: tandem_input::windows::TANDEM_EXTRA_INFO,
                }
            }),
            (any::<i32>(), any::<i32>(), any::<u32>(), any::<u32>()).prop_map(
                |(dx, dy, data, flags)| WinInput::Mouse {
                    dx,
                    dy,
                    data,
                    flags,
                    extra_info: tandem_input::windows::TANDEM_EXTRA_INFO,
                }
            ),
        ]
    }

    proptest! {
        /// Every record reaches the helper exactly as the emulator built it.
        #[test]
        fn inputs_survive_the_wire(inputs in proptest::collection::vec(any_input(), 0..64)) {
            let wire: Vec<WireInput> = inputs.iter().map(WireInput::from).collect();
            let Request::Inject(back) = round_trip(&Request::Inject(wire)) else {
                panic!("not an injection");
            };
            let back: Vec<WinInput> = back.iter().map(WinInput::from).collect();
            prop_assert_eq!(back, inputs);
        }
    }
}
