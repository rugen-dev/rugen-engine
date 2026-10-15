//! `Sounds` sections: one SND, each sound as an Opus stream.
//!
//! A sound is resampled (windowed sinc) to the lowest Opus rate at or above
//! its own (8 / 12 / 16 / 24 / 48 kHz: 22.05 kHz becomes 24 kHz, 44.1 kHz
//! 48 kHz) and encoded in 20 ms packets, VBR, complexity 10, at
//! [`bitrate`] (32 kbit/s per mono channel at 48 kHz, less at lower rates,
//! twice that for stereo). Decoding drops the encoder's look-ahead
//! (`preskip`) and keeps exactly `frames` samples, so a sound lasts as long
//! as before (to the sample, at its new rate) and the mixer plays it at the
//! rate its WAV header names. Sounds that are not plain PCM (or where Opus
//! would not be smaller) keep their original WAV bytes.
//!
//! Layout (little endian, offsets from the section start):
//!
//! ```text
//! header  32 bytes: magic "RSND", version u16, flags u16, snd version u16 u16,
//!         sounds u32, data offset u64, pad[8]
//! sound   40 bytes: group i32, number i32, codec u8 (0 = WAV bytes, zstd or
//!         stored per `zstd` flag; 1 = Opus), channels u8, zstd u8, pad u8,
//!         rate u32, frames u32, preskip u32, offset u64 (from data), len u32,
//!         raw len u32
//! data    WAV bytes, or Opus packets as u16 length + bytes
//! ```

use std::sync::Arc;

use indexmap::IndexMap;

use crate::snd::{Snd, Sound};
use crate::{Error, Result, wav};

const F: &str = "rgc sounds";
pub const MAGIC: &[u8; 4] = b"RSND";
pub const VERSION: u16 = 1;
const HEADER: usize = 32;
const ROW: usize = 40;

/// Bitrate (bits/s) for a stream at `rate` Hz with `channels` channels.
pub fn bitrate(rate: u32, channels: u16) -> i32 {
    let mono = match rate {
        48000 => 32_000,
        24000 => 24_000,
        16000 => 20_000,
        12000 => 16_000,
        _ => 12_000,
    };
    mono * i32::from(channels.clamp(1, 2))
}

/// The Opus rate a sound at `rate` Hz is stored at.
pub fn opus_rate(rate: u32) -> u32 {
    rugen_opus::RATES
        .iter()
        .copied()
        .find(|&r| r >= rate)
        .unwrap_or(48000)
}

/// What a section holds, for logs.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub sounds: usize,
    pub opus: usize,
    pub kept: usize,
    pub wav_bytes: usize,
    pub stored_bytes: usize,
}

struct Encoded {
    key: (i32, i32),
    codec: u8,
    channels: u8,
    zstd: bool,
    rate: u32,
    frames: u32,
    preskip: u32,
    raw_len: u32,
    data: Vec<u8>,
}

/// Opus packets (u16 length + bytes) of `pcm`, plus the pre-skip.
fn encode_opus(pcm: &wav::Pcm) -> rugen_opus::Result<(u32, u32, u32, Vec<u8>)> {
    let rate = opus_rate(pcm.rate);
    let channels = pcm.channels as usize;
    let samples = wav::resample(&pcm.samples, channels, pcm.rate, rate);
    let frames = samples.len() / channels;
    let mut enc = rugen_opus::Encoder::new(rate, pcm.channels as u8, bitrate(rate, pcm.channels))?;
    let preskip = enc.lookahead()?;
    let frame = rate as usize / 50;
    let total = (frames + preskip as usize).div_ceil(frame) * frame;
    let mut input = samples;
    input.resize(total * channels, 0.0);
    let mut out = Vec::new();
    let mut packet = vec![0u8; 4000];
    for chunk in input.chunks(frame * channels) {
        let n = enc.encode_float(chunk, frame, &mut packet)?;
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&packet[..n]);
    }
    Ok((rate, frames as u32, preskip, out))
}

fn encode_one(key: (i32, i32), bytes: &[u8]) -> Encoded {
    let raw = |bytes: &[u8]| {
        let (codec, data) = super::compress(bytes, super::level());
        Encoded {
            key,
            codec: 0,
            channels: 0,
            zstd: codec == super::Codec::Zstd,
            rate: 0,
            frames: 0,
            preskip: 0,
            raw_len: bytes.len() as u32,
            data,
        }
    };
    let Ok(pcm) = wav::parse(bytes) else {
        return raw(bytes);
    };
    if pcm.frames() == 0 {
        return raw(bytes);
    }
    match encode_opus(&pcm) {
        Ok((rate, frames, preskip, data)) => {
            // only a tiny or near-silent WAV zstd-compresses below its Opus:
            // when Opus did not even reach 4:1, compare; otherwise skip the
            // compression (zstd of every WAV at level 19 cost more than Opus)
            if data.len() * 4 >= bytes.len() {
                let kept = raw(bytes);
                if kept.data.len() <= data.len() {
                    return kept;
                }
            }
            Encoded {
                key,
                codec: 1,
                channels: pcm.channels as u8,
                zstd: false,
                rate,
                frames,
                preskip,
                raw_len: data.len() as u32,
                data,
            }
        }
        Err(e) => {
            log::debug!("sound {},{}: {e}; kept as WAV", key.0, key.1);
            raw(bytes)
        }
    }
}

/// Encode every sound of `snd` (in parallel).
pub fn encode(snd: &Snd) -> (Vec<u8>, Stats) {
    let items: Vec<_> = snd.sounds.iter().map(|(k, s)| (*k, s.bytes())).collect();
    let threads = crate::encode_threads();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut done: Vec<Option<Encoded>> = (0..items.len()).map(|_| None).collect();
    let results = std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads.min(items.len().max(1)))
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((k, b)) = items.get(i) else { break };
                        out.push((i, encode_one(*k, b)));
                    }
                    out
                })
            })
            .collect();
        hs.into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect::<Vec<_>>()
    });
    for (i, e) in results {
        done[i] = Some(e);
    }
    let mut stats = Stats {
        sounds: items.len(),
        wav_bytes: items.iter().map(|(_, b)| b.len()).sum(),
        ..Default::default()
    };
    let mut out = vec![0u8; HEADER];
    out[..4].copy_from_slice(MAGIC);
    out[4..6].copy_from_slice(&VERSION.to_le_bytes());
    out[8..10].copy_from_slice(&snd.version.0.to_le_bytes());
    out[10..12].copy_from_slice(&snd.version.1.to_le_bytes());
    out[12..16].copy_from_slice(&(items.len() as u32).to_le_bytes());
    let mut ofs = 0u64;
    let encoded: Vec<Encoded> = done.into_iter().flatten().collect();
    for e in &encoded {
        if e.codec == 1 {
            stats.opus += 1;
        } else {
            stats.kept += 1;
        }
        stats.stored_bytes += e.data.len();
        out.extend_from_slice(&e.key.0.to_le_bytes());
        out.extend_from_slice(&e.key.1.to_le_bytes());
        out.extend_from_slice(&[e.codec, e.channels, u8::from(e.zstd), 0]);
        out.extend_from_slice(&e.rate.to_le_bytes());
        out.extend_from_slice(&e.frames.to_le_bytes());
        out.extend_from_slice(&e.preskip.to_le_bytes());
        out.extend_from_slice(&ofs.to_le_bytes());
        out.extend_from_slice(&(e.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&e.raw_len.to_le_bytes());
        ofs += e.data.len() as u64;
    }
    let data_ofs = out.len() as u64;
    out[16..24].copy_from_slice(&data_ofs.to_le_bytes());
    for e in &encoded {
        out.extend_from_slice(&e.data);
    }
    (out, stats)
}

/// Decode an Opus stream (as stored) into 16-bit WAV bytes.
fn decode_opus(packets: &[u8], rate: u32, channels: u8, frames: u32, preskip: u32) -> Vec<u8> {
    let ch = channels.max(1) as usize;
    let want = frames as usize;
    let mut pcm: Vec<i16> = Vec::with_capacity((want + preskip as usize) * ch);
    if let Ok(mut dec) = rugen_opus::Decoder::new(rate, channels) {
        let mut buf = vec![0i16; rate as usize * 120 / 1000 * ch];
        let mut at = 0;
        while at + 2 <= packets.len() {
            let n = u16::from_le_bytes([packets[at], packets[at + 1]]) as usize;
            at += 2;
            let Some(p) = packets.get(at..at + n) else {
                break;
            };
            at += n;
            match dec.decode(p, &mut buf) {
                Ok(got) => pcm.extend_from_slice(&buf[..got * ch]),
                Err(_) => break,
            }
        }
    }
    let skip = (preskip as usize * ch).min(pcm.len());
    let mut out: Vec<i16> = pcm[skip..].to_vec();
    out.resize(want * ch, 0);
    wav::write_pcm16(rate, u16::from(channels.max(1)), &out)
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap_or([0; 4]))
}
fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]))
}

/// Load a section: Opus sounds decode to WAV bytes on first use.
pub fn decode(bytes: Arc<Vec<u8>>) -> Result<Snd> {
    let b: &[u8] = &bytes;
    if b.len() < HEADER || &b[..4] != MAGIC {
        return Err(Error::fmt(F, "not a sound section"));
    }
    if le16(b, 4) != VERSION {
        return Err(Error::fmt(F, format!("version {}", le16(b, 4))));
    }
    let version = (le16(b, 8), le16(b, 10));
    let n = le32(b, 12) as usize;
    let data_ofs = le64(b, 16) as usize;
    let bad = || Error::fmt(F, "truncated");
    if n > 1 << 20 || data_ofs > b.len() || HEADER + n * ROW > data_ofs {
        return Err(bad());
    }
    let mut sounds = IndexMap::with_capacity(n);
    for i in 0..n {
        let r = &b[HEADER + i * ROW..HEADER + (i + 1) * ROW];
        let key = (le32(r, 0) as i32, le32(r, 4) as i32);
        let (codec, channels, zstd) = (r[8], r[9], r[10] != 0);
        let rate = le32(r, 12);
        let frames = le32(r, 16);
        let preskip = le32(r, 20);
        let start = data_ofs.checked_add(le64(r, 24) as usize).ok_or_else(bad)?;
        let len = le32(r, 32) as usize;
        let raw_len = le32(r, 36) as usize;
        if start.checked_add(len).is_none_or(|e| e > b.len()) || raw_len as u64 > super::MAX_SECTION
        {
            return Err(bad());
        }
        let buf = bytes.clone();
        let sound = match codec {
            0 => Sound::lazy(Box::new(move || {
                let src = &buf[start..start + len];
                if zstd {
                    crate::pack::unzstd(src, raw_len).unwrap_or_default()
                } else {
                    src.to_vec()
                }
            })),
            1 => {
                if !rugen_opus::RATES.contains(&rate) || !(1..=2).contains(&channels) {
                    return Err(Error::fmt(
                        F,
                        format!("sound {},{}: bad stream", key.0, key.1),
                    ));
                }
                // at most ten minutes per sound
                if frames > rate * 600 {
                    return Err(bad());
                }
                Sound::lazy(Box::new(move || {
                    decode_opus(&buf[start..start + len], rate, channels, frames, preskip)
                }))
            }
            c => return Err(Error::fmt(F, format!("codec {c}"))),
        };
        sounds.insert(key, sound);
    }
    Ok(Snd { version, sounds })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, channels: u16, secs: f32) -> Vec<u8> {
        let n = (rate as f32 * secs) as usize;
        let mut s = Vec::new();
        for i in 0..n {
            let v =
                ((i as f64 * 440.0 * std::f64::consts::TAU / rate as f64).sin() * 12000.0) as i16;
            for _ in 0..channels {
                s.push(v);
            }
        }
        wav::write_pcm16(rate, channels, &s)
    }

    #[test]
    fn sounds_keep_their_length() {
        let mut snd = Snd {
            version: (0, 1),
            ..Default::default()
        };
        let mut src = Vec::new();
        for (i, (rate, ch, secs)) in [(22050, 1, 0.73), (44100, 2, 0.31), (11025, 1, 0.012)]
            .into_iter()
            .enumerate()
        {
            src.push((rate, ch, secs, tone(rate, ch, secs)));
            let w = &src.last().unwrap().3;
            let mut bytes = Vec::new();
            bytes.extend_from_slice(w);
            snd.sounds.insert((i as i32, 0), sound_of(bytes));
        }
        // and one that is not PCM: kept byte for byte
        let mut odd = tone(8000, 1, 0.1);
        odd[20] = 2; // ADPCM tag
        snd.sounds.insert((9, 9), sound_of(odd.clone()));
        let (bytes, st) = encode(&snd);
        assert_eq!(st.sounds, 4);
        assert_eq!(st.kept, 1);
        assert!(st.stored_bytes < st.wav_bytes / 2, "{st:?}");
        let back = decode(Arc::new(bytes)).unwrap();
        assert_eq!(back.version, (0, 1));
        assert_eq!(back.get(9, 9).unwrap(), &odd[..]);
        for (i, (rate, ch, secs, _)) in src.iter().enumerate() {
            let w = wav::parse(&back.get(i as i32, 0).unwrap()).unwrap();
            assert_eq!(w.rate, opus_rate(*rate));
            assert_eq!(w.channels, *ch);
            let want =
                ((*rate as f32 * secs) as u64 * w.rate as u64 + *rate as u64 / 2) / *rate as u64;
            assert_eq!(w.frames() as u64, want, "sound {i}: same duration");
        }
        // the tone survives: check correlation in the middle of the first sound
        let w = wav::parse(&back.get(0, 0).unwrap()).unwrap();
        let mid = w.frames() / 2;
        let mut err = 0f32;
        for k in mid..mid + 200 {
            let t = k as f32 / 24000.0;
            let want =
                ((t as f64 * 440.0 * std::f64::consts::TAU).sin() * 12000.0 / 32768.0) as f32;
            err = err.max((w.samples[k] - want).abs());
        }
        assert!(err < 0.08, "max error {err}");
    }

    fn sound_of(bytes: Vec<u8>) -> Sound {
        let mut s = Vec::new();
        s.extend_from_slice(b"ElecbyteSnd\0");
        s.extend_from_slice(&[0, 0, 1, 0]);
        s.extend_from_slice(&1u32.to_le_bytes());
        s.extend_from_slice(&24u32.to_le_bytes());
        s.extend_from_slice(&0u32.to_le_bytes());
        s.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        s.extend_from_slice(&0u32.to_le_bytes());
        s.extend_from_slice(&0u32.to_le_bytes());
        s.extend_from_slice(&bytes);
        Snd::parse(&s).unwrap().sounds.swap_remove(&(0, 0)).unwrap()
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(decode(Arc::new(b"RSND".to_vec())).is_err());
        let snd = Snd::default();
        let (mut b, _) = encode(&snd);
        b[12] = 5;
        assert!(decode(Arc::new(b)).is_err());
    }
}
