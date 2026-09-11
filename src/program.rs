//! Versioned, relocatable programs in caller-owned u32 storage.
use crate::{Budget, properties};

pub(crate) const HEADER: usize = 7;
pub(crate) const MAGIC: u32 = 0x50525831;
pub(crate) const VERSION: u32 = 7;
pub(crate) const U: u32 = 1;
pub(crate) const M: u32 = 2;
pub(crate) const S: u32 = 4;
pub(crate) const Y: u32 = 8;
pub(crate) const I: u32 = 16;
pub(crate) const NAMES: u32 = 32;
pub(crate) const ADMISSION_REVERSE: u32 = 64;
pub(crate) const ADMISSION: u32 = 128;
pub(crate) const ADMISSION_MAX: usize = 32;
pub(crate) const NEGATED: u32 = 1 << 31;
// A class-table record is either (literal low, literal high), or
// (PROPERTY | shared property id, complemented). Both words are relocatable.
pub(crate) const PROPERTY: u32 = 1 << 30;
pub(crate) const MATCH: u32 = 0;
pub(crate) const CHAR: u32 = 1;
pub(crate) const ANY: u32 = 2;
pub(crate) const CLASS: u32 = 3;
pub(crate) const SAVE: u32 = 4;
pub(crate) const SPLIT: u32 = 5;
pub(crate) const JUMP: u32 = 6;
pub(crate) const START: u32 = 7;
pub(crate) const END: u32 = 8;
pub(crate) const WORD: u32 = 9;
pub(crate) const BACKREF: u32 = 10;
pub(crate) const ASSERT: u32 = 11;
pub(crate) const ASSERT_END: u32 = 12;
pub(crate) const REPEAT_INIT: u32 = 13;
pub(crate) const REPEAT_CHOICE: u32 = 14;
pub(crate) const REPEAT_BODY: u32 = 15;
pub(crate) const REPEAT_NEXT: u32 = 16;
pub(crate) const NAMED_BACKREF: u32 = 17;
pub(crate) const ATOM_REPEAT: u32 = 18;
// Lexical i/m/s choices live in instructions, never in mutable match state.
pub(crate) const CHAR_I: u32 = 19;
pub(crate) const CLASS_I: u32 = 20;
pub(crate) const ANY_S: u32 = 21;
pub(crate) const START_M: u32 = 22;
pub(crate) const END_M: u32 = 23;
pub(crate) const WORD_I: u32 = 24;
pub(crate) const BACKREF_I: u32 = 25;
pub(crate) const NAMED_BACKREF_I: u32 = 26;

pub(crate) fn consuming(op: u32) -> bool {
    matches!(op, CHAR | CHAR_I | CLASS | CLASS_I | ANY | ANY_S)
}

pub(crate) fn modified(op: u32, flags: u32) -> u32 {
    match op {
        CHAR if flags & I != 0 => CHAR_I,
        CLASS if flags & I != 0 => CLASS_I,
        WORD if flags & I != 0 => WORD_I,
        BACKREF if flags & I != 0 => BACKREF_I,
        NAMED_BACKREF if flags & I != 0 => NAMED_BACKREF_I,
        ANY if flags & S != 0 => ANY_S,
        START if flags & M != 0 => START_M,
        END if flags & M != 0 => END_M,
        _ => op,
    }
}

/// One name and its numeric capture slots, borrowed from the same program.
/// Duplicate declarations in disjoint alternatives share one entry.
#[derive(Clone, Copy, Debug)]
pub struct NamedGroup<'a> {
    words: &'a [u32],
    units: usize,
    captures: &'a [u32],
}
impl<'a> NamedGroup<'a> {
    pub fn name_units(self) -> impl DoubleEndedIterator<Item = u16> + ExactSizeIterator + 'a {
        (0..self.units).map(move |i| (self.words[i / 2] >> ((i % 2) * 16)) as u16)
    }
    pub fn capture_indices(self) -> &'a [u32] {
        self.captures
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgramError {
    Invalid,
    WorkLimit,
}

/// A validated immutable view. All persistent data is in `words`, with no
/// pointers, reference counts, source borrow, or separate owning allocation.
/// Release this borrow before moving the storage and validate its new borrow.
#[derive(Clone, Copy, Debug)]
pub struct Program<'a> {
    pub(crate) words: &'a [u32],
}

impl<'a> Program<'a> {
    /// Validate the entire format, including every opcode, target and table range.
    /// The experimental native-u32 format requires four-byte alignment; it is
    /// not a stable wire format or a cross-endian serialization format.
    pub fn from_words(words: &'a [u32], budget: &mut Budget) -> Result<Self, ProgramError> {
        let bad = ProgramError::Invalid;
        if words.len() < HEADER
            || words[0] != MAGIC
            || words[1] != VERSION
            || (words[2] & ADMISSION == 0 && words[2] & !(U | M | S | Y | I | NAMES) != 0)
            || words[3] == 0
            || words[4] == 0
        {
            return Err(bad);
        }
        let size = HEADER
            .checked_add((words[4] as usize).checked_mul(3).ok_or(bad)?)
            .ok_or(bad)?
            .checked_add((words[5] as usize).checked_mul(2).ok_or(bad)?)
            .ok_or(bad)?
            .checked_add((words[6] as usize).checked_mul(8).ok_or(bad)?)
            .ok_or(bad)?;
        if size > words.len()
            || words[3]
                .checked_add(words[6])
                .and_then(|n| n.checked_mul(2))
                .is_none()
        {
            return Err(bad);
        }
        let p = Self { words };
        if let Some((pc, _)) = p.admission()
            && (pc >= p.instructions()
                || !matches!(p.instruction(pc)[0], CHAR | CHAR_I | CLASS | CLASS_I))
        {
            return Err(bad);
        }
        if words[2] & NAMES == 0 {
            if size != words.len() {
                return Err(bad);
            }
        } else {
            let names = *words.get(size).ok_or(bad)? as usize;
            if names == 0 || names >= p.capture_count() {
                return Err(bad);
            }
            let mut next = size
                .checked_add(1)
                .and_then(|n| n.checked_add(names.checked_mul(3)?))
                .ok_or(bad)?;
            if next > words.len() {
                return Err(bad);
            }
            for i in 0..names {
                budget.charge(1).map_err(|_| ProgramError::WorkLimit)?;
                let at = size + 1 + i * 3;
                let units = words[at] as usize;
                let members = words[at + 1] as usize;
                if units == 0 || members == 0 || words[at + 2] as usize != next {
                    return Err(bad);
                }
                let packed = units.div_ceil(2);
                let end = next
                    .checked_add(packed)
                    .and_then(|n| n.checked_add(members))
                    .ok_or(bad)?;
                if end > words.len() {
                    return Err(bad);
                }
                if !units.is_multiple_of(2) && words[next + packed - 1] >> 16 != 0 {
                    return Err(bad);
                }
                let group = p.named_group(i).ok_or(bad)?;
                for (j, point) in core::char::decode_utf16(group.name_units()).enumerate() {
                    budget.charge(1).map_err(|_| ProgramError::WorkLimit)?;
                    if !properties::identifier(point.map_err(|_| bad)? as u32, j == 0) {
                        return Err(bad);
                    }
                }
                let mut previous = 0;
                for &capture in group.capture_indices() {
                    budget.charge(1).map_err(|_| ProgramError::WorkLimit)?;
                    if capture <= previous || capture >= words[3] {
                        return Err(bad);
                    }
                    previous = capture;
                }
                next = end;
            }
            if next != words.len() {
                return Err(bad);
            }
        }
        for pc in 0..p.instructions() {
            budget.charge(1).map_err(|_| ProgramError::WorkLimit)?;
            let [op, a, b] = p.instruction(pc);
            let valid = match op {
                MATCH | ANY | ANY_S | START | START_M | END | END_M | ASSERT_END => {
                    a == 0 && b == 0
                }
                CHAR | CHAR_I => a <= if p.unicode() { 0x10ffff } else { 0xffff } && b == 0,
                CLASS | CLASS_I => a
                    .checked_add(b & !NEGATED)
                    .is_some_and(|end| end <= words[5]),
                SAVE => a < words[3] * 2 && b == 0,
                SPLIT => a < words[4] && b < words[4],
                JUMP => a < words[4] && b == 0,
                WORD | WORD_I => a <= 1 && b == 0,
                BACKREF | BACKREF_I => a > 0 && a < words[3] && b == 0,
                NAMED_BACKREF | NAMED_BACKREF_I => (a as usize) < p.name_count() && b == 0,
                ASSERT => a < words[4] && b < 4,
                REPEAT_INIT | REPEAT_CHOICE | REPEAT_BODY | REPEAT_NEXT => a < words[6] && b == 0,
                ATOM_REPEAT => {
                    a < words[6]
                        && b == 0
                        && p.repeat(a as usize)[7] == 1
                        && (p.repeat(a as usize)[3] as usize).checked_sub(2) == Some(pc)
                }
                _ => false,
            };
            if !valid || (pc + 1 == p.instructions() && !matches!(op, MATCH | JUMP)) {
                return Err(bad);
            }
        }
        for i in 0..words[5] as usize {
            budget.charge(1).map_err(|_| ProgramError::WorkLimit)?;
            let [lo, hi] = p.range(i);
            let valid = if lo & PROPERTY != 0 {
                p.unicode() && properties::valid(lo & !PROPERTY) && hi <= 1
            } else {
                lo <= hi && hi <= if p.unicode() { 0x10ffff } else { 0xffff }
            };
            if !valid {
                return Err(bad);
            }
        }
        for i in 0..words[6] as usize {
            budget.charge(1).map_err(|_| ProgramError::WorkLimit)?;
            let r = p.repeat(i);
            if r[2] > 3
                || (r[2] & 1 == 0 && r[0] > r[1])
                || r[3] == 0
                || r[3] >= words[4]
                || r[4] >= words[4]
                || r[5] > r[6]
                || r[6] > words[3]
                || r[7] > 1
            {
                return Err(bad);
            }
            if r[7] == 1 {
                // The atom remains an ordinary instruction in the same
                // program. A tagged repeat may only reference this exact
                // five-instruction shape with a noncapturing consuming body.
                let entry = (r[3] as usize).checked_sub(2).ok_or(bad)?;
                if entry.checked_add(5) != Some(r[4] as usize)
                    || r[5] != r[6]
                    || p.instruction(entry) != [ATOM_REPEAT, i as u32, 0]
                    || p.instruction(entry + 1) != [REPEAT_CHOICE, i as u32, 0]
                    || p.instruction(entry + 2) != [REPEAT_BODY, i as u32, 0]
                    || !consuming(p.instruction(entry + 3)[0])
                    || p.instruction(entry + 4) != [REPEAT_NEXT, i as u32, 0]
                {
                    return Err(bad);
                }
            }
        }
        Ok(p)
    }
    pub fn words(self) -> &'a [u32] {
        self.words
    }
    pub fn capture_count(self) -> usize {
        self.words[3] as usize
    }
    pub fn register_count(self) -> usize {
        (self.words[3] as usize + self.words[6] as usize) * 2
    }
    pub fn size_bytes(self) -> usize {
        self.words.len() * 4
    }
    // The optional hint uses spare flag/header bits and points into existing
    // instructions. It adds no program words or second literal/class buffer.
    pub(crate) fn admission(self) -> Option<(usize, bool)> {
        (self.words[2] & ADMISSION != 0).then_some((
            (self.words[2] >> 8) as usize,
            self.words[2] & ADMISSION_REVERSE != 0,
        ))
    }
    pub(crate) fn literal_run(self, pc: usize) -> usize {
        let op = self.instruction(pc)[0];
        (pc..self.instructions())
            .take(ADMISSION_MAX)
            .take_while(|&at| matches!(op, CHAR | CHAR_I) && self.instruction(at)[0] == op)
            .count()
    }
    fn names_start(self) -> usize {
        HEADER + self.instructions() * 3 + self.words[5] as usize * 2 + self.words[6] as usize * 8
    }
    pub fn name_count(self) -> usize {
        if self.words[2] & NAMES == 0 {
            0
        } else {
            self.words[self.names_start()] as usize
        }
    }
    /// Names appear in first-declaration order, independently of references.
    pub fn named_groups(self) -> impl ExactSizeIterator<Item = NamedGroup<'a>> + 'a {
        (0..self.name_count()).map(move |i| self.named_group(i).unwrap())
    }
    pub fn named_group(self, index: usize) -> Option<NamedGroup<'a>> {
        if index >= self.name_count() {
            return None;
        }
        let at = self.names_start() + 1 + index * 3;
        let units = self.words[at] as usize;
        let members = self.words[at + 1] as usize;
        let start = self.words[at + 2] as usize;
        let end = start + units.div_ceil(2);
        Some(NamedGroup {
            words: &self.words[start..end],
            units,
            captures: &self.words[end..end + members],
        })
    }
    pub(crate) fn instructions(self) -> usize {
        self.words[4] as usize
    }
    pub(crate) fn unicode(self) -> bool {
        self.words[2] & U != 0
    }
    pub(crate) fn instruction(self, pc: usize) -> [u32; 3] {
        let at = HEADER + pc * 3;
        self.words[at..at + 3].try_into().unwrap()
    }
    pub(crate) fn range(self, i: usize) -> [u32; 2] {
        let at = HEADER + self.instructions() * 3 + i * 2;
        self.words[at..at + 2].try_into().unwrap()
    }
    pub(crate) fn repeat(self, i: usize) -> [u32; 8] {
        let at = HEADER + self.instructions() * 3 + self.words[5] as usize * 2 + i * 8;
        self.words[at..at + 8].try_into().unwrap()
    }
}
