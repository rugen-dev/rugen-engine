//! Safe Opus encoder and decoder.
//!
//! Backend: [`unsafe_libopus`], the reference libopus 1.3.1 translated to
//! Rust with c2rust. It needs no C compiler, no cmake, no pkg-config and no
//! system library, so it builds the same on every release target (Windows
//! MSVC x86_64 / i686 / aarch64, Linux, macOS). Every `unsafe` call of the
//! engine's Opus use is in this file; each encoder / decoder owns its state
//! and frees it on drop.
//!
//! Only the plain single-stream API is wrapped: mono or stereo, 8 / 12 / 16
//! / 24 / 48 kHz, one packet in, one frame out.
#![allow(unsafe_code)]

use unsafe_libopus as unsafe_opus;

/// The sample rates Opus encodes and decodes natively.
pub const RATES: [u32; 5] = [8000, 12000, 16000, 24000, 48000];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub i32);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self.0 {
            unsafe_opus::OPUS_BAD_ARG => "bad argument",
            unsafe_opus::OPUS_BUFFER_TOO_SMALL => "buffer too small",
            unsafe_opus::OPUS_INTERNAL_ERROR => "internal error",
            unsafe_opus::OPUS_INVALID_PACKET => "invalid packet",
            unsafe_opus::OPUS_UNIMPLEMENTED => "unimplemented",
            unsafe_opus::OPUS_INVALID_STATE => "invalid state",
            unsafe_opus::OPUS_ALLOC_FAIL => "allocation failed",
            _ => "unknown error",
        };
        write!(f, "opus: {msg} ({})", self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn check(code: i32) -> Result<i32> {
    if code < 0 { Err(Error(code)) } else { Ok(code) }
}

fn valid(rate: u32, channels: u8) -> Result<()> {
    if RATES.contains(&rate) && (channels == 1 || channels == 2) {
        Ok(())
    } else {
        Err(Error(unsafe_opus::OPUS_BAD_ARG))
    }
}

/// An Opus encoder (application `audio`, VBR, complexity 10).
pub struct Encoder {
    st: *mut unsafe_opus::OpusEncoder,
    channels: usize,
    rate: u32,
}

// The state is plain memory owned by this value; libopus keeps no globals.
unsafe impl Send for Encoder {}

impl Encoder {
    /// `bitrate` in bits per second for the whole stream.
    pub fn new(rate: u32, channels: u8, bitrate: i32) -> Result<Self> {
        valid(rate, channels)?;
        let mut err = 0;
        // SAFETY: arguments validated above; `err` outlives the call.
        let st = unsafe {
            unsafe_opus::opus_encoder_create(
                rate as i32,
                i32::from(channels),
                unsafe_opus::OPUS_APPLICATION_AUDIO,
                &mut err,
            )
        };
        if st.is_null() || err != unsafe_opus::OPUS_OK {
            return Err(Error(if err == 0 {
                unsafe_opus::OPUS_ALLOC_FAIL
            } else {
                err
            }));
        }
        let enc = Self {
            st,
            channels: usize::from(channels),
            rate,
        };
        // SAFETY: `st` is a live encoder; integer setters take one i32.
        unsafe {
            check(unsafe_opus::opus_encoder_ctl!(
                enc.st,
                unsafe_opus::OPUS_SET_BITRATE_REQUEST,
                bitrate
            ))?;
            check(unsafe_opus::opus_encoder_ctl!(
                enc.st,
                unsafe_opus::OPUS_SET_COMPLEXITY_REQUEST,
                10
            ))?;
            check(unsafe_opus::opus_encoder_ctl!(
                enc.st,
                unsafe_opus::OPUS_SET_VBR_REQUEST,
                1
            ))?;
        }
        Ok(enc)
    }

    /// Samples per channel the decoder output lags the input by (pre-skip).
    pub fn lookahead(&self) -> Result<u32> {
        let mut v: i32 = 0;
        // SAFETY: getter writes one i32 through the pointer.
        check(unsafe {
            unsafe_opus::opus_encoder_ctl!(self.st, unsafe_opus::OPUS_GET_LOOKAHEAD_REQUEST, &mut v)
        })?;
        Ok(v.max(0) as u32)
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Encode one frame of interleaved float samples (`frame` samples per
    /// channel: 2.5 / 5 / 10 / 20 / 40 / 60 ms) into `out`; returns the
    /// packet length.
    pub fn encode_float(&mut self, pcm: &[f32], frame: usize, out: &mut [u8]) -> Result<usize> {
        if pcm.len() < frame * self.channels {
            return Err(Error(unsafe_opus::OPUS_BAD_ARG));
        }
        // SAFETY: `pcm` holds `frame * channels` samples and `out` is
        // writable for `out.len()` bytes; libopus reads / writes no more.
        let n = check(unsafe {
            unsafe_opus::opus_encode_float(
                self.st,
                pcm.as_ptr(),
                frame as i32,
                out.as_mut_ptr(),
                out.len().min(i32::MAX as usize) as i32,
            )
        })?;
        Ok(n as usize)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: created by opus_encoder_create, destroyed once.
        unsafe { unsafe_opus::opus_encoder_destroy(self.st) }
    }
}

/// An Opus decoder producing 16-bit samples at its own rate.
pub struct Decoder {
    st: *mut unsafe_opus::OpusDecoder,
    channels: usize,
}

// The state is plain memory owned by this value.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new(rate: u32, channels: u8) -> Result<Self> {
        valid(rate, channels)?;
        let mut err = 0;
        // SAFETY: arguments validated above.
        let st =
            unsafe { unsafe_opus::opus_decoder_create(rate as i32, i32::from(channels), &mut err) };
        if st.is_null() || err != unsafe_opus::OPUS_OK {
            return Err(Error(if err == 0 {
                unsafe_opus::OPUS_ALLOC_FAIL
            } else {
                err
            }));
        }
        Ok(Self {
            st,
            channels: usize::from(channels),
        })
    }

    /// Decode one packet into `out` (interleaved); returns samples per
    /// channel. `out` must hold the packet's duration (at most 120 ms).
    pub fn decode(&mut self, packet: &[u8], out: &mut [i16]) -> Result<usize> {
        let frame = out.len() / self.channels;
        if packet.is_empty() || frame == 0 {
            return Err(Error(unsafe_opus::OPUS_BAD_ARG));
        }
        // SAFETY: `packet` is readable for its length, `out` writable for
        // `frame * channels` samples; libopus checks the packet itself.
        let n = check(unsafe {
            unsafe_opus::opus_decode(
                self.st,
                packet.as_ptr(),
                packet.len().min(i32::MAX as usize) as i32,
                out.as_mut_ptr(),
                frame as i32,
                0,
            )
        })?;
        Ok(n as usize)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: created by opus_decoder_create, destroyed once.
        unsafe { unsafe_opus::opus_decoder_destroy(self.st) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_roundtrip() {
        let rate = 48000;
        let mut enc = Encoder::new(rate, 1, 32000).unwrap();
        let pre = enc.lookahead().unwrap() as usize;
        let frame = 960;
        let n = frame * 10;
        let pcm: Vec<f32> = (0..n)
            .map(|i| ((i as f64 * 440.0 * std::f64::consts::TAU / rate as f64).sin() * 0.5) as f32)
            .collect();
        let mut dec = Decoder::new(rate, 1).unwrap();
        let mut out = Vec::new();
        let mut buf = [0u8; 4000];
        let mut pcm_out = vec![0i16; frame];
        for f in pcm.chunks(frame) {
            let len = enc.encode_float(f, frame, &mut buf).unwrap();
            assert!(len > 0);
            let got = dec.decode(&buf[..len], &mut pcm_out).unwrap();
            out.extend_from_slice(&pcm_out[..got]);
        }
        assert_eq!(out.len(), n);
        // after the pre-skip the signal is the sine, within a small error
        let mut err = 0f64;
        let mut sig = 0f64;
        for i in pre..n - frame {
            let want = pcm[i - pre] as f64 * 32768.0;
            let d = out[i] as f64 - want;
            err += d * d;
            sig += want * want;
        }
        let snr = 10.0 * (sig / err).log10();
        assert!(snr > 15.0, "snr {snr:.1} dB");
        assert!(Encoder::new(44100, 1, 32000).is_err());
        assert!(Decoder::new(48000, 3).is_err());
        assert!(dec.decode(&[], &mut pcm_out).is_err());
    }
}
