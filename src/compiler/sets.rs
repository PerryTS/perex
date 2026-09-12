//! Unicode-sets (`v`) character classes.
//!
//! The `v` grammar is not `u`'s with extra operators: it reserves punctuation,
//! requires more escaping, nests classes, and complements a set after closing
//! it under case folding rather than testing each equivalent. This module
//! implements the union grammar and that complement rule. Set operators and
//! string members remain explicitly unsupported rather than approximated.
use super::*;

/// Characters a `v` class must spell with a backslash.
const SYNTAX: [u16; 10] = [
    b'(' as u16,
    b')' as u16,
    b'[' as u16,
    b']' as u16,
    b'{' as u16,
    b'}' as u16,
    b'/' as u16,
    b'-' as u16,
    b'\\' as u16,
    b'|' as u16,
];

/// Punctuation whose doubled form the grammar reserves for future operators.
const DOUBLE: [u16; 19] = [
    b'&' as u16,
    b'!' as u16,
    b'#' as u16,
    b'$' as u16,
    b'%' as u16,
    b'*' as u16,
    b'+' as u16,
    b',' as u16,
    b'.' as u16,
    b':' as u16,
    b';' as u16,
    b'<' as u16,
    b'=' as u16,
    b'>' as u16,
    b'?' as u16,
    b'@' as u16,
    b'^' as u16,
    b'`' as u16,
    b'~' as u16,
];

/// Punctuation a backslash may escape inside a `v` class in addition to the
/// ordinary character escapes.
const RESERVED: [u16; 14] = [
    b'&' as u16,
    b'-' as u16,
    b'!' as u16,
    b'#' as u16,
    b'%' as u16,
    b',' as u16,
    b':' as u16,
    b';' as u16,
    b'<' as u16,
    b'=' as u16,
    b'>' as u16,
    b'@' as u16,
    b'`' as u16,
    b'~' as u16,
];

impl Parser<'_, '_> {
    fn unsupported(&self) -> CompileError {
        CompileError::Unsupported {
            feature: "Unicode sets",
            utf16_offset: self.cursor.position(),
        }
    }

    /// A `v` class body, with the opening bracket already consumed. Members are
    /// appended to the shared range scratch exactly as `u` classes are, so the
    /// program representation, normalization and evaluator are unchanged.
    pub(super) fn class_set(&mut self) -> Result<u32, CompileError> {
        let negative = self.eat(b'^')?;
        let start = self.range_used;
        self.class_set_union()?;
        let count = u32::try_from(self.range_used - start).map_err(|_| CompileError::SizeLimit)?;
        if count & NEGATED != 0 {
            return Err(CompileError::SizeLimit);
        }
        self.leaf(
            CLASS,
            start as u32,
            count | if negative { NEGATED } else { 0 },
        )
    }

    /// Members until the closing bracket. A nested class contributes its own
    /// members to the same union, which is what nesting means without an
    /// operator between the operands.
    fn class_set_union(&mut self) -> Result<(), CompileError> {
        let mut members = 0;
        loop {
            self.step()?;
            let Some(c) = self.peek() else {
                return Err(self.error());
            };
            if c == b']' as u16 {
                self.take()?;
                return Ok(());
            }
            // An operator would change the meaning of everything parsed so far,
            // so it is refused before any member is recorded for it.
            if self.doubled(b'-')? || self.doubled(b'&')? {
                // With nothing on its left the operator has no operand, which
                // the grammar rejects however the operator itself is handled.
                if members == 0 {
                    return Err(self.error());
                }
                return Err(self.unsupported());
            }
            self.class_set_operand()?;
            members += 1;
        }
    }

    /// Whether the next two units are both `c`, without consuming either.
    fn doubled(&mut self, c: u8) -> Result<bool, CompileError> {
        let mut ahead = self.cursor;
        if ahead.next_unit() != Some(u16::from(c)) {
            return Ok(false);
        }
        Ok(ahead.next_unit() == Some(u16::from(c)))
    }

    fn class_set_operand(&mut self) -> Result<(), CompileError> {
        // A nested class is a union operand; its own complement would need the
        // set materialized, which this implementation does not do.
        if self.eat(b'[')? {
            if self.peek() == Some(b'^' as u16) {
                return Err(self.unsupported());
            }
            return self.class_set_union();
        }
        let Some(low) = self.class_set_character()? else {
            // A class escape contributed its own members.
            return Ok(());
        };
        // A range needs an unescaped hyphen that is not the `--` operator.
        if self.peek() == Some(b'-' as u16) && !self.doubled(b'-')? {
            self.take()?;
            if self.peek() == Some(b']' as u16) {
                return Err(self.error());
            }
            let Some(high) = self.class_set_character()? else {
                return Err(self.error());
            };
            if low > high {
                return Err(self.error());
            }
            return self.range(low, high);
        }
        self.range(low, low)
    }

    /// One member. `Ok(None)` means a class escape already recorded its own
    /// members and no range endpoint is available.
    fn class_set_character(&mut self) -> Result<Option<u32>, CompileError> {
        self.step()?;
        let Some(c) = self.peek() else {
            return Err(self.error());
        };
        if c == b'\\' as u16 {
            self.take()?;
            let Some(escape) = self.take()? else {
                return Err(self.error());
            };
            if escape == b'q' as u16 {
                // `\q` is a string member only when its brace follows; alone
                // it is not a valid escape and must stay a syntax error.
                if self.peek() != Some(u16::from(b'{')) {
                    return Err(self.error());
                }
                // String members need the evaluator to match more than one
                // character; that is separate unfinished work.
                return Err(self.unsupported());
            }
            if RESERVED.contains(&escape) {
                return Ok(Some(u32::from(escape)));
            }
            if self.builtin(escape)? {
                return Ok(None);
            }
            return self.escaped_point(escape, true).map(Some);
        }
        if SYNTAX.contains(&c) {
            return Err(self.error());
        }
        if DOUBLE.contains(&c) {
            let mut ahead = self.cursor;
            ahead.next_unit();
            if ahead.next_unit() == Some(c) {
                return Err(self.error());
            }
        }
        // A `v` class is always Unicode, so a member is a whole code point.
        self.cursor
            .next_point()
            .map(Some)
            .ok_or_else(|| self.error())
    }
}
