//! Parsers for MUGEN file formats: .sff .air .snd .act .fnt .def
//!
//! Pure, synchronous, no engine dependencies. Every parser takes bytes or a
//! reader and returns plain data. Community files are frequently malformed,
//! so parsers never panic on bad input: they return [`Error`] or degrade the
//! way MUGEN / Ikemen GO do.

#![forbid(unsafe_code)]

pub mod act;
pub mod air;
pub mod def;
pub mod fnt;
pub mod pack;
pub mod rgc;
pub mod sff;
pub mod snd;
pub mod text;
pub mod vfs;
pub mod wav;

#[cfg(test)]
pub(crate) mod fixtures;

static ENCODE_THREADS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Threads one content conversion spreads its sprite and sound encoding
/// over: every core unless [`set_encode_threads`] lowered it (the game
/// converts in the background and keeps cores for its own frames).
pub fn encode_threads() -> usize {
    match ENCODE_THREADS.load(std::sync::atomic::Ordering::Relaxed) {
        0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
        n => n,
    }
    .clamp(1, 16)
}

/// Cap [`encode_threads`] (0 = every core).
pub fn set_encode_threads(n: usize) {
    ENCODE_THREADS.store(n, std::sync::atomic::Ordering::Relaxed);
}

/// Error from any parser in this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{format}: {msg}")]
    Format { format: &'static str, msg: String },
    #[error("png: {0}")]
    Png(#[from] png::DecodingError),
}

impl Error {
    pub(crate) fn fmt(format: &'static str, msg: impl Into<String>) -> Self {
        Self::Format {
            format,
            msg: msg.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
