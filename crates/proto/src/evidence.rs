//! Compact encodings of the verification evidence a client uploads after a
//! match (`POST /result`): the confirmed input log (run-length encoded:
//! people hold buttons for many frames) and periodic world checksums. The
//! server compares both players' evidence; when a match ran over TURN it
//! never saw the traffic, so this is all it has.

/// Frames between two recorded world checksums.
pub const CHECKSUM_EVERY: u32 = 60;

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn get_varint(buf: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *buf.get(*i)?;
        *i += 1;
        v |= ((b & 0x7F) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// Input log (4 bytes per frame) → runs of `count (varint), frame [4]`.
pub fn encode_inputs(log: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut frames = log.chunks_exact(4).peekable();
    while let Some(f) = frames.next() {
        let mut n = 1u64;
        while frames.peek() == Some(&f) {
            frames.next();
            n += 1;
        }
        put_varint(&mut out, n);
        out.extend_from_slice(f);
    }
    out
}

/// Inverse of [`encode_inputs`]; `None` when corrupt or longer than
/// `max_frames` (decompression bomb guard).
pub fn decode_inputs(rle: &[u8], max_frames: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rle.len() {
        let n = get_varint(rle, &mut i)? as usize;
        let f = rle.get(i..i + 4)?;
        i += 4;
        if n == 0 || out.len() / 4 + n > max_frames {
            return None;
        }
        for _ in 0..n {
            out.extend_from_slice(f);
        }
    }
    Some(out)
}

/// `(frame, checksum)` pairs → `frame u32 BE, checksum u64 BE` each.
pub fn encode_checksums(c: &[(u32, u64)]) -> Vec<u8> {
    c.iter()
        .flat_map(|(f, s)| f.to_be_bytes().into_iter().chain(s.to_be_bytes()))
        .collect()
}

pub fn decode_checksums(b: &[u8]) -> Option<Vec<(u32, u64)>> {
    if b.len() % 12 != 0 {
        return None;
    }
    Some(
        b.chunks_exact(12)
            .map(|c| {
                (
                    u32::from_be_bytes([c[0], c[1], c[2], c[3]]),
                    u64::from_be_bytes(c[4..12].try_into().unwrap_or_default()),
                )
            })
            .collect(),
    )
}

/// Do two checksum logs agree on every frame both recorded below `below`
/// (trailing frames may be unconfirmed)?
pub fn checksums_agree(a: &[(u32, u64)], b: &[(u32, u64)], below: u32) -> bool {
    a.iter()
        .filter(|(f, _)| *f < below)
        .all(|(f, x)| b.iter().find(|(g, _)| g == f).is_none_or(|(_, y)| x == y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_roundtrip_and_shrink() {
        let mut log = Vec::new();
        for f in 0..6000u32 {
            let p1 = ((f / 13) % 7) as u16;
            let p2 = ((f / 29) % 5) as u16;
            log.extend(p1.to_le_bytes());
            log.extend(p2.to_le_bytes());
        }
        let rle = encode_inputs(&log);
        assert!(rle.len() * 4 < log.len(), "{} vs {}", rle.len(), log.len());
        assert_eq!(decode_inputs(&rle, 6000).unwrap(), log);
        assert!(decode_inputs(&rle, 5999).is_none());
        assert!(decode_inputs(&rle[..rle.len() - 1], 6000).is_none());
        assert!(decode_inputs(&[0, 1, 2, 3, 4], 10).is_none());
        assert_eq!(decode_inputs(&[], 10).unwrap(), Vec::<u8>::new());
        // a huge run cannot allocate past the cap
        let mut bomb = Vec::new();
        put_varint(&mut bomb, u32::MAX as u64);
        bomb.extend([0; 4]);
        assert!(decode_inputs(&bomb, 100_000).is_none());
    }

    #[test]
    fn checksum_logs() {
        let a = vec![(60, 1), (120, 2), (180, 3)];
        let b = decode_checksums(&encode_checksums(&a)).unwrap();
        assert_eq!(a, b);
        assert!(decode_checksums(&[0; 11]).is_none());
        let c = vec![(60, 1), (120, 2), (180, 9)];
        assert!(checksums_agree(&a, &c, 180));
        assert!(!checksums_agree(&a, &c, 181));
        assert!(checksums_agree(&a, &[], 1000));
    }
}
