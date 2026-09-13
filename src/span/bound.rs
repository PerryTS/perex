//! Bounded traversal of a capture across relocations of its original owner.
use super::Span;
use crate::{
    Budget,
    binding::{BoundSubject, ImmutableSubject, SubjectError},
    input::{Mark, Position},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadProgress {
    Pending,
    Complete,
}

#[derive(Debug)]
pub enum ReadError<S, E> {
    Subject(SubjectError<S>),
    InvalidSpan,
    InvalidQuantum,
    ChangedPosition,
    WorkLimit,
    Consumer(E),
    Failed,
}

/// An offset-only cursor tied to one immutable subject binding. It traverses
/// the original representation, including surrogate halves, without an index
/// or conversion buffer. Initial non-ASCII seeking is bounded too.
///
/// The host can collect, relocate, allocate or cancel between `try_fold` calls.
/// Consumers run inside the subject borrow and must not collect or mutate it.
/// A returned consumer error or an unwind invalidates this reader; partial
/// consumer effects must never be replayed by resuming it.
pub struct BoundSpan<'a, S: ImmutableSubject> {
    subject: &'a BoundSubject<S>,
    span: Span,
    mark: Mark,
    seeking: bool,
    complete: bool,
    failed: bool,
}

impl<'a, S: ImmutableSubject> BoundSpan<'a, S> {
    /// Select another span of the SAME binding, retaining the current offset
    /// when it is nearer than either endpoint. This avoids repeated scans of
    /// an original byte string when a host visits adjacent spans. Seeking is
    /// still charged and bounded by subsequent `try_fold` calls. A failed or
    /// unwound reader cannot be revived by selecting a new span.
    pub fn retarget(
        &mut self,
        span: Span,
    ) -> Result<(), ReadError<S::Error, core::convert::Infallible>> {
        if self.failed {
            return Err(ReadError::Failed);
        }
        self.failed = true;
        let (mark, position) = self
            .subject
            .with_view(|input| {
                if span.end() > input.len_utf16() {
                    return Err(ReadError::InvalidSpan);
                }
                let current = input
                    .resume_cursor(self.mark)
                    .ok_or(ReadError::ChangedPosition)?;
                let endpoint = if span.start() <= input.len_utf16() - span.start() {
                    0
                } else {
                    input.len_utf16()
                };
                let cursor = if input.seek_work(span.start()) == 1 {
                    input
                        .cursor_at(span.start())
                        .ok_or(ReadError::ChangedPosition)?
                } else if current.position().abs_diff(span.start())
                    <= endpoint.abs_diff(span.start())
                {
                    current
                } else {
                    input
                        .cursor_at(endpoint)
                        .ok_or(ReadError::ChangedPosition)?
                };
                Ok((cursor.mark(), cursor.position()))
            })
            .map_err(ReadError::Subject)??;
        self.mark = mark;
        self.span = span;
        self.seeking = position != span.start();
        self.complete = span.is_empty();
        self.failed = false;
        Ok(())
    }

    /// Constant-work setup after subject binding. ASCII and original UTF-16
    /// seek directly; other byte strings begin at the nearer endpoint.
    /// No pattern or subject pointer is retained in continuation state.
    pub fn new(
        subject: &'a BoundSubject<S>,
        span: Span,
    ) -> Result<Self, ReadError<S::Error, core::convert::Infallible>> {
        let (mark, position) = subject
            .with_view(|input| {
                if span.end() > input.len_utf16() {
                    return Err(ReadError::InvalidSpan);
                }
                let position = if span.is_empty() {
                    0
                } else if input.seek_work(span.start()) == 1 {
                    span.start()
                } else if span.start() <= input.len_utf16() - span.start() {
                    0
                } else {
                    input.len_utf16()
                };
                let cursor = input
                    .cursor_at(position)
                    .ok_or(ReadError::ChangedPosition)?;
                Ok((cursor.mark(), position))
            })
            .map_err(ReadError::Subject)??;
        Ok(Self {
            subject,
            span,
            mark,
            seeking: position != span.start(),
            complete: span.is_empty(),
            failed: false,
        })
    }

    /// [`BoundSpan::new`], seeking to the span from a position this subject
    /// already produced when that is nearer than the reader's own starting
    /// point. Seeking is still charged per unit and bounded by `try_fold`.
    ///
    /// A position from a subject of another layout, or one that is not a
    /// valid position in this subject, is refused as
    /// [`ReadError::ChangedPosition`].
    pub fn new_near(
        subject: &'a BoundSubject<S>,
        span: Span,
        near: Position,
    ) -> Result<Self, ReadError<S::Error, core::convert::Infallible>> {
        let mut reader = Self::new(subject, span)?;
        if near.layout != subject.layout {
            return Err(ReadError::ChangedPosition);
        }
        let (mark, position) = subject
            .with_view(|input| {
                let near = input
                    .resume_cursor(near.mark)
                    .ok_or(ReadError::ChangedPosition)?;
                let own = input
                    .resume_cursor(reader.mark)
                    .ok_or(ReadError::ChangedPosition)?;
                let target = span.start();
                Ok(
                    if !span.is_empty()
                        && near.position().abs_diff(target) < own.position().abs_diff(target)
                    {
                        (near.mark(), near.position())
                    } else {
                        (own.mark(), own.position())
                    },
                )
            })
            .map_err(ReadError::Subject)??;
        reader.mark = mark;
        reader.seeking = !span.is_empty() && position != span.start();
        Ok(reader)
    }

    /// Where this reader stands: the end of what it has delivered, or where
    /// its seek has reached. A later reader or search over the same subject
    /// can start from it.
    pub fn position(&self) -> Position {
        Position {
            mark: self.mark,
            layout: self.subject.layout,
        }
    }

    /// Visit at most `quantum` UTF-16 units, counting both initial seeking and
    /// delivered units against that limit and the caller's shared work budget.
    /// A Pending step can deliver no units while seeking. No allocation occurs.
    /// Complete is repeatable without reacquiring a view or charging more work.
    pub fn try_fold<E>(
        &mut self,
        quantum: usize,
        budget: &mut Budget,
        mut consume: impl FnMut(u16) -> Result<(), E>,
    ) -> Result<ReadProgress, ReadError<S::Error, E>> {
        if self.failed {
            return Err(ReadError::Failed);
        }
        if quantum == 0 {
            return Err(ReadError::InvalidQuantum);
        }
        if self.complete {
            return Ok(ReadProgress::Complete);
        }
        // Poison before entering either the owner or consumer callback. This
        // also covers unwinding, when no returned error can update the state.
        self.failed = true;
        let result = self
            .subject
            .with_view(|input| {
                let mut cursor = input
                    .resume_cursor(self.mark)
                    .ok_or(ReadError::ChangedPosition)?;
                let mut work = 0;
                while self.seeking && cursor.position() != self.span.start() {
                    if work == quantum {
                        return Ok((cursor.mark(), false, false));
                    }
                    budget.charge(1).map_err(|_| ReadError::WorkLimit)?;
                    work += 1;
                    if cursor.position() < self.span.start() {
                        cursor.next_unit().ok_or(ReadError::ChangedPosition)?;
                    } else {
                        cursor.previous_unit().ok_or(ReadError::ChangedPosition)?;
                    }
                }
                while cursor.position() < self.span.end() {
                    if work == quantum {
                        return Ok((cursor.mark(), true, false));
                    }
                    budget.charge(1).map_err(|_| ReadError::WorkLimit)?;
                    work += 1;
                    let unit = cursor.next_unit().ok_or(ReadError::ChangedPosition)?;
                    consume(unit).map_err(ReadError::Consumer)?;
                }
                Ok((cursor.mark(), true, true))
            })
            .map_err(ReadError::Subject)??;
        self.mark = result.0;
        self.seeking = !result.1;
        self.complete = result.2;
        self.failed = false;
        Ok(if self.complete {
            ReadProgress::Complete
        } else {
            ReadProgress::Pending
        })
    }
}
