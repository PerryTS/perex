//! Bounded compilation into host-owned relocatable program storage.
use crate::{
    Budget,
    input::{Cursor, Input},
    program::*,
    properties,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompileError {
    Syntax {
        utf16_offset: usize,
    },
    Unsupported {
        feature: &'static str,
        utf16_offset: usize,
    },
    Nodes,
    Ranges,
    ProgramStorage {
        required_words: usize,
    },
    WorkLimit,
    SizeLimit,
    NestingLimit,
    InvalidProgram,
}

/// Caller-owned parse arena element. Internal indices contain no pointers.
#[derive(Clone, Copy, Debug, Default)]
pub struct Node {
    kind: u32,
    a: u32,
    b: u32,
    c: u32,
    flags: u32,
    first: u32,
    end: u32,
    len: u32,
    start: u32,
    reverse: bool,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Range {
    lo: u32,
    hi: u32,
}
/// A parsed pattern ready for exact-sized program storage. This owns no heap
/// allocation and retains no pattern/flag view. The caller keeps parse scratch
/// stable until emission or drop; the borrowed budget cannot be replenished
/// between preparation and emission. Emission consumes the plan.
#[must_use = "emit the prepared program or drop it to release scratch borrows"]
pub struct Prepared<'s> {
    nodes: &'s mut [Node],
    ranges: &'s mut [Range],
    budget: &'s mut Budget,
    used: usize,
    range_used: usize,
    name_count: u32,
    captures: u32,
    repeats: u32,
    flags: u32,
    root: u32,
    count: u32,
    base_size: usize,
    size: usize,
    /// Whether the selected admission condition is consumed at or after the
    /// match start. Set by `admission`; meaningless before it runs.
    forward: bool,
}

/// Properties of strings, which only `v` accepts and only unnegated. Their
/// members are sequences rather than code points.
const STRING_PROPERTIES: [&str; 7] = [
    "Basic_Emoji",
    "Emoji_Keycap_Sequence",
    "RGI_Emoji",
    "RGI_Emoji_Flag_Sequence",
    "RGI_Emoji_Modifier_Sequence",
    "RGI_Emoji_Tag_Sequence",
    "RGI_Emoji_ZWJ_Sequence",
];

const EMPTY: u32 = 32;
const SEQ: u32 = 33;
const ALT: u32 = 34;
const GROUP: u32 = 35;
const REPEAT: u32 = 36;
const WRAP: u32 = 37;
const NAME_META: u32 = 38;
const NAME_DECL: u32 = 39;
mod admission;
mod candidate;
mod classes;
mod escapes;
mod names;
mod prepared;
mod repetition;
mod sets;

struct Parser<'a, 's> {
    pattern: Input<'a>,
    cursor: Cursor<'a>,
    flags: u32,
    unicode_sets: bool,
    nodes: &'s mut [Node],
    used: usize,
    ranges: &'s mut [Range],
    range_used: usize,
    name_chars: usize,
    name_head: u32,
    name_count: u32,
    named_mode: Option<bool>,
    captures: u32,
    capture_total: Option<u32>,
    repeats: u32,
    budget: &'s mut Budget,
    depth: usize,
}
impl Parser<'_, '_> {
    fn error(&self) -> CompileError {
        CompileError::Syntax {
            utf16_offset: self.cursor.position(),
        }
    }
    fn step(&mut self) -> Result<(), CompileError> {
        self.budget.charge(1).map_err(|_| CompileError::WorkLimit)
    }
    fn peek(&self) -> Option<u16> {
        let mut c = self.cursor;
        c.next_unit()
    }
    fn take(&mut self) -> Result<Option<u16>, CompileError> {
        self.step()?;
        Ok(self.cursor.next_unit())
    }
    fn eat(&mut self, c: u8) -> Result<bool, CompileError> {
        if self.peek() == Some(u16::from(c)) {
            self.take()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn add(&mut self, mut node: Node) -> Result<u32, CompileError> {
        self.step()?;
        if node.len == 0 && node.kind < EMPTY {
            node.len = 1;
        }
        let id = u32::try_from(self.used).map_err(|_| CompileError::SizeLimit)?;
        *self.nodes.get_mut(self.used).ok_or(CompileError::Nodes)? = node;
        self.used += 1;
        Ok(id)
    }
    fn leaf(&mut self, kind: u32, a: u32, mut b: u32) -> Result<u32, CompileError> {
        let mut flags = self.flags;
        if kind == CLASS
            && let Some(count) = self.normalize_class(a as usize, (b & !NEGATED) as usize)?
        {
            b = (b & NEGATED) | count as u32;
            if count >= 8 {
                flags |= SORTED_CLASS;
            }
        }
        self.add(Node {
            kind,
            a,
            b,
            flags,
            first: self.captures,
            end: self.captures,
            ..Node::default()
        })
    }
    fn join(&mut self, kind: u32, a: u32, b: u32) -> Result<u32, CompileError> {
        let left = self.nodes[a as usize];
        let right = self.nodes[b as usize];
        let len = left
            .len
            .checked_add(right.len)
            .and_then(|n| n.checked_add(if kind == ALT { 2 } else { 0 }))
            .ok_or(CompileError::SizeLimit)?;
        self.add(Node {
            kind,
            a,
            b,
            first: left.first.min(right.first),
            end: left.end.max(right.end),
            len,
            ..Node::default()
        })
    }
    fn disjunction(&mut self) -> Result<u32, CompileError> {
        self.depth += 1;
        if self.depth > 128 {
            return Err(CompileError::NestingLimit);
        }
        let mut node = self.sequence()?;
        while self.eat(b'|')? {
            let right = self.sequence()?;
            node = self.join(ALT, node, right)?;
        }
        self.depth -= 1;
        Ok(node)
    }
    fn sequence(&mut self) -> Result<u32, CompileError> {
        let mut result = None;
        while !matches!(self.peek(), None | Some(41 | 124)) {
            let atom = self.atom()?;
            let node = self.quantifier(atom)?;
            result = Some(match result {
                None => node,
                Some(left) => self.join(SEQ, left, node)?,
            });
        }
        match result {
            Some(n) => Ok(n),
            None => self.add(Node {
                kind: EMPTY,
                first: self.captures,
                end: self.captures,
                ..Node::default()
            }),
        }
    }
    fn modifiers(&mut self) -> Result<(), CompileError> {
        let (mut add, mut remove, mut seen) = (0, 0, 0);
        let mut subtract = false;
        loop {
            let flag = match self.peek() {
                Some(105) => I,
                Some(109) => M,
                Some(115) => S,
                Some(45) if !subtract => {
                    self.take()?;
                    subtract = true;
                    continue;
                }
                Some(58) => {
                    self.take()?;
                    if seen == 0 {
                        return Err(self.error());
                    }
                    self.flags = (self.flags | add) & !remove;
                    return Ok(());
                }
                _ => return Err(self.error()),
            };
            self.take()?;
            if seen & flag != 0 {
                return Err(self.error());
            }
            seen |= flag;
            if subtract {
                remove |= flag;
            } else {
                add |= flag;
            }
        }
    }
    fn atom(&mut self) -> Result<u32, CompileError> {
        let before = self.cursor;
        let c = self.take()?.ok_or_else(|| self.error())?;
        match c {
            40 => {
                let mut kind = GROUP;
                let mut flags = 0;
                let outer_flags = self.flags;
                let first = self.captures;
                let mut capture = first;
                let mut declaration = None;
                if self.eat(b'?')? {
                    if self.eat(b':')? {
                        kind = SEQ;
                    } else if self.eat(b'=')? {
                        kind = ASSERT;
                    } else if self.eat(b'!')? {
                        kind = ASSERT;
                        flags = 1;
                    } else if self.eat(b'<')? {
                        kind = ASSERT;
                        flags = 2;
                        if self.eat(b'=')? {
                        } else if self.eat(b'!')? {
                            flags = 3;
                        } else {
                            kind = GROUP;
                            flags = 0;
                            self.named_mode = Some(true);
                            let name = self.name()?;
                            declaration = Some(self.declare_name(name, capture)?);
                        }
                    } else if matches!(self.peek(), Some(105 | 109 | 115 | 45)) {
                        self.modifiers()?;
                        kind = SEQ;
                    } else {
                        return Err(self.error());
                    }
                }
                if kind == GROUP {
                    self.captures = self
                        .captures
                        .checked_add(1)
                        .ok_or(CompileError::SizeLimit)?;
                }
                let child = self.disjunction();
                self.flags = outer_flags;
                let child = child?;
                if !self.eat(b')')? {
                    return Err(self.error());
                }
                if kind == SEQ {
                    let inner = self.nodes[child as usize];
                    return self.add(Node {
                        kind: WRAP,
                        a: child,
                        first: inner.first,
                        end: inner.end,
                        len: inner.len,
                        ..Node::default()
                    });
                }
                if kind == ASSERT {
                    capture = 0;
                }
                let len = self.nodes[child as usize]
                    .len
                    .checked_add(2)
                    .ok_or(CompileError::SizeLimit)?;
                let group = self.add(Node {
                    kind,
                    a: child,
                    b: capture,
                    flags,
                    first,
                    end: self.captures,
                    len,
                    ..Node::default()
                })?;
                if let Some(id) = declaration {
                    self.nodes[id as usize].c = group;
                }
                Ok(group)
            }
            46 => self.leaf(ANY, 0, 0),
            94 => self.leaf(START, 0, 0),
            36 => self.leaf(END, 0, 0),
            91 => self.class(),
            92 => self.escape_node(),
            42 | 43 | 63 => Err(self.error()),
            123 => {
                self.cursor = before;
                if self.braces()?.is_some() || self.flags & U != 0 {
                    return Err(self.error());
                }
                self.cursor = before;
                self.take()?;
                self.leaf(CHAR, 123, 0)
            }
            93 | 125 if self.flags & U != 0 => Err(self.error()),
            _ => {
                self.cursor = before;
                self.step()?;
                let point = if self.flags & U != 0 {
                    self.cursor.next_point().unwrap()
                } else {
                    u32::from(self.cursor.next_unit().unwrap())
                };
                self.leaf(CHAR, point, 0)
            }
        }
    }
    fn decimal(&mut self) -> Result<Option<u32>, CompileError> {
        let mut value = None;
        while let Some(c @ 48..=57) = self.peek() {
            self.take()?;
            value = Some(
                value
                    .unwrap_or(0u32)
                    .checked_mul(10)
                    .and_then(|n| n.checked_add(u32::from(c - 48)))
                    .ok_or(CompileError::SizeLimit)?,
            );
        }
        Ok(value)
    }
    fn braces(&mut self) -> Result<Option<(u32, u32, bool)>, CompileError> {
        let saved = self.cursor;
        if !self.eat(b'{')? {
            return Ok(None);
        }
        let Some(min) = self.decimal()? else {
            self.cursor = saved;
            return Ok(None);
        };
        let (max, infinite) = if self.eat(b',')? {
            match self.decimal()? {
                Some(n) => (n, false),
                None => (0, true),
            }
        } else {
            (min, false)
        };
        if !self.eat(b'}')? {
            self.cursor = saved;
            return Ok(None);
        }
        if !infinite && min > max {
            return Err(self.error());
        }
        Ok(Some((min, max, infinite)))
    }
    fn quantifier(&mut self, child: u32) -> Result<u32, CompileError> {
        let bound = match self.peek() {
            Some(42) => {
                self.take()?;
                Some((0, 0, true))
            }
            Some(43) => {
                self.take()?;
                Some((1, 0, true))
            }
            Some(63) => {
                self.take()?;
                Some((0, 1, false))
            }
            Some(123) => self.braces()?,
            _ => None,
        };
        let Some((min, max, infinite)) = bound else {
            return Ok(child);
        };
        let node = self.nodes[child as usize];
        if matches!(node.kind, START | END | WORD) {
            return Err(self.error());
        }
        if node.kind == ASSERT && (self.flags & U != 0 || node.flags & 2 != 0) {
            return Err(self.error());
        }
        let lazy = self.eat(b'?')?;
        if !lazy && self.merge_repetition(child, min, max, infinite)? {
            return Ok(child);
        }
        let id = self.repeats;
        self.repeats = self.repeats.checked_add(1).ok_or(CompileError::SizeLimit)?;
        self.add(Node {
            kind: REPEAT,
            a: child,
            b: min,
            c: max,
            flags: u32::from(infinite) | (u32::from(lazy) << 1),
            first: node.first,
            end: node.end,
            len: node.len.checked_add(4).ok_or(CompileError::SizeLimit)?,
            start: id,
            ..Node::default()
        })
    }
    fn range(&mut self, lo: u32, hi: u32) -> Result<(), CompileError> {
        self.step()?;
        if self.range_used == self.ranges.len() - self.name_chars {
            return Err(CompileError::Ranges);
        }
        *self
            .ranges
            .get_mut(self.range_used)
            .ok_or(CompileError::Ranges)? = Range { lo, hi };
        self.range_used += 1;
        Ok(())
    }
    fn builtin(&mut self, c: u16) -> Result<bool, CompileError> {
        if matches!(c, 112 | 80) && self.flags & U != 0 {
            self.property(c == 80)?;
            return Ok(true);
        }
        let ranges: &[(u32, u32)] = match c | 32 {
            100 => &[(48, 57)],
            119 if self.flags & (U | I) == U | I => &[
                (48, 57),
                (65, 90),
                (95, 95),
                (97, 122),
                (0x17f, 0x17f),
                (0x212a, 0x212a),
            ],
            119 => &[(48, 57), (65, 90), (95, 95), (97, 122)],
            115 => &[
                (9, 13),
                (32, 32),
                (160, 160),
                (0x1680, 0x1680),
                (0x2000, 0x200a),
                (0x2028, 0x2029),
                (0x202f, 0x202f),
                (0x205f, 0x205f),
                (0x3000, 0x3000),
                (0xfeff, 0xfeff),
            ],
            _ => return Ok(false),
        };
        if c & 32 != 0 {
            for &(lo, hi) in ranges {
                self.range(lo, hi)?;
            }
        } else {
            let mut start = 0;
            for &(lo, hi) in ranges {
                if start < lo {
                    self.range(start, lo - 1)?;
                }
                start = hi + 1;
            }
            self.range(
                start,
                if self.flags & U != 0 {
                    0x10ffff
                } else {
                    0xffff
                },
            )?;
        }
        Ok(true)
    }
    fn property_name(&mut self, bytes: &mut [u8; 64]) -> Result<usize, CompileError> {
        let mut length = 0;
        while let Some(c @ (48..=57 | 65..=90 | 95 | 97..=122)) = self.peek() {
            if length == bytes.len() {
                // Every accepted property/value name is <=64 ASCII bytes,
                // verified by the generator; a longer name cannot be valid.
                return Err(self.error());
            }
            self.take()?;
            bytes[length] = c as u8;
            length += 1;
        }
        Ok(length)
    }
    fn property(&mut self, negative: bool) -> Result<(), CompileError> {
        if !self.eat(b'{')? {
            return Err(self.error());
        }
        let mut name = [0; 64];
        let mut value = [0; 64];
        let name_len = self.property_name(&mut name)?;
        let value = if self.eat(b'=')? {
            let length = self.property_name(&mut value)?;
            Some(&value[..length])
        } else {
            None
        };
        let Some(id) = properties::resolve(&name[..name_len], value) else {
            // A property of strings is valid only under `v`, and only
            // unnegated. Matching one needs the evaluator to consume more than
            // one character, so report the gap rather than a false syntax
            // error; every other unknown name is still a syntax error.
            if self.unicode_sets
                && !negative
                && value.is_none()
                && STRING_PROPERTIES
                    .iter()
                    .any(|known| known.as_bytes() == &name[..name_len])
            {
                return Err(CompileError::Unsupported {
                    feature: "Unicode sets",
                    utf16_offset: self.cursor.position(),
                });
            }
            return Err(self.error());
        };
        if !self.eat(b'}')? {
            return Err(self.error());
        }
        // Under `v` the set is closed under case folding before it is
        // complemented, which `u` does not do; the encodings differ so the
        // evaluator can tell them apart.
        self.range(
            PROPERTY | id,
            match (negative, self.unicode_sets) {
                (false, _) => 0,
                (true, false) => 1,
                (true, true) => 2,
            },
        )
    }
    fn hex(&mut self, n: usize) -> Result<Option<u32>, CompileError> {
        let saved = self.cursor;
        let mut value = 0;
        for _ in 0..n {
            let Some(c) = self.take()? else {
                self.cursor = saved;
                return Ok(None);
            };
            let digit = match c {
                48..=57 => c - 48,
                65..=70 => c - 55,
                97..=102 => c - 87,
                _ => {
                    self.cursor = saved;
                    return Ok(None);
                }
            };
            value = value * 16 + u32::from(digit);
        }
        Ok(Some(value))
    }
    fn escaped_point(&mut self, c: u16, in_class: bool) -> Result<u32, CompileError> {
        let fixed_unicode_escape = c == 117 && self.peek() != Some(123);
        let point = match c {
            110 => 10,
            114 => 13,
            116 => 9,
            118 => 11,
            102 => 12,
            98 if in_class => 8,
            48..=57 => self.numeric_character(c)?,
            99 => {
                if let Some(next) = self.peek()
                    && (matches!(next, 65..=90 | 97..=122)
                        || (in_class && self.flags & U == 0 && matches!(next, 48..=57 | 95)))
                {
                    self.take()?;
                    u32::from(next & 31)
                } else if self.flags & U != 0 {
                    return Err(self.error());
                } else {
                    // Annex B's invalid control prefix consumes only the
                    // backslash. Leave c as a separate (quantifiable) atom.
                    self.cursor.previous_unit();
                    92
                }
            }
            120 | 117 => {
                if c == 117 && self.flags & U != 0 && self.eat(b'{')? {
                    let mut value = 0u32;
                    let mut digits = 0;
                    while self.peek() != Some(125) {
                        let digit = self.hex(1)?.ok_or_else(|| self.error())?;
                        value = value
                            .checked_mul(16)
                            .and_then(|n| n.checked_add(digit))
                            .ok_or_else(|| self.error())?;
                        if value > 0x10ffff {
                            return Err(self.error());
                        }
                        digits += 1;
                    }
                    if digits == 0 || !self.eat(b'}')? {
                        return Err(self.error());
                    }
                    value
                } else {
                    match self.hex(if c == 120 { 2 } else { 4 })? {
                        Some(value) => value,
                        None if self.flags & U == 0 => u32::from(c),
                        None => return Err(self.error()),
                    }
                }
            }
            107 => {
                if self.has_named_mode()? {
                    return Err(self.error());
                }
                107
            }
            _ => {
                if self.flags & U != 0
                    && !(matches!(
                        c,
                        94 | 36 | 92 | 46 | 42 | 43 | 63 | 40 | 41 | 91 | 93 | 123 | 125 | 124 | 47
                    ) || (in_class && c == 45))
                {
                    return Err(self.error());
                }
                u32::from(c)
            }
        };
        if self.flags & U != 0 && (0xd800..=0xdbff).contains(&point) && fixed_unicode_escape {
            let saved = self.cursor;
            if self.eat(b'\\')?
                && self.eat(b'u')?
                && let Some(low) = self.hex(4)?
                && (0xdc00..=0xdfff).contains(&low)
            {
                return Ok(0x10000 + ((point - 0xd800) << 10) + (low - 0xdc00));
            }
            self.cursor = saved;
        }
        Ok(point)
    }
    fn escape_node(&mut self) -> Result<u32, CompileError> {
        let c = self.take()?.ok_or_else(|| self.error())?;
        if matches!(c, 98 | 66) {
            return self.leaf(WORD, u32::from(c == 66), 0);
        }
        if c == 107 && self.has_named_mode()? {
            if !self.eat(b'<')? {
                return Err(self.error());
            }
            let name = self.name()?;
            return self.leaf(NAMED_BACKREF, name, 0);
        }
        if matches!(c, 49..=57) {
            return self.numeric_node(c);
        }
        let start = self.range_used;
        if self.builtin(c)? {
            return self.leaf(CLASS, start as u32, (self.range_used - start) as u32);
        }
        let point = self.escaped_point(c, false)?;
        self.leaf(CHAR, point, 0)
    }
    fn class_atom(&mut self) -> Result<Option<u32>, CompileError> {
        if self.eat(b'\\')? {
            let c = self.take()?.ok_or_else(|| self.error())?;
            if self.builtin(c)? {
                return Ok(None);
            }
            return Ok(Some(self.escaped_point(c, true)?));
        }
        self.step()?;
        if self.flags & U != 0 {
            self.cursor
                .next_point()
                .map(Some)
                .ok_or_else(|| self.error())
        } else {
            self.cursor
                .next_unit()
                .map(|n| Some(u32::from(n)))
                .ok_or_else(|| self.error())
        }
    }
    fn class(&mut self) -> Result<u32, CompileError> {
        if self.unicode_sets {
            return self.class_set();
        }
        let negative = self.eat(b'^')?;
        let start = self.range_used;
        while self.peek() != Some(93) {
            if self.peek().is_none() {
                return Err(self.error());
            }
            let left = self.class_atom()?;
            let dash = self.eat(b'-')?;
            if dash && self.peek() != Some(93) {
                let right = self.class_atom()?;
                match (left, right) {
                    (Some(lo), Some(hi)) => {
                        if lo > hi {
                            return Err(self.error());
                        }
                        self.range(lo, hi)?;
                    }
                    _ if self.flags & U != 0 => return Err(self.error()),
                    _ => {
                        if let Some(c) = left {
                            self.range(c, c)?;
                        }
                        if let Some(c) = right {
                            self.range(c, c)?;
                        }
                        self.range(45, 45)?;
                    }
                }
            } else {
                if let Some(c) = left {
                    self.range(c, c)?;
                }
                // A consumed trailing hyphen is literal.
                if dash {
                    self.range(45, 45)?;
                }
            }
        }
        self.take()?;
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
}

/// Compile with explicit parse/range scratch and final program storage. No
/// allocation callbacks, external engine, pattern translation or subject copying.
/// This uses the same preparation and emission as `prepare`/`Prepared::emit`.
pub fn compile<'p>(
    pattern: Input<'_>,
    flags: &str,
    nodes: &mut [Node],
    ranges: &mut [Range],
    output: &'p mut [u32],
    budget: &mut Budget,
) -> Result<Program<'p>, CompileError> {
    if let Some(first) = output.first_mut() {
        *first = 0;
    }
    prepare_with(pattern, flags, nodes, ranges, budget, |plan| {
        plan.emit(output)
    })
}

/// Parse and size a program without allocating or retaining the pattern view.
/// The returned plan borrows only caller-owned stable scratch and the budget.
/// After this call the host can end its pattern borrow, allocate exactly
/// `Prepared::required_words()` words, and emit without parsing again.
/// This is not yet a resumable parser or a moving-GC scratch owner.
///
/// The operation's budget cannot be reset while a plan remains live:
/// ```compile_fail
/// use perex::{Budget, compiler::{Node, Range, prepare}, input::Input};
/// let mut nodes = [Node::default(); 64];
/// let mut ranges = [Range::default(); 64];
/// let mut budget = Budget::new(1000);
/// let plan = prepare(Input::utf8("a"), "", &mut nodes, &mut ranges, &mut budget).unwrap();
/// budget = Budget::new(1000);
/// let mut output = vec![0; plan.required_words()];
/// let _ = plan.emit(&mut output);
/// ```
/// Scratch cannot be mutated or relocated while the plan borrows it:
/// ```compile_fail
/// use perex::{Budget, compiler::{Node, Range, prepare}, input::Input};
/// let mut nodes = [Node::default(); 64];
/// let mut ranges = [Range::default(); 64];
/// let mut budget = Budget::new(1000);
/// let plan = prepare(Input::utf8("a"), "", &mut nodes, &mut ranges, &mut budget).unwrap();
/// nodes[0] = Node::default();
/// let mut output = vec![0; plan.required_words()];
/// let _ = plan.emit(&mut output);
/// ```
pub fn prepare<'s>(
    pattern: Input<'_>,
    flags: &str,
    nodes: &'s mut [Node],
    ranges: &'s mut [Range],
    budget: &'s mut Budget,
) -> Result<Prepared<'s>, CompileError> {
    prepare_with(pattern, flags, nodes, ranges, budget, Ok)
}

// A shared final continuation lets the one-call API emit directly without
// requiring a separately returned plan object. There is one parser/emitter.
fn prepare_with<'s, T>(
    pattern: Input<'_>,
    flags: &str,
    nodes: &'s mut [Node],
    ranges: &'s mut [Range],
    budget: &'s mut Budget,
    finish: impl FnOnce(Prepared<'s>) -> Result<T, CompileError>,
) -> Result<T, CompileError> {
    let mut seen = 0u32;
    let mut bits = 0;
    for c in flags.bytes() {
        let bit = match c {
            b'd' => 1,
            b'g' => 2,
            b'i' => 4,
            b'm' => 8,
            b's' => 16,
            b'u' => 32,
            b'v' => 64,
            b'y' => 128,
            _ => return Err(CompileError::Syntax { utf16_offset: 0 }),
        };
        if seen & bit != 0 {
            return Err(CompileError::Syntax { utf16_offset: 0 });
        }
        seen |= bit;
        bits |= match c {
            b'i' => I,
            b'm' => M,
            b's' => S,
            b'u' | b'v' => U,
            b'y' => Y,
            _ => 0,
        };
    }
    if seen & 96 == 96 {
        return Err(CompileError::Syntax { utf16_offset: 0 });
    }
    let mut parser = Parser {
        pattern,
        cursor: pattern.cursor(),
        flags: bits,
        unicode_sets: seen & 64 != 0,
        nodes,
        used: 0,
        ranges,
        range_used: 0,
        name_chars: 0,
        name_head: 0,
        name_count: 0,
        named_mode: if bits & U != 0 { Some(true) } else { None },
        captures: 1,
        capture_total: None,
        repeats: 0,
        budget,
        depth: 0,
    };
    let root = parser.disjunction()?;
    if parser.peek().is_some() {
        return Err(parser.error());
    }
    parser.check_names()?;
    for node in &parser.nodes[..parser.used] {
        parser
            .budget
            .charge(1)
            .map_err(|_| CompileError::WorkLimit)?;
        if node.kind == BACKREF && node.a >= parser.captures {
            return Err(parser.error());
        }
    }
    let count = parser.nodes[root as usize]
        .len
        .checked_add(3)
        .ok_or(CompileError::SizeLimit)?;
    let base_size = HEADER
        .checked_add(
            (count as usize)
                .checked_mul(3)
                .ok_or(CompileError::SizeLimit)?,
        )
        .and_then(|n| n.checked_add(parser.range_used.checked_mul(2)?))
        .and_then(|n| n.checked_add((parser.repeats as usize).checked_mul(8)?))
        .ok_or(CompileError::SizeLimit)?;
    let size = base_size
        .checked_add(parser.names_words()?)
        .ok_or(CompileError::SizeLimit)?;
    if size > u32::MAX as usize {
        return Err(CompileError::SizeLimit);
    }
    if parser
        .captures
        .checked_add(parser.repeats)
        .and_then(|n| n.checked_mul(2))
        .is_none()
    {
        return Err(CompileError::SizeLimit);
    }
    finish(Prepared {
        nodes: parser.nodes,
        ranges: parser.ranges,
        budget: parser.budget,
        used: parser.used,
        range_used: parser.range_used,
        name_count: parser.name_count,
        captures: parser.captures,
        repeats: parser.repeats,
        flags: bits,
        root,
        count,
        base_size,
        size,
        forward: false,
    })
}
