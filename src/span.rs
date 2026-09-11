//! Results use UTF-16 offsets, never pointers into subject storage.

use crate::input::{Cursor, Input};

mod bound;
pub use bound::{BoundSpan, ReadError, ReadProgress};

/// A half-open interval of UTF-16 code units. A capture may bisect a surrogate
/// pair. An unset capture is `None`, distinct from `Some(Span::new(n, n)?)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Span {
    start: usize,
    end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Option<Self> {
        (start <= end).then_some(Self { start, end })
    }

    pub fn start(self) -> usize {
        self.start
    }

    pub fn end(self) -> usize {
        self.end
    }

    pub fn len(self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Borrow the exact units in this span without materializing a substring.
    /// Returns `None` when the span extends beyond the subject. The host decides
    /// whether and how to allocate any eventual language-level result string.
    pub fn units(self, input: Input<'_>) -> Option<SpanUnits<'_>> {
        if self.end > input.len_utf16() {
            return None;
        }
        Some(SpanUnits {
            cursor: input.cursor_at(self.start)?,
            remaining: self.len(),
        })
    }
}

/// A scoped, allocation-free view of a capture's code units. Like an input
/// cursor, this iterator must be released before the host relocates the subject.
#[derive(Clone, Debug)]
pub struct SpanUnits<'a> {
    cursor: Cursor<'a>,
    remaining: usize,
}

impl Iterator for SpanUnits<'_> {
    type Item = u16;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        self.cursor.next_unit()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for SpanUnits<'_> {}
impl core::iter::FusedIterator for SpanUnits<'_> {}
