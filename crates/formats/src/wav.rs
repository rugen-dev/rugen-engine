//! Minimal RIFF WAVE reader / writer for SND subfiles: integer PCM (8, 16,
//! 24, 32 bit) and 32-bit float, mono or stereo. Anything else (ADPCM,
//! more channels, broken headers) is reported as an error so callers can
//! keep the original bytes.

use crate::{Error, Result};

const F: &str = "wav";

/// Decoded samples, interleaved, in `-1.0..=1.0`.
#[derive(Clone, Debug, PartialEq)]
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl Pcm {
    /// Samples per channel.
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Parse a WAV file. Truncated data chunks are read as far as they go
/// (MUGEN plays them that way).
pub fn parse(bytes: &[u8]) -> Result<Pcm> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(Error::fmt(F, "not a RIFF WAVE file"));
    }
    let mut at = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = le32(bytes, at + 4) as usize;
        let body = at + 8;
        let end = body.saturating_add(len).min(bytes.len());
        match id {
            b"fmt " if end - body >= 16 => {
                let mut tag = le16(bytes, body);
                let channels = le16(bytes, body + 2);
                let rate = le32(bytes, body + 4);
                let bits = le16(bytes, body + 14);
                // WAVE_FORMAT_EXTENSIBLE: the real tag opens the sub-format GUID
                if tag == 0xFFFE && end - body >= 26 {
                    tag = le16(bytes, body + 24);
                }
                fmt = Some((tag, channels, rate, bits));
            }
            b"data" => {
                data = Some(&bytes[body..end]);
                break;
            }
            _ => {}
        }
        at = body.saturating_add(len).saturating_add(len & 1);
    }
    let (tag, channels, rate, bits) = fmt.ok_or_else(|| Error::fmt(F, "no fmt chunk"))?;
    let data = data.ok_or_else(|| Error::fmt(F, "no data chunk"))?;
    if !(1..=2).contains(&channels) {
        return Err(Error::fmt(F, format!("{channels} channels")));
    }
    if rate == 0 || rate > 384_000 {
        return Err(Error::fmt(F, format!("sample rate {rate}")));
    }
    let samples: Vec<f32> = match (tag, bits) {
        (1, 8) => data.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
        (1, 16) => data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect(),
        (1, 24) => data
            .chunks_exact(3)
            .map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0)
            .collect(),
        (1, 32) => data
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0)
            .collect(),
        (3, 32) => data
            .chunks_exact(4)
            .map(|c| {
                let v = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                if v.is_finite() {
                    v.clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            })
            .collect(),
        _ => return Err(Error::fmt(F, format!("format {tag} / {bits} bits"))),
    };
    let mut samples = samples;
    samples.truncate(samples.len() - samples.len() % channels as usize);
    Ok(Pcm {
        rate,
        channels,
        samples,
    })
}

/// A 16-bit PCM WAV file of interleaved `samples`.
pub fn write_pcm16(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Resample interleaved `samples` from `from` Hz to `to` Hz with a windowed
/// sinc (Blackman, 16 zero crossings, cut-off below the lower Nyquist).
/// The output has `round(frames * to / from)` frames.
pub fn resample(samples: &[f32], channels: usize, from: u32, to: u32) -> Vec<f32> {
    let channels = channels.max(1);
    let frames = samples.len() / channels;
    if from == to || frames == 0 {
        return samples.to_vec();
    }
    let out_frames = ((frames as u64 * to as u64 + from as u64 / 2) / from as u64) as usize;
    let ratio = to as f64 / from as f64;
    // low-pass at the smaller Nyquist frequency, a little below it
    let cutoff = ratio.min(1.0) * 0.95;
    const ZEROS: f64 = 16.0;
    let half = (ZEROS / cutoff).ceil() as i64;
    let taps = (2 * half) as usize;
    // output o sits at input position o * from / to: its fractional part
    // takes `phases` distinct values, each with its own precomputed filter
    let g = gcd(from as u64, to as u64);
    let (step, mut phases) = (from as u64 / g, to as u64 / g);
    let quantised = phases > 4096;
    if quantised {
        phases = 4096;
    }
    let mut table = vec![0f32; phases as usize * taps];
    for p in 0..phases as usize {
        let frac = p as f64 / phases as f64;
        let row = &mut table[p * taps..(p + 1) * taps];
        let mut norm = 0f64;
        let mut tmp = vec![0f64; taps];
        for (k, slot) in tmp.iter_mut().enumerate() {
            // tap k reads input floor(center) - half + 1 + k
            let d = (k as i64 - half + 1) as f64 - frac;
            let x = d * cutoff;
            let sinc = if x.abs() < 1e-9 {
                1.0
            } else {
                let px = std::f64::consts::PI * x;
                px.sin() / px
            };
            let t = d / half as f64 * 0.5 + 0.5;
            let win = if (0.0..=1.0).contains(&t) {
                let a = std::f64::consts::TAU * t;
                0.42 - 0.5 * a.cos() + 0.08 * (2.0 * a).cos()
            } else {
                0.0
            };
            *slot = sinc * win;
            norm += *slot;
        }
        let norm = if norm.abs() < 1e-12 { 1.0 } else { norm };
        for (r, v) in row.iter_mut().zip(&tmp) {
            *r = (v / norm) as f32;
        }
    }
    let mut out = vec![0f32; out_frames * channels];
    for o in 0..out_frames {
        let pos = o as u64 * step;
        let (base, phase) = if quantised {
            let center = o as f64 / ratio;
            let b = center.floor();
            (
                b as i64,
                (((center - b) * phases as f64) as usize).min(phases as usize - 1),
            )
        } else {
            (
                (pos / (to as u64 / g)) as i64,
                (pos % (to as u64 / g)) as usize,
            )
        };
        let row = &table[phase * taps..(phase + 1) * taps];
        let first = base - half + 1;
        for c in 0..channels.min(2) {
            let mut acc = 0f32;
            for (k, w) in row.iter().enumerate() {
                let i = first + k as i64;
                if i >= 0 && (i as usize) < frames {
                    acc += w * samples[i as usize * channels + c];
                }
            }
            out[o * channels + c] = acc.clamp(-1.0, 1.0);
        }
    }
    out
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm16_roundtrip() {
        let s: Vec<i16> = vec![0, 1000, -1000, 32767, -32768, 5];
        let w = write_pcm16(22050, 2, &s);
        let p = parse(&w).unwrap();
        assert_eq!((p.rate, p.channels, p.frames()), (22050, 2, 3));
        assert!((p.samples[1] - 1000.0 / 32768.0).abs() < 1e-6);
        assert_eq!(p.samples[4], -1.0);
    }

    #[test]
    fn eight_bit_and_truncated() {
        let mut w = write_pcm16(11025, 1, &[0; 4]);
        // rewrite as 8-bit unsigned with a data length beyond the file
        w[34] = 8;
        w[40..44].copy_from_slice(&1000u32.to_le_bytes());
        let p = parse(&w).unwrap();
        assert_eq!(p.frames(), 8);
        assert!(parse(b"RIFF....WAVEjunk").is_err());
    }

    #[test]
    fn resample_keeps_length_and_tone() {
        let from = 22050;
        let n = 22050;
        let tone: Vec<f32> = (0..n)
            .map(|i| ((i as f64 * 1000.0 * std::f64::consts::TAU / from as f64).sin() * 0.5) as f32)
            .collect();
        let out = resample(&tone, 1, from, 24000);
        assert_eq!(out.len(), 24000);
        // the middle still is a 1 kHz sine of the same amplitude
        let peak = out[1000..23000].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 0.02, "peak {peak}");
        let i = 12000;
        let want = ((i as f64 * 1000.0 * std::f64::consts::TAU / 24000.0).sin() * 0.5) as f32;
        assert!((out[i] - want).abs() < 0.02);
    }
}
