//! Unicode-sets (`v`) character classes.
//!
//! The `v` grammar is not `u`'s with extra operators: it reserves punctuation,
//! requires more escaping, nests classes, complements a set after closing it
//! under case folding rather than testing each equivalent, and admits members
//! that are not single code points. This module implements that grammar. A
//! class body is parsed into a [`Set`] — the code points it matches, the
//! strings of two or more code points it matches, and whether it matches the
//! empty string — and only becomes instructions once the operators around it
//! are done, so the operators combine members rather than code.
use super::*;
use crate::{casefold, sequences};
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

/// The end of a string list.
const NO_STRING: u32 = u32::MAX;

/// A class body's members, before they become instructions.
///
/// The grammar calls code points and strings members of one set, so an
/// operator combines both: code points positionally, because every one of them
/// consumes exactly one code point wherever it appears, and strings by
/// comparing the text they spell. A string of one code point is a code point,
/// and is recorded as one.
#[derive(Clone, Copy)]
struct Set {
    /// A node consuming exactly one member code point, or a class with no
    /// members.
    chars: u32,
    /// The string members of two or more code points, linked through each
    /// descriptor's `c` field in the order they were written.
    head: u32,
    tail: u32,
    /// The properties of strings this set draws members from, one node per
    /// length, linked through each node's `start` field until emission gives
    /// that field its address.
    tables: u32,
    table_tail: u32,
    /// The empty string is a member.
    empty: bool,
    /// The grammar's `MayContainStrings`, which decides whether `[^…]` is a
    /// syntax error. It follows the syntax rather than what survives an
    /// operator: `[^[\q{ab}]--[\q{ab}]]` is an error though the set it
    /// describes has no members at all.
    strings: bool,
}

impl Set {
    /// A set of code points and nothing else.
    fn of(chars: u32) -> Self {
        Set {
            chars,
            head: NO_STRING,
            tail: NO_STRING,
            tables: NO_STRING,
            table_tail: NO_STRING,
            empty: false,
            strings: false,
        }
    }
}

impl Parser<'_, '_> {
    /// A `v` class body, with the opening bracket already consumed.
    ///
    /// A union of ordinary members is one class, exactly as a `u` class is, so
    /// the program representation and the evaluator are unchanged. Everything
    /// else is built from the constructs the engine already has, because every
    /// code point member matches exactly one code point: `A--B` is "not B
    /// here, then A", `A&&B` is "A here, then B", and a complement is "not
    /// this, then any code point". That is exact under `i` too — a class
    /// matches a character when some case equivalent of it is a member, which
    /// is membership of the set closed under folding, and the complement of a
    /// closed set is what `v` asks for. A string member is the sequence it
    /// spells, and a set with strings is those sequences and its code points as
    /// one ordered alternation.
    pub(super) fn class_set(&mut self) -> Result<u32, CompileError> {
        let set = self.class_set_operand()?;
        self.materialize(set)
    }

    /// A property of strings written outside a class, which matches the same
    /// members as the class holding only it.
    pub(super) fn sequence_class(&mut self, set: u32) -> Result<u32, CompileError> {
        let set = self.sequence_property(set)?;
        self.materialize(set)
    }

    /// A class body as a set, so an enclosing operator combines its members
    /// instead of its instructions.
    fn class_set_operand(&mut self) -> Result<Set, CompileError> {
        let negative = self.eat(b'^')?;
        let set = self.class_set_expression()?;
        if !negative {
            return Ok(set);
        }
        // A complement is "not this, then any code point", which needs every
        // member to be one code point. The grammar refuses the rest rather
        // than asking what a complement of a set of strings would mean.
        if set.strings {
            return Err(self.error());
        }
        debug_assert!(set.head == NO_STRING && !set.empty);
        let start = self.range_used;
        self.range(0, 0x10ffff)?;
        let any = self.leaf(CLASS, start as u32, 1)?;
        let absent = self.class_set_lookahead(set.chars, true)?;
        Ok(Set::of(self.join(SEQ, absent, any)?))
    }

    /// The node that matches this set: its strings longest first, then its code
    /// points, then the empty string.
    ///
    /// That order is the grammar's. An ordered alternation tries each element
    /// before every shorter one and, when what follows the class fails, falls
    /// back to the next shortest — which is what the specification's matcher
    /// over a sorted element list does.
    fn materialize(&mut self, set: Set) -> Result<u32, CompileError> {
        let mut node = None;
        if set.empty {
            node = Some(self.add(Node {
                kind: EMPTY,
                first: self.captures,
                end: self.captures,
                ..Node::default()
            })?);
        }
        // A class with no members is still the answer when it is the only one.
        if !self.matches_nothing(set.chars)
            || (!set.empty && set.head == NO_STRING && set.tables == NO_STRING)
        {
            node = Some(match node {
                None => set.chars,
                Some(rest) => self.join(ALT, set.chars, rest)?,
            });
        }
        let mut tables = self.whole_property(set)?;
        loop {
            let string = self.shortest_string(set.head)?;
            let table = self.shortest_table(tables)?;
            let lengths = (
                (string != NO_STRING).then(|| self.nodes[string as usize].b as usize),
                (table != NO_STRING).then(|| self.table_length(self.nodes[table as usize].a)),
            );
            let alternative = match lengths {
                (None, None) => break,
                (Some(_), None) => self.string_sequence(string)?,
                (Some(left), Some(right)) if left <= right => self.string_sequence(string)?,
                _ => {
                    tables = self.unlink_table(tables, table);
                    self.reserve_filter(table)?;
                    table
                }
            };
            node = Some(match node {
                None => alternative,
                Some(rest) => self.join(ALT, alternative, rest)?,
            });
        }
        // Every set has a code point part, kept above when nothing else is
        // there, so one alternative always exists.
        node.ok_or(CompileError::InvalidProgram)
    }

    /// The shortest string not yet taken, or `NO_STRING` when none is left.
    /// Taking them in this order and building each alternation around the last
    /// leaves the longest first.
    fn shortest_string(&mut self, head: u32) -> Result<u32, CompileError> {
        let (mut best, mut current) = (NO_STRING, head);
        while current != NO_STRING {
            self.step()?;
            let node = self.nodes[current as usize];
            if node.kind == STRING && (best == NO_STRING || node.b < self.nodes[best as usize].b) {
                best = current;
            }
            current = node.c;
        }
        Ok(best)
    }

    /// One instruction for a whole property of strings, when nothing else of
    /// the class falls between its members' lengths — which is so when the
    /// class draws from one property and lists no strings of its own. The
    /// groups it replaces stay in the arena as members of nothing.
    fn whole_property(&mut self, set: Set) -> Result<u32, CompileError> {
        let head = set.tables;
        if set.head != NO_STRING || head == NO_STRING {
            return Ok(head);
        }
        let property = self.nodes[head as usize].a & 255;
        let (mut current, mut group) = (head, 0);
        while current != NO_STRING {
            self.step()?;
            let node = self.nodes[current as usize];
            if node.a != property | (group << 8) || node.c != self.nodes[head as usize].c {
                return Ok(head);
            }
            current = node.start;
            group += 1;
        }
        if group as usize != sequences::groups(property) {
            return Ok(head);
        }
        let mut current = self.nodes[head as usize].start;
        while current != NO_STRING {
            self.step()?;
            self.nodes[current as usize].len = 0;
            current = self.nodes[current as usize].start;
        }
        self.nodes[head as usize].a = property | EVERY_GROUP;
        self.nodes[head as usize].start = NO_STRING;
        Ok(head)
    }

    /// The shortest of the members a property of strings still contributes.
    fn shortest_table(&mut self, head: u32) -> Result<u32, CompileError> {
        let (mut best, mut current) = (NO_STRING, head);
        while current != NO_STRING {
            self.step()?;
            let node = self.nodes[current as usize];
            let length = self.table_length(node.a);
            if best == NO_STRING || length < self.table_length(self.nodes[best as usize].a) {
                best = current;
            }
            current = node.start;
        }
        Ok(best)
    }

    /// The longest members an instruction over a property of strings tries,
    /// which is how it orders against the other members of its class.
    fn table_length(&self, a: u32) -> usize {
        let group = if a & EVERY_GROUP == EVERY_GROUP {
            0
        } else {
            (a >> 8) as usize
        };
        sequences::length(a & 255, group)
    }

    /// The list without one of its nodes, which is now an alternative of the
    /// class rather than a member still waiting for its place.
    fn unlink_table(&mut self, head: u32, taken: u32) -> u32 {
        if head == taken {
            return self.nodes[taken as usize].start;
        }
        let mut current = head;
        while current != NO_STRING {
            if self.nodes[current as usize].start == taken {
                self.nodes[current as usize].start = self.nodes[taken as usize].start;
                break;
            }
            current = self.nodes[current as usize].start;
        }
        head
    }

    /// The characters of one string, as a sequence, taking it from the list.
    fn string_sequence(&mut self, string: u32) -> Result<u32, CompileError> {
        let descriptor = self.nodes[string as usize];
        let mut node = descriptor.a;
        for i in 1..descriptor.b {
            node = self.join(SEQ, node, descriptor.a + i)?;
        }
        // Taken: the descriptor is not a member waiting for its place any more,
        // and nothing reaches it, so the emitter passes over it.
        self.nodes[string as usize].kind = EMPTY;
        Ok(node)
    }

    /// Whether this node is a class with no members, so that an operator can
    /// leave it out of what it builds. A node no alternative reaches is never
    /// given an address, and the emitter writes nothing for it.
    fn matches_nothing(&self, node: u32) -> bool {
        let node = self.nodes[node as usize];
        node.kind == CLASS && node.b == 0
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
    fn class_set_expression(&mut self) -> Result<Set, CompileError> {
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
            return Ok(Set::of(self.leaf(CLASS, start as u32, 0)?));
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

    /// `A--B--C`: a member of `A` that is a member of neither `B` nor `C`.
    fn class_set_subtraction(&mut self, left: Set) -> Result<Set, CompileError> {
        let mut set = left;
        while self.doubled(b'-')? {
            self.take()?;
            self.take()?;
            let right = self.class_set_operand_member()?;
            let (head, tail) = self.filter_strings(set, right, false)?;
            self.filter_tables(set, right, false)?;
            self.keep_as_source(right)?;
            let chars = self.without_chars(set.chars, right.chars)?;
            set = Set {
                chars,
                head,
                tail,
                empty: set.empty && !right.empty,
                strings: set.strings,
                ..set
            };
        }
        self.class_set_close(set)
    }

    /// `A&&B&&C`: a member of every operand.
    fn class_set_intersection(&mut self, left: Set) -> Result<Set, CompileError> {
        let mut set = left;
        while self.doubled(b'&')? {
            self.take()?;
            self.take()?;
            let right = self.class_set_operand_member()?;
            let (head, tail) = self.filter_strings(set, right, true)?;
            self.filter_tables(set, right, true)?;
            self.keep_as_source(right)?;
            let chars = self.common_chars(set.chars, right.chars)?;
            set = Set {
                chars,
                head,
                tail,
                empty: set.empty && right.empty,
                strings: set.strings && right.strings,
                ..set
            };
        }
        self.class_set_close(set)
    }

    /// Members until the closing bracket.
    fn class_set_union(&mut self, first: Set) -> Result<Set, CompileError> {
        let mut set = first;
        loop {
            self.step()?;
            let Some(c) = self.peek() else {
                return Err(self.error());
            };
            if c == b']' as u16 {
                self.take()?;
                return Ok(set);
            }
            // An operator after a union's members mixes the two grammars.
            if self.doubled(b'-')? || self.doubled(b'&')? {
                return Err(self.error());
            }
            let (next, _) = self.class_set_member()?;
            set = self.union_sets(set, next)?;
        }
    }

    /// Two operands of a union: their code points, and their members' sources,
    /// whose lists join end to end.
    fn union_sets(&mut self, left: Set, right: Set) -> Result<Set, CompileError> {
        let chars = self.union_of(left.chars, right.chars)?;
        let (head, tail) = match (left.head, right.head) {
            (NO_STRING, _) => (right.head, right.tail),
            (_, NO_STRING) => (left.head, left.tail),
            _ => {
                self.nodes[left.tail as usize].c = right.head;
                (left.head, right.tail)
            }
        };
        let (tables, table_tail) = match (left.tables, right.tables) {
            (NO_STRING, _) => (right.tables, right.table_tail),
            (_, NO_STRING) => (left.tables, left.table_tail),
            _ => {
                self.nodes[left.table_tail as usize].start = right.tables;
                (left.tables, right.table_tail)
            }
        };
        Ok(Set {
            chars,
            head,
            tail,
            tables,
            table_tail,
            empty: left.empty || right.empty,
            strings: left.strings || right.strings,
        })
    }

    /// Record an operator on every property of strings `left` draws from. The
    /// members themselves stay shared: what an operator leaves is a filter of
    /// one bit per member, written into the program beside the instruction
    /// that reads them.
    fn filter_tables(&mut self, left: Set, right: Set, shared: bool) -> Result<(), CompileError> {
        let (mut current, mut step, mut chain) = (left.tables, NO_STRING, NO_STRING);
        while current != NO_STRING {
            self.step()?;
            // The groups of one property share a chain, and keep sharing one:
            // a step is added per property rather than per group, so that the
            // groups can still become one instruction.
            if step == NO_STRING || self.nodes[current as usize].c != chain {
                chain = self.nodes[current as usize].c;
                step = self.add(Node {
                    kind: FILTER,
                    a: u32::from(shared),
                    b: right.head,
                    c: chain,
                    flags: right.tables,
                    ..Node::default()
                })?;
            }
            self.nodes[current as usize].c = step;
            current = self.nodes[current as usize].start;
        }
        Ok(())
    }

    /// Reserve a filter's words in the program, now that this instruction is
    /// one of the class's alternatives and knows how many members it tries.
    fn reserve_filter(&mut self, table: u32) -> Result<(), CompileError> {
        let node = self.nodes[table as usize];
        if node.c == NO_STRING || node.b != 0 {
            return Ok(());
        }
        let property = node.a & 255;
        let members = if node.a & EVERY_GROUP == EVERY_GROUP {
            sequences::count(property)
        } else {
            let (start, end) = sequences::group(property, (node.a >> 8) as usize);
            end - start
        };
        let words = members.div_ceil(32);
        self.filter_words = self
            .filter_words
            .checked_add(words)
            .ok_or(CompileError::SizeLimit)?;
        self.nodes[table as usize].b =
            u32::try_from(self.filter_words - words + 1).map_err(|_| CompileError::SizeLimit)?;
        Ok(())
    }

    /// A property of strings as a set: its members of one code point, which are
    /// `Emoji_Presentation` without the regional indicators, and one node for
    /// each length of its longer members, so those lengths order against the
    /// other members of the class they appear in.
    fn sequence_property(&mut self, set: u32) -> Result<Set, CompileError> {
        let chars = if sequences::has_points(set) {
            let start = self.range_used;
            self.range(PROPERTY | sequences::EMOJI_PRESENTATION, 0)?;
            let members = self.leaf(CLASS, start as u32, 1)?;
            let start = self.range_used;
            self.range(PROPERTY | sequences::REGIONAL_INDICATOR, 0)?;
            let indicators = self.leaf(CLASS, start as u32, 1)?;
            let absent = self.class_set_lookahead(indicators, true)?;
            self.join(SEQ, absent, members)?
        } else {
            let start = self.range_used;
            self.leaf(CLASS, start as u32, 0)?
        };
        let (mut head, mut tail) = (NO_STRING, NO_STRING);
        for group in 0..sequences::groups(set) {
            let node = self.add(Node {
                kind: SEQSET,
                a: set | (group as u32) << 8,
                b: 0,
                c: NO_STRING,
                flags: self.flags,
                first: self.captures,
                end: self.captures,
                len: 1,
                start: NO_STRING,
                ..Node::default()
            })?;
            if head == NO_STRING {
                head = node;
            } else {
                self.nodes[tail as usize].start = node;
            }
            tail = node;
        }
        Ok(Set {
            chars,
            head: NO_STRING,
            tail: NO_STRING,
            tables: head,
            table_tail: tail,
            empty: false,
            strings: true,
        })
    }

    /// Two code point operands of a union. Two plain classes whose members are
    /// adjacent in the range scratch are one class, which is what an ordinary
    /// `v` class is; anything else is an alternation.
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

    /// The code points of `left` that are not code points of `right`. A string
    /// member of `right` removes no code point, so only its code points guard.
    fn without_chars(&mut self, left: u32, right: u32) -> Result<u32, CompileError> {
        if self.matches_nothing(right) {
            return Ok(left);
        }
        let absent = self.class_set_lookahead(right, true)?;
        self.join(SEQ, absent, left)
    }

    /// The code points in both `left` and `right`.
    fn common_chars(&mut self, left: u32, right: u32) -> Result<u32, CompileError> {
        if self.matches_nothing(left) && self.matches_nothing(right) {
            return Ok(left);
        }
        let present = self.class_set_lookahead(left, false)?;
        self.join(SEQ, present, right)
    }

    /// The strings of `left` an operator keeps: those `right` also has for an
    /// intersection, those it does not for a subtraction. A dropped string's
    /// nodes leave the program, so they are made inert rather than left to be
    /// emitted from an address no one assigned.
    fn filter_strings(
        &mut self,
        left: Set,
        right: Set,
        shared: bool,
    ) -> Result<(u32, u32), CompileError> {
        let (mut head, mut tail) = (NO_STRING, NO_STRING);
        let mut current = left.head;
        while current != NO_STRING {
            self.step()?;
            let next = self.nodes[current as usize].c;
            if self.contains_string(right, current)? == shared {
                self.nodes[current as usize].c = NO_STRING;
                if head == NO_STRING {
                    head = current;
                } else {
                    self.nodes[tail as usize].c = current;
                }
                tail = current;
            }
            current = next;
        }
        Ok((head, tail))
    }

    /// Whether `set` has this exact string as a member, whether it lists it or
    /// draws it from a property of strings. Under `i` the members are the
    /// folded strings, so equal means equal after folding.
    fn contains_string(&mut self, set: Set, string: u32) -> Result<bool, CompileError> {
        let mut current = set.head;
        while current != NO_STRING {
            self.step()?;
            let node = self.nodes[current as usize];
            if node.kind == STRING && self.same_string(current, string)? {
                return Ok(true);
            }
            current = node.c;
        }
        if set.tables == NO_STRING {
            return Ok(false);
        }
        let mut points = [0; sequences::LONGEST];
        let Some(length) = spelled(self.nodes, string, &mut points) else {
            return Ok(false);
        };
        source_has(
            self.nodes,
            self.budget,
            NO_STRING,
            set.tables,
            &points[..length],
            self.flags & I != 0,
        )
    }

    /// Whether two strings spell the same sequence of members.
    fn same_string(&mut self, left: u32, right: u32) -> Result<bool, CompileError> {
        let (left, right) = (self.nodes[left as usize], self.nodes[right as usize]);
        if left.b != right.b {
            return Ok(false);
        }
        let fold = self.flags & I != 0;
        for i in 0..left.b {
            self.step()?;
            let a = self.nodes[(left.a + i) as usize].a;
            let b = self.nodes[(right.a + i) as usize].a;
            if a != b && !(fold && casefold::equal(a, b, true)) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The bracket that closes an operator chain. Anything else is either the
    /// other operator, which cannot be mixed with this one, or a member, which
    /// an operator chain does not take.
    fn class_set_close(&mut self, set: Set) -> Result<Set, CompileError> {
        if self.peek() == Some(b']' as u16) {
            self.take()?;
            return Ok(set);
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

    /// One operand of an operator. A range is not one; the grammar allows a
    /// range only in a union.
    fn class_set_operand_member(&mut self) -> Result<Set, CompileError> {
        let (set, ranged) = self.class_set_member()?;
        if ranged {
            return Err(self.error());
        }
        Ok(set)
    }

    /// One element of a class body, and whether it was a range. A nested class
    /// is its own set; everything else is the members it recorded.
    fn class_set_member(&mut self) -> Result<(Set, bool), CompileError> {
        self.step()?;
        if self.eat(b'[')? {
            return Ok((self.class_set_operand()?, false));
        }
        let start = self.range_used;
        let mut set = Set::of(0);
        let ranged = if self.string_disjunction_ahead() {
            set = self.class_string_disjunction()?;
            false
        } else {
            self.class_set_range()?
        };
        if let Some(property) = self.pending_set.take() {
            // A property of strings is a member on its own, never a range
            // endpoint, and contributes no term to the class table.
            return Ok((self.sequence_property(property)?, false));
        }
        let count = u32::try_from(self.range_used - start).map_err(|_| CompileError::SizeLimit)?;
        if count & NEGATED != 0 {
            return Err(CompileError::SizeLimit);
        }
        set.chars = self.leaf(CLASS, start as u32, count)?;
        Ok((set, ranged))
    }

    /// Whether a string disjunction begins here. `\q` is a string disjunction
    /// only when its brace follows; alone it is not an escape at all, and has
    /// to stay a syntax error.
    fn string_disjunction_ahead(&self) -> bool {
        let mut ahead = self.cursor;
        ahead.next_unit() == Some(u16::from(b'\\'))
            && ahead.next_unit() == Some(u16::from(b'q'))
            && ahead.next_unit() == Some(u16::from(b'{'))
    }

    /// `\q{ab|c|}`: the alternatives it lists. One of a single code point is a
    /// code point of the class like any other, one of none is the empty
    /// string, and the rest are strings.
    fn class_string_disjunction(&mut self) -> Result<Set, CompileError> {
        self.take()?;
        self.take()?;
        self.take()?;
        let mut set = Set::of(0);
        loop {
            let (mut first, mut count, mut single) = (0, 0, 0);
            loop {
                self.step()?;
                match self.peek() {
                    None => return Err(self.error()),
                    Some(c) if c == u16::from(b'}') || c == u16::from(b'|') => break,
                    _ => {}
                }
                let Some(point) = self.class_set_character()? else {
                    // A class escape stands for code points, not for one
                    // character of a string.
                    return Err(self.error());
                };
                // The first code point is only a string's once a second one
                // follows it, so it is held until then.
                match count {
                    0 => single = point,
                    1 => {
                        first = self.leaf(CHAR, single, 0)?;
                        self.leaf(CHAR, point, 0)?;
                    }
                    _ => {
                        self.leaf(CHAR, point, 0)?;
                    }
                }
                count += 1;
            }
            match count {
                0 => {
                    set.empty = true;
                    set.strings = true;
                }
                1 => self.range(single, single)?,
                _ => {
                    self.push_string(&mut set, first, count)?;
                    set.strings = true;
                }
            }
            if !self.eat(b'|')? {
                break;
            }
        }
        if !self.eat(b'}')? {
            return Err(self.error());
        }
        Ok(set)
    }

    /// Record one string. Its characters are already in the arena, in order
    /// and next to each other, because nothing else was added while it was
    /// read.
    fn push_string(&mut self, set: &mut Set, first: u32, count: u32) -> Result<(), CompileError> {
        let string = self.add(Node {
            kind: STRING,
            a: first,
            b: count,
            c: NO_STRING,
            ..Node::default()
        })?;
        if set.head == NO_STRING {
            set.head = string;
        } else {
            self.nodes[set.tail as usize].c = string;
        }
        set.tail = string;
        Ok(())
    }

    /// An operator's right operand is not part of the set it leaves; what it
    /// contributes is the comparison. Its properties of strings stay in the
    /// arena for that comparison, marked as members of nothing so that the
    /// emitter writes no instruction for them.
    fn keep_as_source(&mut self, set: Set) -> Result<(), CompileError> {
        let mut current = set.tables;
        while current != NO_STRING {
            self.step()?;
            self.nodes[current as usize].len = 0;
            current = self.nodes[current as usize].start;
        }
        Ok(())
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
                // A string disjunction is a member of a class body, taken
                // before this; anywhere else — a range endpoint, a string's
                // own characters — `\q` is not an escape.
                return Err(self.error());
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

/// The code points a string member spells, when it is short enough to be a
/// member of a property of strings at all.
fn spelled(nodes: &[Node], string: u32, points: &mut [u32; sequences::LONGEST]) -> Option<usize> {
    let descriptor = nodes[string as usize];
    let length = descriptor.b as usize;
    if length > points.len() {
        return None;
    }
    for (i, point) in points[..length].iter_mut().enumerate() {
        *point = nodes[descriptor.a as usize + i].a;
    }
    Some(length)
}

/// Whether one operator's source — the strings it lists and the properties of
/// strings it draws from — has this exact member. Read while the class is
/// parsed and again while it is emitted, from the same arena.
pub(super) fn source_has(
    nodes: &[Node],
    budget: &mut Budget,
    strings: u32,
    tables: u32,
    points: &[u32],
    fold: bool,
) -> Result<bool, CompileError> {
    let mut spelling = [0; sequences::LONGEST];
    let mut current = strings;
    while current != NO_STRING {
        charge(budget)?;
        let node = nodes[current as usize];
        if let Some(length) = spelled(nodes, current, &mut spelling)
            && sequences::same(&spelling[..length], points, fold)
        {
            return Ok(true);
        }
        current = node.c;
    }
    let mut current = tables;
    while current != NO_STRING {
        charge(budget)?;
        let node = nodes[current as usize];
        let (set, group) = (node.a & 255, (node.a >> 8) as usize);
        if sequences::length(set, group) == points.len()
            && sequences::position(set, points, fold).is_some_and(|(found, _)| found == group)
            && passes(nodes, budget, node.c, points, fold)?
        {
            return Ok(true);
        }
        current = node.start;
    }
    Ok(false)
}

/// Whether a member of a property of strings survives every operator recorded
/// on the group it belongs to.
pub(super) fn passes(
    nodes: &[Node],
    budget: &mut Budget,
    chain: u32,
    points: &[u32],
    fold: bool,
) -> Result<bool, CompileError> {
    let mut step = chain;
    while step != NO_STRING {
        charge(budget)?;
        let node = nodes[step as usize];
        if source_has(nodes, budget, node.b, node.flags, points, fold)? != (node.a == 1) {
            return Ok(false);
        }
        step = node.c;
    }
    Ok(true)
}

fn charge(budget: &mut Budget) -> Result<(), CompileError> {
    budget.charge(1).map_err(|_| CompileError::WorkLimit)
}
