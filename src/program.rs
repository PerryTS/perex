//! Versioned, relocatable programs in caller-owned u32 storage.
use crate::{Budget, properties};

pub(crate) const HEADER: usize = 10;
pub(crate) const MAGIC: u32 = 0x50525831;
pub(crate) const VERSION: u32 = 12;
pub(crate) const U: u32 = 1;
pub(crate) const M: u32 = 2;
pub(crate) const S: u32 = 4;
pub(crate) const Y: u32 = 8;
pub(crate) const I: u32 = 16;
pub(crate) const NAMES: u32 = 32;
pub(crate) const ADMISSION_REVERSE: u32 = 64;
pub(crate) const ADMISSION: u32 = 128;
pub(crate) const ADMISSION_MAX: usize = 32;
/// Most characters word 9 can claim as an unconditional leading literal.
pub(crate) const LEADING_MAX: u32 = 32;
/// Word 9's low byte holds the leading run; this bit is a separate claim that
/// the admission condition is consumed at or after the match start.
pub(crate) const ADMISSION_FORWARD: u32 = 1 << 31;
/// Word 9 bits outside the leading run and the forward claim are reserved.
pub(crate) const LEADING_MASK: u32 = 255;
/// Longest end-anchored match word 8 can bound, in UTF-16 units. The stored
/// value is one more than the length, so zero means no bound was derived.
pub(crate) const MAX_END_UNITS: u32 = 254;
/// Instructions word 9's derivation may inspect. Interior `SAVE` bookkeeping
/// consumes nothing, so a bounded number is skipped while collecting the run.
const LEADING_SCAN: usize = 128;
/// Word 9 low-byte value meaning the entry is an alternation of literal runs
/// rather than one run. Its branches are read from the instructions.
pub(crate) const LEADING_ALTERNATION: u32 = 1;
/// Word 9 low-byte flag marking the leading run as case-insensitive. On wholly
/// ASCII storage an ASCII-insensitive comparison is exact, because the
/// non-ASCII characters that fold into ASCII — U+017F and U+212A — cannot
/// appear there at all.
pub(crate) const LEADING_FOLD: u32 = 128;
/// Alternatives the entry alternation may hold. Each needs one stack slot.
pub(crate) const LEADING_BRANCHES: usize = 8;
/// Word 9 bits holding the leading atom repeat's instruction, one more than the
/// index so that zero means the claim is absent.
pub(crate) const RUN_SKIP: u32 = 0x7fff_ff00;
pub(crate) const NEGATED: u32 = 1 << 31;
// A class-table record is either (literal low, literal high), or
// (PROPERTY | shared property id, complement kind). Both words are relocatable.
// The complement kind is 0 for a plain term, 1 for `\P` under `u`, where the
// complement is tested for each case equivalent, and 2 for `\P` under `v`,
// where the set is closed under case folding before it is complemented. The
// two differ only when `i` is also set; see `docs/sets.md`.
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
pub(crate) const CLASS_SORTED: u32 = 27;
pub(crate) const CLASS_SORTED_I: u32 = 28;
// Compiler node flag only; the emitted opcode carries the sorted-class choice.
pub(crate) const SORTED_CLASS: u32 = 1 << 8;

pub(crate) fn consuming(op: u32) -> bool {
    matches!(
        op,
        CHAR | CHAR_I | CLASS | CLASS_I | CLASS_SORTED | CLASS_SORTED_I | ANY | ANY_S
    )
}

pub(crate) fn modified(op: u32, flags: u32) -> u32 {
    match op {
        CHAR if flags & I != 0 => CHAR_I,
        CLASS if flags & SORTED_CLASS != 0 => {
            if flags & I != 0 {
                CLASS_SORTED_I
            } else {
                CLASS_SORTED
            }
        }
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

/// Characters every successful match must consume first, starting at its own
/// start position. Execution begins at instruction zero, so a straight-line run
/// of `SAVE` bookkeeping and ASCII `CHAR` instructions there consumes exactly
/// those characters on every path. Anything else — a branch, a class, a repeat,
/// a folded or non-ASCII character — ends the run. ASCII bytes cannot occur
/// inside a multibyte UTF-8/WTF-8 encoding, so the result is also a byte claim.
///
/// This is derived from the instructions rather than trusted, so word 9 is
/// fully validated instead of merely structurally checked.
/// Characters every successful match must consume first, starting at its own
/// start position. Execution begins at instruction zero, so a straight-line run
/// of entry `SAVE` bookkeeping followed by ASCII `CHAR` instructions consumes
/// exactly those characters on every path. Anything else — a branch, a class, a
/// repeat, a folded or non-ASCII character — ends the run. ASCII bytes cannot
/// occur inside a multibyte UTF-8/WTF-8 encoding, so this is also a byte claim.
///
/// The word is derived from the instructions rather than trusted, so word 9 is
/// fully validated instead of merely structurally checked.
pub(crate) fn derive_leading(words: &[u32], instructions: usize) -> u32 {
    let start = leading_pc(words, instructions);
    // A run is wholly case-sensitive or wholly folded; the first instruction
    // decides which, and a change of kind ends it like any other opcode.
    let kind = if start < instructions {
        words[HEADER + start * 3]
    } else {
        MATCH
    };
    let mut pc = start;
    let mut characters = 0;
    while pc < instructions && characters < LEADING_MAX {
        let at = HEADER + pc * 3;
        if words[at] != kind || words[at + 1] >= 128 {
            break;
        }
        characters += 1;
        pc += 1;
    }
    // One leading character is already the word 7 descriptor's claim.
    if characters >= 2 && kind == CHAR {
        return characters;
    }
    if characters >= 2 && kind == CHAR_I {
        return characters | LEADING_FOLD;
    }
    if derive_alternation(words, instructions) {
        return LEADING_ALTERNATION;
    }
    0
}

/// Whether the entry is a branch whose every alternative begins with at least
/// two ASCII characters. Such a prefix can be compared against original bytes
/// before a start is published, exactly as a single run can, and needs no
/// program storage because the branches are already in the instructions.
///
/// Only `SPLIT` and character runs are accepted. Anything else — a jump, a
/// class, a repeat, a folded or non-ASCII character — makes the prefix
/// undecidable here and disables the claim.
fn derive_alternation(words: &[u32], instructions: usize) -> bool {
    let entry = leading_pc(words, instructions);
    if entry >= instructions || words[HEADER + entry * 3] != SPLIT {
        return false;
    }
    let mut pending = [0usize; LEADING_BRANCHES];
    let mut depth = 0;
    let mut pc = entry;
    let mut branches = 0;
    let mut inspected = 0;
    loop {
        inspected += 1;
        if inspected > LEADING_SCAN || pc >= instructions {
            return false;
        }
        let at = HEADER + pc * 3;
        if words[at] == SPLIT {
            if depth == pending.len() {
                return false;
            }
            pending[depth] = words[at + 2] as usize;
            depth += 1;
            pc = words[at + 1] as usize;
            continue;
        }
        let mut run = 0;
        let mut next = pc;
        while next < instructions
            && words[HEADER + next * 3] == CHAR
            && words[HEADER + next * 3 + 1] < 128
            && run < LEADING_MAX
        {
            run += 1;
            next += 1;
        }
        if run < 2 {
            return false;
        }
        branches += 1;
        if depth == 0 {
            return branches >= 2;
        }
        depth -= 1;
        pc = pending[depth];
    }
}

/// Whether every successful match asserts `^` without the `m` flag before it
/// consumes anything, so it can only begin at the subject's start.
///
/// From instruction zero, capture bookkeeping is skipped and a `SPLIT` is
/// followed into both branches; every path must reach `START`. Anything else
/// first — a character, a class, a repeat, an assertion, a jump, `^` with `m`
/// — could consume or match elsewhere, and disables the claim. A search for
/// such a program tries only its first start, as a sticky one does: every later
/// start fails at the same `^`.
///
/// Derived from the instructions each search, like [`derive_leading`], rather
/// than stored, so no program word can claim it falsely.
pub(crate) fn derive_start_anchored(words: &[u32], instructions: usize) -> bool {
    let mut pending = [0usize; LEADING_BRANCHES];
    let mut depth = 0;
    let mut pc = 0;
    let mut inspected = 0;
    loop {
        inspected += 1;
        if inspected > LEADING_SCAN || pc >= instructions {
            return false;
        }
        let at = HEADER + pc * 3;
        match words[at] {
            SAVE => pc += 1,
            SPLIT => {
                if depth == pending.len() {
                    return false;
                }
                pending[depth] = words[at + 2] as usize;
                depth += 1;
                pc = words[at + 1] as usize;
            }
            START => {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
                pc = pending[depth];
            }
            _ => return false,
        }
    }
}

/// The first instruction that can consume input. Only entry `SAVE` bookkeeping
/// is skipped, so the characters that follow are a contiguous run.
pub(crate) fn leading_pc(words: &[u32], instructions: usize) -> usize {
    let mut pc = 0;
    while pc < instructions && pc < LEADING_SCAN && words[HEADER + pc * 3] == SAVE {
        pc += 1;
    }
    pc
}

/// Whether a failed attempt at one start proves failure at every later start
/// inside the same run of the pattern's leading atom.
///
/// If the pattern begins with an unbounded repeat of a single atom, an attempt
/// at `p` tries the rest of the pattern at every position the run reaches from
/// `p`. An attempt at `p + 1` inside that same run tries a subset of those
/// positions, so it must fail too. A bounded repeat has no such containment —
/// `[a-z]{1,3}` from `p + 1` reaches a position `p` never tried — and a
/// backreference can make the rest depend on what the run captured, so both
/// disable the claim.
///
/// Returns the packed instruction of the repeat, or zero.
pub(crate) fn derive_run_skip(words: &[u32], p: Program<'_>) -> u32 {
    let entry = leading_pc(words, p.instructions());
    if entry >= p.instructions() {
        return 0;
    }
    let [op, id, _] = p.instruction(entry);
    if op != ATOM_REPEAT || id as usize >= words[6] as usize {
        return 0;
    }
    let record = p.repeat(id as usize);
    // Tagged as a single-atom repeat, with no maximum.
    if record[7] != 1 || record[2] & 1 == 0 {
        return 0;
    }
    for pc in 0..p.instructions() {
        if matches!(
            p.instruction(pc)[0],
            BACKREF | BACKREF_I | NAMED_BACKREF | NAMED_BACKREF_I
        ) {
            return 0;
        }
    }
    let packed = (entry as u32 + 1) << 8;
    if packed & !RUN_SKIP != 0 { 0 } else { packed }
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
        // Word 8's upper byte carries the end-anchored length bound; word 7 has
        // no such field. Both share the character-range encoding below it. Like
        // the range descriptors, the bound's representation is checked here and
        // its semantic guarantee belongs to the compiler.
        for (index, &candidate) in words[7..9].iter().enumerate() {
            let descriptor = if index == 1 {
                candidate & 0xff_ffff
            } else {
                candidate
            };
            if index == 0 && candidate != descriptor {
                return Err(bad);
            }
            let lo = (descriptor >> 8) & 255;
            let hi = (descriptor >> 16) & 255;
            if descriptor > 1 && (descriptor != (2 | (lo << 8) | (hi << 16)) || lo > hi || hi > 127)
            {
                return Err(bad);
            }
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
        budget
            .charge(LEADING_SCAN / 8)
            .map_err(|_| ProgramError::WorkLimit)?;
        budget
            .charge(p.instructions() / 8 + 1)
            .map_err(|_| ProgramError::WorkLimit)?;
        // Word 9's bits are now fully assigned: the leading claim in its low
        // byte, the run-skip instruction in its middle bits and the forward
        // admission claim in its top bit. There are no reserved bits to reject,
        // so every field is checked on its own.
        if words[9] & RUN_SKIP != derive_run_skip(words, p)
            || words[9] & LEADING_MASK != derive_leading(words, p.instructions())
            || (words[9] & ADMISSION_FORWARD != 0 && words[2] & ADMISSION == 0)
        {
            return Err(bad);
        }
        if let Some((pc, _)) = p.admission()
            && (pc >= p.instructions()
                || !matches!(
                    p.instruction(pc)[0],
                    CHAR | CHAR_I | CLASS | CLASS_I | CLASS_SORTED | CLASS_SORTED_I
                ))
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
                CLASS_SORTED | CLASS_SORTED_I => {
                    let end = a.checked_add(b & !NEGATED).ok_or(bad)?;
                    if end > words[5] || end == a {
                        return Err(bad);
                    }
                    let mut previous = None;
                    for index in a..end {
                        budget.charge(1).map_err(|_| ProgramError::WorkLimit)?;
                        let [lo, hi] = p.range(index as usize);
                        if lo & PROPERTY != 0 || previous.is_some_and(|last| last >= lo) {
                            return Err(bad);
                        }
                        previous = Some(hi);
                    }
                    true
                }
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
                p.unicode() && properties::valid(lo & !PROPERTY) && hi <= 2
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
    /// Characters at instruction zero that every match consumes first. Zero
    /// disables the claim. See [`derive_leading`].
    pub(crate) fn leading(self) -> usize {
        (self.words[9] & LEADING_MASK & !LEADING_FOLD) as usize
    }
    /// Whether [`Program::leading`] compares without regard to ASCII case.
    pub(crate) fn leading_fold(self) -> bool {
        self.words[9] & LEADING_FOLD != 0
    }
    /// Whether every successful match consumes the admission condition at or
    /// after its own start position, so an absent later occurrence proves that
    /// no match can begin at or after that position.
    pub(crate) fn admission_forward(self) -> bool {
        self.words[9] & ADMISSION_FORWARD != 0
    }
    /// Units an end-anchored match can span, so no match can begin earlier than
    /// that far from the subject's end. Zero when no bound was derived.
    pub(crate) fn end_bound(self) -> Option<usize> {
        match self.words[8] >> 24 {
            0 => None,
            packed => Some((packed - 1) as usize),
        }
    }
    /// Instruction of the leading atom repeat whose run a failed start lets the
    /// search skip. See [`derive_run_skip`].
    pub(crate) fn run_skip(self) -> Option<usize> {
        match (self.words[9] & RUN_SKIP) >> 8 {
            0 => None,
            packed => Some(packed as usize - 1),
        }
    }
    /// Instruction holding the first character of [`Program::leading`].
    pub(crate) fn leading_pc(self) -> usize {
        leading_pc(self.words, self.instructions())
    }
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
    /// Whether the assertion at `at` has a body that is nothing but a run of
    /// ASCII characters followed by its end, which is the shape the executor
    /// already compares against original bytes.
    fn literal_assertion(self, at: usize) -> bool {
        let [_, after, _] = self.instruction(at);
        let body = at + 1;
        let Some(end) = (after as usize).checked_sub(1) else {
            return false;
        };
        let Some(length) = end.checked_sub(body) else {
            return false;
        };
        if length < 1 || length > LEADING_MAX as usize || end >= self.instructions() {
            return false;
        }
        if self.instruction(end)[0] != ASSERT_END {
            return false;
        }
        (body..end).all(|pc| {
            let [op, a, _] = self.instruction(pc);
            op == CHAR && a < 128
        })
    }

    /// Whether the compilation tier described in `docs/compilation.md` could
    /// emit code for this program, and whether its work at one start position is
    /// therefore statically bounded.
    ///
    /// This is a property of the program alone. It is not a promise that a
    /// search will be compiled: the tier also requires wholly ASCII subject
    /// storage, a host-owned code buffer, and a target it has an encoder for.
    /// A program this rejects is not refused — it is searched by the
    /// interpreter, which is the only definition of what a pattern means.
    ///
    /// The subset is deliberately small. It covers a leading capture's
    /// bookkeeping, ASCII characters, classes of ASCII ranges, `.`, and bounded
    /// repeats of those. Folding, properties, sorted classes, assertions,
    /// alternation, backreferences and unbounded repeats are all excluded, and
    /// each would be a separate decision to admit.
    pub fn compilable(self) -> bool {
        for pc in 0..self.instructions() {
            let [op, a, b] = self.instruction(pc);
            let admitted = match op {
                MATCH | SAVE => true,
                // A folded ASCII comparison is exact on the storage this tier
                // requires: the only two non-ASCII characters that fold into
                // ASCII, U+017F and U+212A, cannot occur in it. That is the
                // same argument the start scan and the retreat already make,
                // and the differential suite already covers it.
                CHAR | CHAR_I => a < 128,
                ANY | ANY_S => true,
                // Position against zero, against the length, or either side of
                // a line terminator. None of them reads the subject beyond a
                // byte, and none needs a table.
                START | START_M | END | END_M => true,
                // A word boundary over ASCII is two membership tests of a range
                // set the emitter can write out.
                WORD | WORD_I => true,
                // A lookbehind whose body is a run of ASCII characters is a
                // comparison against the bytes just before the position, which
                // the interpreter already makes over bytes and an emitter can
                // make the same way. Every other assertion needs a sub-search,
                // which this subset does not do.
                ASSERT => b & 2 != 0 && self.literal_assertion(pc),
                ASSERT_END => true,
                CLASS | CLASS_I => {
                    let count = b & !NEGATED;
                    // A property range needs the shared tables, which generated
                    // code does not carry, and a range past ASCII needs decoding
                    // this subset does not do.
                    // A folded class is the range and its case-swapped
                    // counterpart, which over ASCII is arithmetic rather than
                    // the case-equivalence table the interpreter consults.
                    (a..a + count).all(|i| {
                        let [lo, hi] = self.range(i as usize);
                        lo & PROPERTY == 0 && hi < 128
                    })
                }
                // Bounded or not. An unbounded repeat makes a search's work
                // depend on the subject, but what bounds how long generated
                // code runs before the collector may have it is the budget it
                // is given, not the pattern: a search that exhausts that budget
                // returns and is re-run by the interpreter. Requiring a static
                // bound here instead admitted only trivial literals, which are
                // the cases already within 2x of V8.
                ATOM_REPEAT => true,
                // A repeat's own structure around its body. `REPEAT_INIT` is
                // absent from this list: a repeat that needs it has captures or
                // nullability to reset per iteration, which is a separate
                // decision to admit.
                REPEAT_CHOICE | REPEAT_BODY | REPEAT_NEXT => true,
                _ => false,
            };
            if !admitted {
                return false;
            }
        }
        true
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
