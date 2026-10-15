//! CNS: MUGEN's state and expression language.
//!
//! - [`value`]: the three-typed value model (int, float, bottom) with MUGEN's
//!   promotion rules, following Ikemen GO where MUGEN is underspecified.
//! - [`lexer`] + [`parser`]: expression text → [`code::Code`] (flat stack
//!   bytecode). The parser handles every irregular trigger form
//!   (`AnimElem = 2, >= 0`, `Command = "x"`, `HitDefAttr = SC, NA`, …).
//! - [`code`]: the bytecode and its VM. Trigger values come from a
//!   [`code::Context`] the simulation implements.
//! - [`state`]: `[Statedef]` / `[State]` blocks into [`state::StateDef`]s.
//! - [`cmd`]: `.cmd` files: command definitions and the `-1` state.
//!
//! Determinism: floats are `f32` with transcendentals from `libm`, so
//! results match across platforms.

#![forbid(unsafe_code)]

pub mod cmd;
pub mod code;
pub mod fnv;
pub mod lexer;
pub mod parser;
pub mod state;
pub mod trigger;
pub mod value;

pub use code::{Code, Context, Redirect, VarKind};
pub use parser::{compile, compile_list};
pub use trigger::Trigger;
pub use value::Value;

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{msg}")]
pub struct Error {
    pub msg: String,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
