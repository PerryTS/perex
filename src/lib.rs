//! Perex: an ECMAScript regex engine with explicit host memory ownership.
//!
//! Lossless borrowed input cursors and UTF-16 result spans are implemented.
//! The compiler, matcher and program format are not implemented yet. The
//! embedding API is experimental and not stabilized.
//!
//! The implementation will expose one compiler and matcher, immutable
//! relocatable programs, caller-controlled scratch, lossless subject access,
//! and capture spans. Hosts remain responsible for object allocation and GC.
//!
//! See the repository's architecture and memory contract for the requirements.

#![no_std]
#![forbid(unsafe_code)]

pub mod input;
pub mod span;
