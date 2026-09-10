//! Perex: an ECMAScript regex engine with explicit host memory ownership.
//!
//! An experimental compiler, ordered evaluator, lossless borrowed input cursors
//! and UTF-16 spans are implemented. Full ECMAScript coverage and efficient
//! host resumption are outstanding; the embedding API and format are unstable.
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

mod casefold;
pub mod compiler;
pub mod executor;
pub mod program;
mod properties;

/// Work allowance shared across an entire compile or search operation. It is
/// never reset when trying another start position or entering an assertion.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    remaining: usize,
}
impl Budget {
    pub fn new(work: usize) -> Self {
        Self { remaining: work }
    }
    pub fn remaining(self) -> usize {
        self.remaining
    }
    pub(crate) fn charge(&mut self, work: usize) -> Result<(), ()> {
        match self.remaining.checked_sub(work) {
            Some(n) => {
                self.remaining = n;
                Ok(())
            }
            None => {
                self.remaining = 0;
                Err(())
            }
        }
    }
}
