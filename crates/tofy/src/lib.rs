//! tofy: a Rust control language that emits a language-agnostic resource spec,
//! then plans and applies it.

pub use tofy_macros::main;
pub use tofy_spec as spec;

pub mod builder;
pub mod cli;
pub mod docker;
pub mod emit;
pub mod engine;
pub mod error;
pub mod lock;
pub mod outputs;
pub mod prelude;
pub mod rt;
pub mod state;

pub use error::{Error, Result};
