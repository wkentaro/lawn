//! herdr-core's framed mode: `[channel u32 LE][kind u8][len u32 LE][payload]`.

use std::io::{self, Read};

pub(crate) const DATA: u8 = 0;
pub(crate) const CREDIT: u8 = 1;
pub(crate) const CLOSE: u8 = 2;
pub(crate) const RESIZE: u8 = 3;
pub(crate) const FOCUS: u8 = 4;
pub(crate) const AUTOMATIC_INPUT: u8 = 6;

const HEADER: usize = 9;
/// The core never sends a larger payload.
pub(crate) const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

pub(crate) fn encode(channel: u32, kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(&channel.to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

pub(crate) struct Frame {
    pub(crate) channel: u32,
    pub(crate) kind: u8,
    pub(crate) payload: Vec<u8>,
}

/// Reads one frame; `Ok(None)` at a clean end of stream.
pub(crate) fn read(reader: &mut impl Read) -> io::Result<Option<Frame>> {
    let mut header = [0u8; HEADER];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let [c0, c1, c2, c3, kind, l0, l1, l2, l3] = header;
    let len = u32::from_le_bytes([l0, l1, l2, l3]) as usize;
    if len > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("core frame of {len} bytes"),
        ));
    }
    let mut payload = vec![0; len];
    reader.read_exact(&mut payload)?;
    Ok(Some(Frame {
        channel: u32::from_le_bytes([c0, c1, c2, c3]),
        kind,
        payload,
    }))
}

/// A payload in frame-sized pieces; an empty one is still one frame.
pub(crate) fn pieces(payload: &[u8]) -> impl Iterator<Item = &[u8]> {
    let empty = payload.is_empty().then_some(payload);
    payload.chunks(MAX_PAYLOAD).chain(empty)
}

pub(crate) fn resize_payload(cols: u16, rows: u16) -> [u8; 4] {
    let [c0, c1] = cols.to_le_bytes();
    let [r0, r1] = rows.to_le_bytes();
    [c0, c1, r0, r1]
}

/// Decodes a RESIZE payload into `(cols, rows)`.
pub(crate) fn parse_resize(payload: &[u8]) -> Option<(u16, u16)> {
    let [c0, c1, r0, r1] = *payload else {
        return None;
    };
    Some((u16::from_le_bytes([c0, c1]), u16::from_le_bytes([r0, r1])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_keep_empty_frames_and_split_large_ones() {
        assert_eq!(pieces(&[]).collect::<Vec<_>>(), vec![&[] as &[u8]]);
        let big = vec![0u8; MAX_PAYLOAD + 1];
        let lens: Vec<usize> = pieces(&big).map(<[u8]>::len).collect();
        assert_eq!(lens, [MAX_PAYLOAD, 1]);
    }

    #[test]
    fn frames_round_trip_back_to_back() {
        let mut bytes = encode(7, DATA, b"hi");
        bytes.extend(encode(0, CLOSE, b""));
        let mut reader = bytes.as_slice();

        let first = read(&mut reader).unwrap().unwrap();
        assert_eq!(
            (first.channel, first.kind, first.payload.as_slice()),
            (7, DATA, &b"hi"[..])
        );
        let second = read(&mut reader).unwrap().unwrap();
        assert_eq!(
            (second.channel, second.kind, second.payload.len()),
            (0, CLOSE, 0)
        );
        assert!(read(&mut reader).unwrap().is_none());
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let mut bytes = encode(1, DATA, b"");
        bytes[5..9].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read(&mut bytes.as_slice()).is_err());
    }

    #[test]
    fn resize_is_cols_then_rows_little_endian() {
        assert_eq!(resize_payload(0x0102, 0x0304), [0x02, 0x01, 0x04, 0x03]);
        assert_eq!(parse_resize(&resize_payload(80, 24)), Some((80, 24)));
        assert_eq!(parse_resize(b"abc"), None);
    }
}
