//! Versioned, relocatable programs in caller-owned u32 storage.
use crate::Budget;

pub(crate) const HEADER: usize = 7;
pub(crate) const MAGIC: u32 = 0x50525831;
pub(crate) const VERSION: u32 = 2;
pub(crate) const U: u32 = 1;
pub(crate) const M: u32 = 2;
pub(crate) const S: u32 = 4;
pub(crate) const Y: u32 = 8;
pub(crate) const I: u32 = 16;
pub(crate) const NEGATED: u32 = 1 << 31;
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
            || words[2] & !(U | M | S | Y | I) != 0
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
        if size != words.len()
            || words[3]
                .checked_add(words[6])
                .and_then(|n| n.checked_mul(2))
                .is_none()
        {
            return Err(bad);
        }
        let p = Self { words };
        for pc in 0..p.instructions() {
            budget.charge(1).map_err(|_| ProgramError::WorkLimit)?;
            let [op, a, b] = p.instruction(pc);
            let valid = match op {
                MATCH | ANY | START | END | ASSERT_END => a == 0 && b == 0,
                CHAR => a <= if p.unicode() { 0x10ffff } else { 0xffff } && b == 0,
                CLASS => a
                    .checked_add(b & !NEGATED)
                    .is_some_and(|end| end <= words[5]),
                SAVE => a < words[3] * 2 && b == 0,
                SPLIT => a < words[4] && b < words[4],
                JUMP => a < words[4] && b == 0,
                WORD => a <= 1 && b == 0,
                BACKREF => a > 0 && a < words[3] && b == 0,
                ASSERT => a < words[4] && b < 4,
                REPEAT_INIT | REPEAT_CHOICE | REPEAT_BODY | REPEAT_NEXT => a < words[6] && b == 0,
                _ => false,
            };
            if !valid || (pc + 1 == p.instructions() && !matches!(op, MATCH | JUMP)) {
                return Err(bad);
            }
        }
        for i in 0..words[5] as usize {
            budget.charge(1).map_err(|_| ProgramError::WorkLimit)?;
            let [lo, hi] = p.range(i);
            if lo > hi || hi > if p.unicode() { 0x10ffff } else { 0xffff } {
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
                || r[7] != 0
            {
                return Err(bad);
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
