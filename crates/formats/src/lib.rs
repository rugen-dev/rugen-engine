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
pub mod sff;
pub mod snd;
pub mod text;

#[cfg(test)]
pub(crate) mod fixtures;

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
