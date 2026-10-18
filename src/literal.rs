//! Exact execution of bounded start-anchored literal programs. This is a
//! reading of the same immutable instructions as the evaluator, with no
//! automaton, scratch, allocation, or retained subject storage.
use crate::{Budget, executor::ExecError, input::Input, program::*, span::Span};

/// The largest run answered without yielding. Longer programs use the ordinary
/// resumable evaluator or automaton, preserving the host's polling openings.
const MAX_CHARS: usize = 32;

pub(crate) fn eligible(p: Program<'_>) -> bool {
    let count = p.instructions();
    p.capture_count() == 1
        && (5..=MAX_CHARS + 4).contains(&count)
        && p.instruction(0) == [SAVE, 0, 0]
        && p.instruction(1)[0] == START
        && p.instruction(count - 2) == [SAVE, 1, 0]
        && p.instruction(count - 1)[0] == MATCH
        && (2..count - 2).all(|pc| p.instruction(pc)[0] == CHAR)
}

/// An exact, capture-free `^` followed by literal characters. Obtained only
/// from a validated program whose descriptor was derived from its instructions.
#[derive(Clone, Copy)]
pub struct LiteralPrefix<'a> {
    program: Program<'a>,
}

impl<'a> Program<'a> {
    /// A bounded literal-prefix executor, or `None` for every other program.
    /// In particular, multiline `^`, folded characters, captures and trailing
    /// assertions remain the ordinary evaluator's or automaton's responsibility.
    #[inline]
    pub fn literal_prefix(self) -> Option<LiteralPrefix<'a>> {
        (self.words()[7] & LITERAL_PREFIX != 0).then_some(LiteralPrefix { program: self })
    }
}

impl LiteralPrefix<'_> {
    /// The complete group-zero answer the evaluator would produce at or after
    /// `start_utf16`. Reads the original subject; no view or address is retained.
    /// Work exhaustion is an error, never a no-match answer.
    #[inline]
    pub fn find(
        self,
        input: Input<'_>,
        start_utf16: usize,
        budget: &mut Budget,
    ) -> Result<Option<Span>, ExecError> {
        budget.charge(1).map_err(|_| ExecError::WorkLimit)?;
        // A counted ASCII subject needs no cursor state or decoding. Compare
        // the original bytes directly with the immutable CHAR operands; a
        // non-ASCII operand simply cannot match this storage.
        if let Some(bytes) = input.ascii_bytes() {
            if start_utf16 != 0 {
                return Ok(None);
            }
            let count = self.program.instructions() - 4;
            for index in 0..count {
                budget.charge(1).map_err(|_| ExecError::WorkLimit)?;
                if bytes.get(index).copied().map(u32::from)
                    != Some(self.program.instruction(index + 2)[1])
                {
                    return Ok(None);
                }
            }
            return Ok(Span::new(0, count));
        }
        self.find_cursor(input, start_utf16, budget)
    }

    // Keep the lossless cursor machinery outside the ASCII caller's frame.
    #[inline(never)]
    fn find_cursor(
        self,
        input: Input<'_>,
        start_utf16: usize,
        budget: &mut Budget,
    ) -> Result<Option<Span>, ExecError> {
        let unicode = self.program.words()[2] & U != 0;
        // Unicode RegExpBuiltinExec can normalize a start between a leading
        // surrogate pair's halves back to zero. No later start can reach ^.
        if start_utf16 != 0 {
            if start_utf16 != 1 || !unicode {
                return Ok(None);
            }
            let Some(mut start) = input.cursor_at(1) else {
                return Ok(None);
            };
            start.normalize_unicode_start();
            if start.position() != 0 {
                return Ok(None);
            }
        }
        let mut cursor = input.cursor();
        for pc in 2..self.program.instructions() - 2 {
            budget.charge(1).map_err(|_| ExecError::WorkLimit)?;
            let expected = self.program.instruction(pc)[1];
            let actual = if unicode {
                cursor.next_point()
            } else {
                cursor.next_unit().map(u32::from)
            };
            if actual != Some(expected) {
                return Ok(None);
            }
        }
        Ok(Span::new(0, cursor.position()))
    }
}
