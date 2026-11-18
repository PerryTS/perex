//! The validated word view used by execution loops. Provider capabilities stay
//! outside it so static DFA instantiations carry only the instructions they read.
use super::*;

#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Code<'a> {
    pub(crate) words: &'a [u32],
}

impl<'a> Code<'a> {
    /// The validated header fixes these section extents for this borrow.
    /// Taking each slice once lets the evaluator index records against their
    /// own bounds instead of rebuilding an offset into all words per opcode.
    pub(crate) fn sections(self) -> ProgramSections<'a> {
        let (instructions, tail) = self.words[HEADER..].split_at(self.instructions() * 3);
        let (ranges, tail) = tail.split_at(self.words[5] as usize * 2);
        let repeats = &tail[..self.words[6] as usize * 8];
        ProgramSections {
            instructions: instructions.as_chunks::<3>().0,
            ranges: ranges.as_chunks::<2>().0,
            repeats: repeats.as_chunks::<8>().0,
        }
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
    /// Whether [`Code::leading`] compares without regard to ASCII case.
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
    /// Instruction holding the first character of [`Code::leading`].
    /// Word 7's first-character descriptor; see `docs/candidate.md`.
    pub(crate) fn first_descriptor(self) -> u32 {
        self.words[7] & DESCRIPTOR
    }
    /// Whether every match begins at the subject's start. See
    /// [`derive_start_anchored`].
    pub(crate) fn start_anchored(self) -> bool {
        self.words[7] & ANCHORED != 0
    }
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
    fn filters_start(self) -> usize {
        HEADER + self.instructions() * 3 + self.words[5] as usize * 2 + self.words[6] as usize * 8
    }
    fn names_start(self) -> usize {
        self.filters_start() + self.words[10] as usize
    }
    /// One word of the filter section, which an instruction addresses by its
    /// first word. Members the operators removed have a zero bit.
    pub(crate) fn filter(self, at: usize) -> u32 {
        self.words[self.filters_start() + at]
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
    /// Whether the group's `index`th member survived the operators around it.
    pub(crate) fn admits(self, filter: u32, index: usize) -> bool {
        filter == 0 || self.filter(filter as usize - 1 + index / 32) & (1 << (index % 32)) != 0
    }
}
