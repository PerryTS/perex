//! The compilation tier described in `docs/compilation.md`.
//!
//! Stage 2: instruction encoding and code generation. This crate emits bytes
//! into a caller-owned buffer and never maps, protects or calls anything, which
//! is what keeps `#![forbid(unsafe_code)]`. Nothing here is reachable from a
//! search yet.
pub(crate) mod a64;
pub(crate) mod emit;
