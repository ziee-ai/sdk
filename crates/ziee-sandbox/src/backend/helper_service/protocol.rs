//! Wire protocol for the sandbox-helper named pipe.
//!
//! Length-prefixed JSON: a 4-byte little-endian `u32` byte count, then that
//! many bytes of `serde_json`. One request, one response, per connection —
//! no multiplexing needed (calls are rare: once per WSL session for VmId,
//! once at install for registration).

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Cap on a single framed message. The largest payload is an error string;
/// 64 KiB is comfortably more than any legitimate message and bounds a
/// malformed/hostile length prefix.
const MAX_FRAME: u32 = 64 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    /// Liveness probe — used by the client to distinguish "service not
    /// installed" from "service wedged".
    Ping,
    /// Resolve the WSL utility VM's VmId (the privileged `hcsdiag`/HCS call).
    ResolveVmId,
    /// Ensure the `GuestCommunicationServices` GUIDs for
    /// `port_start..port_start+count` are registered. Returns how many were
    /// newly added (callers can decide whether a `wsl --shutdown` is needed).
    EnsureRegistered { port_start: u32, count: u32 },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Pong,
    /// VmId in canonical `aabbccdd-eeff-...` form.
    VmId(String),
    /// Count of newly-registered GUIDs (0 = all were already present).
    Registered { added: u32 },
    /// Operation failed; message is human-readable (surfaced in server logs).
    Error(String),
}

/// Write a length-prefixed JSON frame.
pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(msg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame too large"))?;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame exceeds MAX_FRAME"));
    }
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

/// Read a length-prefixed JSON frame.
pub fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> io::Result<T> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame exceeds MAX_FRAME"));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // Re-encode a value to JSON so we can compare decoded == original without
    // requiring PartialEq on the protocol enums.
    fn j<T: Serialize>(v: &T) -> Vec<u8> {
        serde_json::to_vec(v).unwrap()
    }

    #[test]
    fn write_frame_emits_le_length_prefix() {
        let msg = Request::EnsureRegistered { port_start: 10001, count: 100 };
        let mut out = Vec::new();
        write_frame(&mut out, &msg).unwrap();
        let body = j(&msg);
        // 4-byte LE prefix + body, prefix == body length.
        assert_eq!(&out[..4], &(body.len() as u32).to_le_bytes());
        assert_eq!(&out[4..], &body[..]);
    }

    #[test]
    fn request_variants_round_trip() {
        for msg in [
            Request::Ping,
            Request::ResolveVmId,
            Request::EnsureRegistered { port_start: 10001, count: 100 },
        ] {
            let mut buf = Vec::new();
            write_frame(&mut buf, &msg).unwrap();
            let mut cur = Cursor::new(buf);
            let decoded: Request = read_frame(&mut cur).unwrap();
            assert_eq!(j(&decoded), j(&msg));
        }
    }

    #[test]
    fn response_variants_round_trip() {
        for msg in [
            Response::Pong,
            Response::VmId("aabbccdd-eeff-0011-2233-445566778899".to_string()),
            Response::Registered { added: 7 },
            Response::Error("boom".to_string()),
        ] {
            let mut buf = Vec::new();
            write_frame(&mut buf, &msg).unwrap();
            let mut cur = Cursor::new(buf);
            let decoded: Response = read_frame(&mut cur).unwrap();
            assert_eq!(j(&decoded), j(&msg));
        }
    }

    #[test]
    fn read_frame_rejects_oversized_length_prefix() {
        // A hostile/garbage length prefix beyond MAX_FRAME must be refused
        // BEFORE any allocation/read of the body.
        let mut framed = Vec::new();
        framed.extend_from_slice(&(MAX_FRAME + 1).to_le_bytes());
        // No body bytes — the length check must trip first.
        let mut cur = Cursor::new(framed);
        let err = read_frame::<_, Response>(&mut cur).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn read_frame_rejects_truncated_body() {
        // Valid length prefix but a short body → read_exact errors (no hang).
        let mut framed = Vec::new();
        framed.extend_from_slice(&100u32.to_le_bytes());
        framed.extend_from_slice(b"only a few bytes");
        let mut cur = Cursor::new(framed);
        assert!(read_frame::<_, Response>(&mut cur).is_err());
    }
}
