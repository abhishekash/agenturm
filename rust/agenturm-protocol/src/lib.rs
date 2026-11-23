//! Bounded transport framing and wire types for supervised local plugins.
//!
//! A frame is a four-byte big-endian length followed by UTF-8 bytes. This
//! framing functions do not validate JSON, authorize a plugin, or acknowledge evidence.
//! A framing error poisons the connection: callers must close it and replay
//! from the last durable cursor instead of trying to resynchronize.

#![forbid(unsafe_code)]

pub mod host;
pub mod metric;
pub mod observation;

use std::fmt;
use std::io::{self, Read, Write};

/// The host-wide maximum data frame; peers may negotiate a smaller limit.
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug)]
pub enum FrameError {
    InvalidLimit,
    Empty,
    TooLarge { size: usize, max: usize },
    Truncated,
    InvalidUtf8,
    Allocation,
    Io(io::Error),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimit => write!(f, "frame limit must be positive"),
            Self::Empty => write!(f, "empty frame is forbidden"),
            Self::TooLarge { size, max } => write!(f, "frame of {size} bytes exceeds {max}"),
            Self::Truncated => write!(f, "frame ended before its declared length"),
            Self::InvalidUtf8 => write!(f, "frame is not UTF-8"),
            Self::Allocation => write!(f, "frame allocation failed"),
            Self::Io(error) => write!(f, "frame I/O failed: {error}"),
        }
    }
}

impl std::error::Error for FrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

fn effective_limit(max: usize) -> Result<usize, FrameError> {
    if max == 0 {
        Err(FrameError::InvalidLimit)
    } else {
        Ok(max.min(MAX_FRAME_BYTES))
    }
}

fn read_remaining(reader: &mut impl Read, bytes: &mut [u8]) -> Result<(), FrameError> {
    reader.read_exact(bytes).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            FrameError::Truncated
        } else {
            FrameError::Io(error)
        }
    })
}

/// Read one frame. `None` means clean EOF before any prefix byte.
///
/// The declared length is rejected before payload allocation or read. A
/// truncated prefix or payload is always an error, never a clean EOF.
pub fn read_frame(reader: &mut impl Read, max: usize) -> Result<Option<String>, FrameError> {
    let limit = effective_limit(max)?;
    let mut prefix = [0_u8; 4];
    loop {
        match reader.read(&mut prefix[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(FrameError::Io(error)),
        }
    }
    read_remaining(reader, &mut prefix[1..])?;
    let size = u32::from_be_bytes(prefix) as usize;
    if size == 0 {
        return Err(FrameError::Empty);
    }
    if size > limit {
        return Err(FrameError::TooLarge { size, max: limit });
    }
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(size)
        .map_err(|_| FrameError::Allocation)?;
    payload.resize(size, 0);
    read_remaining(reader, &mut payload)?;
    String::from_utf8(payload)
        .map(Some)
        .map_err(|_| FrameError::InvalidUtf8)
}

/// Write one frame and flush it. On any write error the connection must close.
pub fn write_frame(writer: &mut impl Write, payload: &str, max: usize) -> Result<(), FrameError> {
    let limit = effective_limit(max)?;
    let size = payload.len();
    if size == 0 {
        return Err(FrameError::Empty);
    }
    if size > limit {
        return Err(FrameError::TooLarge { size, max: limit });
    }
    let size = u32::try_from(size).map_err(|_| FrameError::TooLarge { size, max: limit })?;
    writer
        .write_all(&size.to_be_bytes())
        .map_err(FrameError::Io)?;
    writer
        .write_all(payload.as_bytes())
        .map_err(FrameError::Io)?;
    writer.flush().map_err(FrameError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip_multiple_frames_and_clean_eof() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, "{\"id\":\"α\"}", MAX_FRAME_BYTES).unwrap();
        write_frame(&mut bytes, "{}", MAX_FRAME_BYTES).unwrap();
        assert_eq!(
            bytes,
            b"\0\0\0\x0b{\"id\":\"\xCE\xB1\"}\0\0\0\x02{}".to_vec()
        );
        let mut stream = Cursor::new(bytes);
        assert_eq!(
            read_frame(&mut stream, MAX_FRAME_BYTES).unwrap(),
            Some("{\"id\":\"α\"}".into())
        );
        assert_eq!(
            read_frame(&mut stream, MAX_FRAME_BYTES).unwrap(),
            Some("{}".into())
        );
        assert_eq!(read_frame(&mut stream, MAX_FRAME_BYTES).unwrap(), None);
    }

    #[test]
    fn truncated_prefix_and_payload_are_errors() {
        assert!(matches!(
            read_frame(&mut Cursor::new([0_u8; 2]), 10),
            Err(FrameError::Truncated)
        ));
        assert!(matches!(
            read_frame(&mut Cursor::new([0, 0, 0, 3, b'a']), 10),
            Err(FrameError::Truncated)
        ));
    }

    #[test]
    fn oversized_length_is_rejected_before_payload_read() {
        let mut stream = Cursor::new([0, 0, 1, 0, b'x', b'y']);
        assert!(matches!(
            read_frame(&mut stream, 8),
            Err(FrameError::TooLarge { size: 256, max: 8 })
        ));
        assert_eq!(stream.position(), 4);
        assert!(matches!(
            write_frame(&mut Vec::new(), "123456789", 8),
            Err(FrameError::TooLarge { .. })
        ));
    }

    #[test]
    fn invalid_utf8_empty_and_zero_limit_are_errors() {
        assert!(matches!(
            read_frame(&mut Cursor::new([0, 0, 0, 1, 0xff]), 8),
            Err(FrameError::InvalidUtf8)
        ));
        assert!(matches!(
            read_frame(&mut Cursor::new([0, 0, 0, 0]), 8),
            Err(FrameError::Empty)
        ));
        assert!(matches!(
            write_frame(&mut Vec::new(), "", 8),
            Err(FrameError::Empty)
        ));
        assert!(matches!(
            read_frame(&mut Cursor::new([]), 0),
            Err(FrameError::InvalidLimit)
        ));
    }

    #[test]
    fn global_ceiling_applies_even_if_peer_requests_more() {
        let declared = (MAX_FRAME_BYTES as u32 + 1).to_be_bytes();
        assert!(matches!(
            read_frame(&mut Cursor::new(declared), usize::MAX),
            Err(FrameError::TooLarge {
                max: MAX_FRAME_BYTES,
                ..
            })
        ));
    }
}

pub mod native_review;

pub mod telemetry_summary;
