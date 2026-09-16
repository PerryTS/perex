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

    /// A `v` class body, with the opening bracket already consumed.
    ///
    /// A union of ordinary members is one class, exactly as a `u` class is, so
    /// the program representation and the evaluator are unchanged. An operator
    /// or a nested complement is built from the constructs the engine already
    /// has, because every operand matches exactly one code point: `A--B` is
    /// "not B here, then A", `A&&B` is "A here, then B", and a complement is
    /// "not this, then any code point". That is exact under `i` too — a class
    /// matches a character when some case equivalent of it is a member, which
    /// is membership of the set closed under folding, and the complement of a
    /// closed set is what `v` asks for.
    pub(super) fn class_set(&mut self) -> Result<u32, CompileError> {
        let negative = self.eat(b'^')?;
        let node = self.class_set_expression()?;
        if !negative {
            return Ok(node);
        }
        let start = self.range_used;
        self.range(0, 0x10ffff)?;
        let any = self.leaf(CLASS, start as u32, 1)?;
        let absent = self.class_set_lookahead(node, true)?;
        self.join(SEQ, absent, any)
    }

    /// An assertion that `child` does or does not match at this position. It
    /// captures nothing, so it begins and ends at the captures already seen.
    fn class_set_lookahead(&mut self, child: u32, negative: bool) -> Result<u32, CompileError> {
        let inner = self.nodes[child as usize];
        let len = inner.len.checked_add(2).ok_or(CompileError::SizeLimit)?;
        self.add(Node {
            kind: ASSERT,
            a: child,
            b: 0,
            flags: u32::from(negative),
            first: inner.first,
            end: inner.end,
            len,
            ..Node::default()
        })
    }

    /// A union, an intersection or a subtraction, up to and including the
    /// closing bracket. The grammar forbids mixing the two operators, which
    /// falls out of each chain accepting only its own.
    fn class_set_expression(&mut self) -> Result<u32, CompileError> {
        self.step()?;
        if self.peek().is_none() {
            return Err(self.error());
        }
        // An operator with nothing on its left has no operand.
        if self.doubled(b'-')? || self.doubled(b'&')? {
            return Err(self.error());
        }
        if self.peek() == Some(b']' as u16) {
            self.take()?;
            // The empty class matches nothing.
            let start = self.range_used;
            return self.leaf(CLASS, start as u32, 0);
        }
        // An operator takes operands, and a range is not one: `[a-z--[aeiou]]`
        // is a syntax error where `[[a-z]--[aeiou]]` is the set it looks like.
        let (first, ranged) = self.class_set_member()?;
        if self.doubled(b'-')? {
            if ranged {
                return Err(self.error());
            }
            return self.class_set_subtraction(first);
        }
        if self.doubled(b'&')? {
            if ranged {
                return Err(self.error());
            }
            return self.class_set_intersection(first);
        }
        self.class_set_union(first)
    }

    /// `A--B--C`: a code point in `A` that is in neither `B` nor `C`.
    fn class_set_subtraction(&mut self, left: u32) -> Result<u32, CompileError> {
        let mut node = left;
        while self.doubled(b'-')? {
            self.take()?;
            self.take()?;
            let right = self.class_set_operand_node()?;
            let absent = self.class_set_lookahead(right, true)?;
            node = self.join(SEQ, absent, node)?;
        }
        self.class_set_close(node)
    }

    /// `A&&B&&C`: a code point in every operand. Each operand but the last is
    /// asserted where the last one is matched.
    fn class_set_intersection(&mut self, left: u32) -> Result<u32, CompileError> {
        let mut node = left;
        while self.doubled(b'&')? {
            self.take()?;
            self.take()?;
            let right = self.class_set_operand_node()?;
            let present = self.class_set_lookahead(node, false)?;
            node = self.join(SEQ, present, right)?;
        }
        self.class_set_close(node)
    }

    /// Members until the closing bracket. Ordinary members accumulate into one
    /// class, as they always have; an operand that is not a plain member ends
    /// that run and joins the union as an alternative.
    fn class_set_union(&mut self, first: u32) -> Result<u32, CompileError> {
        let mut node = first;
        loop {
            self.step()?;
            let Some(c) = self.peek() else {
                return Err(self.error());
            };
            if c == b']' as u16 {
                self.take()?;
                return Ok(node);
            }
            // An operator after a union's members mixes the two grammars.
            if self.doubled(b'-')? || self.doubled(b'&')? {
                return Err(self.error());
            }
            let (next, _) = self.class_set_member()?;
            node = self.union_of(node, next)?;
        }
    }

    /// Two operands of a union. Two plain classes whose members are adjacent in
    /// the range scratch are one class, which is what an ordinary `v` class is;
    /// anything else is an alternation.
    fn union_of(&mut self, left: u32, right: u32) -> Result<u32, CompileError> {
        let (a, b) = (self.nodes[left as usize], self.nodes[right as usize]);
        if a.kind == CLASS
            && b.kind == CLASS
            && a.b & NEGATED == 0
            && b.b & NEGATED == 0
            && a.a + a.b == b.a
            && right as usize + 1 == self.used
        {
            // The later class was the last node added, so dropping it leaves
            // the arena as it was before it.
            self.used -= 1;
            let count = a.b.checked_add(b.b).ok_or(CompileError::SizeLimit)?;
            if count & NEGATED != 0 {
                return Err(CompileError::SizeLimit);
            }
            self.nodes[left as usize].b = count;
            return Ok(left);
        }
        self.join(ALT, left, right)
    }

    /// The bracket that closes an operator chain. Anything else is either the
    /// other operator, which cannot be mixed with this one, or a member, which
    /// an operator chain does not take.
    fn class_set_close(&mut self, node: u32) -> Result<u32, CompileError> {
        if self.peek() == Some(b']' as u16) {
            self.take()?;
            return Ok(node);
        }
        Err(self.error())
    }

    /// Whether the next two units are both `c`, without consuming either.
    fn doubled(&mut self, c: u8) -> Result<bool, CompileError> {
        let mut ahead = self.cursor;
        if ahead.next_unit() != Some(u16::from(c)) {
            return Ok(false);
        }
        Ok(ahead.next_unit() == Some(u16::from(c)))
    }

    /// One operand of an operator: a nested class, a class escape, a property
    /// or a single character. A range is not an operand; the grammar allows one
    /// only in a union.
    fn class_set_operand_node(&mut self) -> Result<u32, CompileError> {
        let (node, ranged) = self.class_set_member()?;
        if ranged {
            return Err(self.error());
        }
        Ok(node)
    }

    /// One element of a class body, and whether it was a range. A nested class
    /// is its own node; everything else is a class of the members it recorded.
    fn class_set_member(&mut self) -> Result<(u32, bool), CompileError> {
        self.step()?;
        if self.eat(b'[')? {
            return Ok((self.class_set()?, false));
        }
        let start = self.range_used;
        let ranged = self.class_set_range()?;
        let count = u32::try_from(self.range_used - start).map_err(|_| CompileError::SizeLimit)?;
        if count & NEGATED != 0 {
            return Err(CompileError::SizeLimit);
        }
        Ok((self.leaf(CLASS, start as u32, count)?, ranged))
    }

    /// A plain member: a character, a range, a class escape or a property.
    /// Reports whether it was a range.
    fn class_set_range(&mut self) -> Result<bool, CompileError> {
        let Some(low) = self.class_set_character()? else {
            // A class escape contributed its own members.
            return Ok(false);
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
            self.range(low, high)?;
            return Ok(true);
        }
        self.range(low, low)?;
        Ok(false)
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
