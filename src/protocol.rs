//! Bounded RESP2 command decoding and binary-safe response encoding.
//!
//! Commands are nonempty arrays of non-null bulk strings only. Decoding validates
//! a complete frame before allocating its arguments; pipelined bytes are not
//! charged to the first frame. Returned arguments never borrow the input buffer.

use bytes::Bytes;
use std::{error::Error, fmt};

/// Resource limits for one command, including all RESP framing bytes.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_frame_bytes: usize,
    pub max_bulk_bytes: usize,
    pub max_args: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_frame_bytes: 8 * 1024 * 1024,
            max_bulk_bytes: 1024 * 1024,
            max_args: 1024,
        }
    }
}

/// Maximum digits in an array or bulk length, including leading zeroes.
pub const MAX_HEADER_DIGITS: usize = 20;

/// Malformed input or a resource limit violation. Messages never echo input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    InvalidType,
    InvalidLength,
    InvalidTerminator,
    HeaderTooLong,
    FrameTooLarge,
    BulkTooLarge,
    TooManyArguments,
    EmptyCommand,
    AllocationFailed,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidType => "expected an array of bulk strings",
            Self::InvalidLength => "invalid unsigned RESP length",
            Self::InvalidTerminator => "expected CRLF terminator",
            Self::HeaderTooLong => "RESP length header too long",
            Self::FrameTooLarge => "command exceeds frame limit",
            Self::BulkTooLarge => "bulk string exceeds limit",
            Self::TooManyArguments => "command exceeds argument limit",
            Self::EmptyCommand => "command must contain an argument",
            Self::AllocationFailed => "command allocation failed",
        })
    }
}

impl Error for ProtocolError {}

/// Parse a bounded unsigned header; reject limits even before its CRLF arrives.
fn header(
    input: &[u8],
    start: usize,
    marker: u8,
    limits: Limits,
    maximum: usize,
    limit_error: ProtocolError,
) -> Result<Option<(usize, usize)>, ProtocolError> {
    if start >= limits.max_frame_bytes {
        return Err(ProtocolError::FrameTooLarge);
    }
    let Some(&first) = input.get(start) else {
        return Ok(None);
    };
    if first != marker {
        return Err(ProtocolError::InvalidType);
    }
    let mut cursor = start + 1; // start is below max_frame_bytes, so cannot overflow.
    let mut value = 0usize;
    let mut digits = 0;
    loop {
        if cursor >= limits.max_frame_bytes {
            return Err(ProtocolError::FrameTooLarge);
        }
        let Some(&byte) = input.get(cursor) else {
            return Ok(None);
        };
        if byte == b'\r' {
            if digits == 0 {
                return Err(ProtocolError::InvalidLength);
            }
            let end = cursor.checked_add(2).ok_or(ProtocolError::FrameTooLarge)?;
            if end > limits.max_frame_bytes {
                return Err(ProtocolError::FrameTooLarge);
            }
            return match input.get(cursor + 1) {
                Some(b'\n') => Ok(Some((value, end))),
                Some(_) => Err(ProtocolError::InvalidTerminator),
                None => Ok(None),
            };
        }
        if !byte.is_ascii_digit() {
            return Err(ProtocolError::InvalidLength);
        }
        digits += 1;
        if digits > MAX_HEADER_DIGITS {
            return Err(ProtocolError::HeaderTooLong);
        }
        value = value
            .checked_mul(10)
            .and_then(|v| v.checked_add(usize::from(byte - b'0')))
            .ok_or(ProtocolError::InvalidLength)?;
        if value > maximum {
            return Err(limit_error);
        }
        cursor += 1;
    }
}

/// Decode one command. `Ok(None)` means more bytes are needed, not bad input.
///
/// Lengths are unsigned decimal (leading zeroes are allowed within the header
/// cap). Nulls, nested arrays, inline commands and non-bulk arguments are errors.
pub fn decode(input: &[u8], limits: Limits) -> Result<Option<(Vec<Bytes>, usize)>, ProtocolError> {
    if input.is_empty() {
        return Ok(None);
    }
    let Some((count, mut cursor)) = header(
        input,
        0,
        b'*',
        limits,
        limits.max_args,
        ProtocolError::TooManyArguments,
    )?
    else {
        return Ok(None);
    };
    if count == 0 {
        return Err(ProtocolError::EmptyCommand);
    }
    let array_end = cursor;
    // Even an empty bulk string costs six bytes: "$0\r\n\r\n".
    let minimum = count
        .checked_mul(6)
        .and_then(|n| n.checked_add(cursor))
        .ok_or(ProtocolError::FrameTooLarge)?;
    if minimum > limits.max_frame_bytes {
        return Err(ProtocolError::FrameTooLarge);
    }
    for index in 0..count {
        let Some((len, body)) = header(
            input,
            cursor,
            b'$',
            limits,
            limits.max_bulk_bytes,
            ProtocolError::BulkTooLarge,
        )?
        else {
            return Ok(None);
        };
        let end = body
            .checked_add(len)
            .and_then(|n| n.checked_add(2))
            .ok_or(ProtocolError::FrameTooLarge)?;
        let minimum_end = (count - index - 1)
            .checked_mul(6)
            .and_then(|n| n.checked_add(end))
            .ok_or(ProtocolError::FrameTooLarge)?;
        if minimum_end > limits.max_frame_bytes {
            return Err(ProtocolError::FrameTooLarge);
        }
        // Check a partial terminator too: an available wrong byte is not incomplete.
        if let Some(&byte) = input.get(end - 2)
            && byte != b'\r'
        {
            return Err(ProtocolError::InvalidTerminator);
        }
        if input.len() < end {
            return Ok(None);
        }
        if input[end - 1] != b'\n' {
            return Err(ProtocolError::InvalidTerminator);
        }
        cursor = end;
    }

    let consumed = cursor;
    let mut args = Vec::new();
    args.try_reserve_exact(count)
        .map_err(|_| ProtocolError::AllocationFailed)?;
    cursor = array_end;
    for _ in 0..count {
        // This pass visits only headers and slices validated above.
        let Some((len, body)) = header(
            input,
            cursor,
            b'$',
            limits,
            limits.max_bulk_bytes,
            ProtocolError::BulkTooLarge,
        )?
        else {
            return Ok(None);
        };
        args.push(Bytes::copy_from_slice(&input[body..body + len]));
        cursor = body + len + 2;
    }
    Ok(Some((args, consumed)))
}

/// RESP2 responses. CR/LF in simple strings or errors are encoded as spaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Simple(String),
    Error(String),
    Integer(i64),
    Bulk(Bytes),
    Null,
    Array(Vec<Frame>),
}

impl Frame {
    /// Append a response without allowing simple/error values to inject frames.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Simple(value) | Self::Error(value) => {
                out.push(if matches!(self, Self::Simple(_)) {
                    b'+'
                } else {
                    b'-'
                });
                out.extend(
                    value
                        .bytes()
                        .map(|b| if matches!(b, b'\r' | b'\n') { b' ' } else { b }),
                );
                out.extend_from_slice(b"\r\n");
            }
            Self::Integer(value) => {
                out.push(b':');
                out.extend_from_slice(value.to_string().as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            Self::Bulk(value) => {
                out.push(b'$');
                out.extend_from_slice(value.len().to_string().as_bytes());
                out.extend_from_slice(b"\r\n");
                out.extend_from_slice(value);
                out.extend_from_slice(b"\r\n");
            }
            Self::Null => out.extend_from_slice(b"$-1\r\n"),
            Self::Array(values) => {
                out.push(b'*');
                out.extend_from_slice(values.len().to_string().as_bytes());
                out.extend_from_slice(b"\r\n");
                for value in values {
                    value.encode(out);
                }
            }
        }
    }

    /// Encoded bytes, saturating at `usize::MAX` if an oversized tree cannot fit.
    /// Does not allocate or materialize shared bulk data.
    pub fn encoded_len(&self) -> usize {
        match self {
            Self::Simple(value) | Self::Error(value) => value.len().saturating_add(3),
            Self::Integer(value) => {
                let magnitude = value.unsigned_abs();
                let digits = if magnitude == 0 {
                    1
                } else {
                    magnitude.ilog10() as usize + 1
                };
                digits + usize::from(*value < 0) + 3
            }
            Self::Bulk(value) => value
                .len()
                .saturating_add(decimal_len(value.len()))
                .saturating_add(5),
            Self::Null => 5,
            Self::Array(values) => values.iter().fold(decimal_len(values.len()) + 3, |n, v| {
                n.saturating_add(v.encoded_len())
            }),
        }
    }
}

fn decimal_len(value: usize) -> usize {
    if value == 0 {
        1
    } else {
        value.ilog10() as usize + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn fragmented_binary_pipeline_and_owned_arguments() {
        let frame = b"*3\r\n$3\r\nSET\r\n$0\r\n\r\n$5\r\n\0\r\n\xffx\r\n";
        for split in 0..frame.len() {
            assert_eq!(
                decode(&frame[..split], Limits::default()),
                Ok(None),
                "split={split}"
            );
            let mut joined = frame[..split].to_vec();
            joined.extend_from_slice(&frame[split..]);
            let (args, used) = decode(&joined, Limits::default()).unwrap().unwrap();
            assert_eq!(used, frame.len());
            joined.fill(0);
            assert_eq!(
                args,
                [
                    Bytes::from_static(b"SET"),
                    Bytes::new(),
                    Bytes::from_static(b"\0\r\n\xffx")
                ]
            );
        }
        let pipeline = [frame.as_slice(), frame.as_slice()].concat();
        let limits = Limits {
            max_frame_bytes: frame.len(),
            ..Limits::default()
        };
        let (_, used) = decode(&pipeline, limits).unwrap().unwrap();
        assert_eq!(used, frame.len());
        assert!(decode(&pipeline[used..], limits).unwrap().is_some());
    }

    #[test]
    fn rejects_malformed_and_overflowing_input() {
        for input in [
            &b"PING\r\n"[..],
            b"+PING\r\n",
            b"*-1\r\n",
            b"*0\r\n",
            b"*+1\r\n",
            b"*1\n",
            b"*1\rx",
            b"*1\r\n$-1\r\n",
            b"*1\r\n*1\r\n",
            b"*1\r\n+OK\r\n",
            b"*1\r\n$1\r\nx\n",
            b"*1\r\n$1\r\nx\rx",
            b"*1\r\n$\r\n",
            b"*1\r\n$ 1\r\n",
            b"*1\r\n$1x\r\n",
        ] {
            assert!(decode(input, Limits::default()).is_err(), "input={input:?}");
        }
        let unlimited = Limits {
            max_frame_bytes: usize::MAX,
            max_bulk_bytes: usize::MAX,
            max_args: usize::MAX,
        };
        assert!(decode(b"*18446744073709551616\r\n", unlimited).is_err());
        assert!(decode(b"*1\r\n$18446744073709551615\r\n", unlimited).is_err());
        assert_eq!(
            decode(b"*000000000000000000000", unlimited),
            Err(ProtocolError::HeaderTooLong)
        );
        let error = decode(b"SECRET", Limits::default())
            .unwrap_err()
            .to_string();
        assert!(!error.contains("SECRET"));
    }

    #[test]
    fn limits_are_inclusive_and_declared_sizes_fail_early() {
        let limits = Limits {
            max_frame_bytes: 64,
            max_bulk_bytes: 3,
            max_args: 1,
        };
        assert_eq!(decode(b"*2", limits), Err(ProtocolError::TooManyArguments));
        assert_eq!(
            decode(b"*1\r\n$4", limits),
            Err(ProtocolError::BulkTooLarge)
        );
        let wire = b"*1\r\n$3\r\nabc\r\n";
        let exact = Limits {
            max_frame_bytes: wire.len(),
            ..limits
        };
        assert!(decode(wire, exact).unwrap().is_some());
        assert_eq!(
            decode(
                wire,
                Limits {
                    max_frame_bytes: wire.len() - 1,
                    ..limits
                }
            ),
            Err(ProtocolError::FrameTooLarge)
        );
        assert_eq!(
            decode(
                b"*1\r\n$3\r\n",
                Limits {
                    max_frame_bytes: 12,
                    ..limits
                }
            ),
            Err(ProtocolError::FrameTooLarge)
        );
        assert_eq!(
            decode(
                b"*10\r\n",
                Limits {
                    max_frame_bytes: 64,
                    max_args: 10,
                    ..limits
                }
            ),
            Err(ProtocolError::FrameTooLarge)
        );
    }

    #[test]
    fn response_lengths_and_injection_protection() {
        for frame in [
            Frame::Simple("a\r\nb".into()),
            Frame::Error("ERR bad\n+OK\r".into()),
            Frame::Integer(i64::MIN),
            Frame::Integer(i64::MAX),
            Frame::Integer(0),
            Frame::Null,
            Frame::Bulk(Bytes::from_static(b"\0\r\n")),
            Frame::Array(vec![Frame::Null, Frame::Array(vec![Frame::Integer(-10)])]),
        ] {
            let mut encoded = Vec::new();
            frame.encode(&mut encoded);
            assert_eq!(frame.encoded_len(), encoded.len());
        }
        let mut out = Vec::new();
        Frame::Error("ERR x\r\n+OK".into()).encode(&mut out);
        assert_eq!(out, b"-ERR x  +OK\r\n");
    }

    proptest! {
        #[test]
        fn bulk_commands_roundtrip(args in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..256), 1..16)) {
            let args: Vec<Bytes> = args.into_iter().map(Bytes::from).collect();
            let frame = Frame::Array(args.iter().cloned().map(Frame::Bulk).collect());
            let mut out = Vec::new();
            frame.encode(&mut out);
            prop_assert_eq!(out.len(), frame.encoded_len());
            prop_assert_eq!(decode(&out, Limits::default()), Ok(Some((args, out.len()))));
        }

        #[test]
        fn arbitrary_input_is_bounded_and_does_not_panic(input in prop::collection::vec(any::<u8>(), 0..512)) {
            let _ = decode(&input, Limits { max_frame_bytes: 128, max_bulk_bytes: 64, max_args: 8 });
        }
    }
}
