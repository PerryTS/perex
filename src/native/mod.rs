//! The compilation tier described in `docs/compilation.md`.
//!
//! Stage 2: instruction encoding and code generation. This crate emits bytes
//! into a caller-owned buffer and never maps, protects or calls anything: those
//! are the host's, which is what keeps `#![forbid(unsafe_code)]` here. Nothing
//! in this module is reachable from a search; a host that wants it asks for the
//! code and executes it itself.
//!
//! This is experimental and unstable, like the rest of the embedding surface.
pub mod a64;
pub mod emit;
