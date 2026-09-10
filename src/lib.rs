//! Perex: an ECMAScript regex engine with explicit host memory ownership.
//!
//! This crate is an initial project scaffold. The compiler, matcher, program
//! format, and embedding API are not implemented or stabilized yet.
//!
//! The implementation will expose one compiler and matcher, immutable
//! relocatable programs, caller-controlled scratch, lossless subject access,
//! and capture spans. Hosts remain responsible for object allocation and GC.
//!
//! See the repository's architecture and memory contract for the requirements.

#![no_std]
#![forbid(unsafe_code)]
