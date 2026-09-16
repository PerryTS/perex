//! Generating a whole search, in the instructions [`super::a64`] encodes. See
//! `docs/compilation.md`.
//!
//! The subset is a sequence of atoms, some of them greedily repeated, with
//! capture bookkeeping around them: no alternation, no backreference, no
//! assertion needing a sub-search. That is what the cases measured behind V8
//! are made of, and it needs only the retreat form of backtracking rather than
//! the general frame and undo machinery the interpreter carries.
//!
//! The generated code owns the whole search, not one attempt. Compiling only
//! the attempt would leave the per-start phase machinery in place, and
//! `docs/performance.md` measures that to be most of what a short case costs.
//!
//! It assumes wholly ASCII subject storage, which is the caller's to check, and
//! that registers it does not write have already been cleared.
#![allow(dead_code)]

use super::a64::Assembler;
use super::machine::{
    AT, BYTE, Cond, EncodeError, FROM, LENGTH, Label, MAX_REPEATS, Machine, Patch, SUBJECT, Slot,
    TABLE, TMP, repeat_end, repeat_floor,
};
use super::verify::{Facts, VerifyError, verify};
use crate::program::{
    ANY, ANY_S, ASSERT, ASSERT_END, ATOM_REPEAT, CHAR, CHAR_I, CLASS, END, MATCH, NEGATED,
    PROPERTY, Program, SAVE, START, Y, consuming, derive_start_anchored,
};

/// Branches waiting for a label that has not been emitted yet.
const MAX_PATCHES: usize = 512;

/// Ranges one class may hold and still have code generated for it.
const MAX_RANGES: usize = 64;

/// Ranges past which a class is tested through a table rather than by comparing
/// each of them. Two comparisons and two branches per range is what a repeated
/// class pays for every byte of its run, and one load does not grow with the
/// class at all.
const TABLE_RANGES: u32 = 2;

/// Membership tables one program may carry.
const MAX_TABLES: usize = 8;

/// Why a program could not have code generated for it. Distinct from
/// [`EncodeError`], which is about instructions rather than programs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmitError {
    /// The program is outside the subset this generator handles. The
    /// interpreter runs it, as it runs everything.
    Unsupported,
    /// The program needed more of something than the generator carries.
    TooLarge,
    /// The instructions could not be encoded.
    Encode(EncodeError),
    /// The code was generated but could not be shown safe to execute, and has
    /// been cleared. Every one of these is a generator bug.
    Unverified(VerifyError),
}

impl From<EncodeError> for EmitError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

/// Branches that all go to one place, collected until that place is known.
struct Patches {
    list: [Patch; MAX_PATCHES],
    count: usize,
}

impl Patches {
    fn new() -> Self {
        Self {
            list: [Patch::default(); MAX_PATCHES],
            count: 0,
        }
    }
    fn push(&mut self, patch: Patch) -> Result<(), EmitError> {
        if self.count == MAX_PATCHES {
            return Err(EmitError::TooLarge);
        }
        self.list[self.count] = patch;
        self.count += 1;
        Ok(())
    }
    fn take(&mut self, other: &mut Patches) -> Result<(), EmitError> {
        for index in 0..other.count {
            self.push(other.list[index])?;
        }
        Ok(())
    }
    /// Point every branch collected here at the next instruction.
    fn bind(&mut self, asm: &mut Assembler<'_>) {
        for patch in &self.list[..self.count] {
            asm.bind(*patch);
        }
        self.count = 0;
    }
}

/// An open repeat: where to re-run its continuation from, and the branches that
/// will make it give a character back.
struct Repeat {
    retry: Label,
    failures: Patches,
}

/// Whether this program is a sequence of atoms and greedy repeats of atoms,
/// which is what [`emit_search`] generates.
pub fn supported(program: Program<'_>) -> bool {
    // A sticky search tries only its requested position, which is a different
    // search from the one generated here.
    if program.words()[2] & Y != 0 {
        return false;
    }
    let mut pc = 0;
    let mut depth = 0;
    loop {
        if pc >= program.instructions() {
            return false;
        }
        let [op, a, b] = program.instruction(pc);
        match op {
            MATCH => return true,
            // Position tests, which consume nothing. The multiline forms also
            // match at a line terminator, which is a different test and a
            // separate decision to admit.
            SAVE | START | END => pc += 1,
            // A lookbehind over a run of characters. Its body and end are
            // stepped over, not visited: the comparison replaces them.
            ASSERT if literal_lookbehind(program, pc).is_some() => pc = a as usize,
            ATOM_REPEAT => {
                let record = program.repeat(a as usize);
                // A lazy repeat takes its continuation before its body, which
                // is a different shape from the retreat emitted below.
                if record[2] & 2 != 0 || depth == MAX_REPEATS {
                    return false;
                }
                let body = pc + 3;
                if body >= program.instructions() {
                    return false;
                }
                let [body_op, body_a, body_b] = program.instruction(body);
                if !atom(program, body_op, body_a, body_b) {
                    return false;
                }
                // A bound past an immediate's reach is rarer than it is worth
                // encoding around.
                if record[0] >= 1 << 12 || (record[2] & 1 == 0 && record[1] >= 1 << 12) {
                    return false;
                }
                depth += 1;
                pc = record[4] as usize;
            }
            _ if atom(program, op, a, b) => pc += 1,
            _ => return false,
        }
    }
}

/// Whether one instruction consumes a character this generator can test.
fn atom(program: Program<'_>, op: u32, a: u32, b: u32) -> bool {
    match op {
        CHAR | CHAR_I => a < 128,
        ANY | ANY_S => true,
        CLASS => {
            let count = b & !NEGATED;
            count > 0
                && count as usize <= MAX_RANGES
                && (a..a + count).all(|i| {
                    let [lo, hi] = program.range(i as usize);
                    lo & PROPERTY == 0 && hi < 128
                })
        }
        _ => false,
    }
}

/// The body of a lookbehind that is nothing but a run of ASCII characters,
/// which is a comparison against the bytes just before the position rather than
/// a reversed sub-search. `None` for any other assertion.
///
/// This is the same shape the interpreter already compares over bytes.
fn literal_lookbehind(program: Program<'_>, pc: usize) -> Option<(usize, usize)> {
    let [_, after, flags] = program.instruction(pc);
    // Bit one is the reverse direction; a lookahead needs a sub-search.
    if flags & 2 == 0 {
        return None;
    }
    let body = pc + 1;
    let end = (after as usize).checked_sub(1)?;
    let length = end.checked_sub(body)?;
    if !(1..1 << 12).contains(&length) || end >= program.instructions() {
        return None;
    }
    if program.instruction(end)[0] != ASSERT_END {
        return None;
    }
    (body..end)
        .all(|pc| {
            let [op, a, _] = program.instruction(pc);
            op == CHAR && a < 128
        })
        .then_some((body, length))
}

/// Send a failure to the innermost repeat that can retreat, or to the next
/// start when there is none.
fn give_up(
    next_start: &mut Patches,
    repeats: &mut [Option<Repeat>; MAX_REPEATS],
    depth: usize,
    patch: Patch,
) -> Result<(), EmitError> {
    match depth {
        0 => next_start.push(patch),
        _ => repeats[depth - 1]
            .as_mut()
            .expect("an open repeat")
            .failures
            .push(patch),
    }
}

/// Membership tables, written after the code that reads them.
struct Tables {
    data: [[u8; 256]; MAX_TABLES],
    at: [Patch; MAX_TABLES],
    count: usize,
}

impl Tables {
    fn new() -> Self {
        Self {
            data: [[0; 256]; MAX_TABLES],
            at: [Patch::default(); MAX_TABLES],
            count: 0,
        }
    }

    /// Emit an address for a table of everything this class admits, and keep
    /// the table to be written once the code is done.
    fn address(
        &mut self,
        asm: &mut Assembler<'_>,
        program: Program<'_>,
        into: Slot,
        a: u32,
        b: u32,
    ) -> Result<(), EmitError> {
        if self.count == MAX_TABLES {
            return Err(EmitError::TooLarge);
        }
        let count = b & !NEGATED;
        let negated = b & NEGATED != 0;
        for byte in 0..=255u32 {
            let inside = (a..a + count).any(|i| {
                let [lo, hi] = program.range(i as usize);
                byte >= lo && byte <= hi
            });
            self.data[self.count][byte as usize] = u8::from(inside != negated);
        }
        self.at[self.count] = asm.data_address(into);
        self.count += 1;
        Ok(())
    }

    /// Write every table where the addresses above point.
    fn place(&mut self, asm: &mut Assembler<'_>) {
        for index in 0..self.count {
            asm.bind(self.at[index]);
            asm.data(&self.data[index]);
        }
    }
}

/// Characters every match must still consume from `pc` onwards. A repeat
/// contributes its minimum, since that is all it is obliged to take.
fn least_from(program: Program<'_>, mut pc: usize) -> usize {
    let mut least = 0;
    while pc < program.instructions() {
        let [op, a, _] = program.instruction(pc);
        match op {
            MATCH => break,
            ATOM_REPEAT => {
                let record = program.repeat(a as usize);
                least += record[0] as usize;
                pc = record[4] as usize;
            }
            // An assertion consumes nothing, and the characters in its body are
            // not the match's: walking into them would count them twice over
            // and reject starts that do fit.
            ASSERT => pc = a as usize,
            other => {
                least += usize::from(consuming(other));
                pc += 1;
            }
        }
    }
    least
}

/// Test the byte at `AT` against one atom, branching to `fail` when it does not
/// hold. Does not advance; the caller decides whether a match consumes.
fn emit_atom(
    asm: &mut Assembler<'_>,
    program: Program<'_>,
    op: u32,
    a: u32,
    b: u32,
    table: Option<Slot>,
    fail: &mut Patches,
) -> Result<(), EmitError> {
    match op {
        // Anything at all, so nothing to test.
        ANY_S => {}
        CHAR | CHAR_I => {
            asm.load_byte(BYTE, SUBJECT, AT);
            let byte = a as u8;
            if op == CHAR_I && byte.is_ascii_alphabetic() {
                // Either case, which over this storage is the whole of what
                // folding means: the two non-ASCII characters that fold into
                // ASCII cannot occur in it.
                asm.compare_byte(BYTE, u32::from(byte.to_ascii_lowercase()));
                let matched = asm.branch_if(Cond::Equal);
                asm.compare_byte(BYTE, u32::from(byte.to_ascii_uppercase()));
                fail.push(asm.branch_if(Cond::NotEqual))?;
                asm.bind(matched);
            } else {
                asm.compare_byte(BYTE, u32::from(byte));
                fail.push(asm.branch_if(Cond::NotEqual))?;
            }
        }
        // Anything but a line terminator. On ASCII storage only the two
        // single-byte terminators can occur; U+2028 and U+2029 are not
        // representable in it.
        ANY => {
            asm.load_byte(BYTE, SUBJECT, AT);
            asm.compare_byte(BYTE, u32::from(b'\n'));
            fail.push(asm.branch_if(Cond::Equal))?;
            asm.compare_byte(BYTE, u32::from(b'\r'));
            fail.push(asm.branch_if(Cond::Equal))?;
        }
        CLASS => {
            asm.load_byte(BYTE, SUBJECT, AT);
            let count = b & !NEGATED;
            let negated = b & NEGATED != 0;
            if let Some(table) = table {
                // One load, whatever the class holds.
                asm.load_byte(BYTE, table, BYTE);
                asm.compare_byte(BYTE, 0);
                fail.push(asm.branch_if(Cond::Equal))?;
            } else if count == 1 {
                // A byte is inside one range exactly when subtracting the low
                // bound leaves something no larger than the range is wide,
                // which is one subtraction and one comparison rather than two
                // comparisons and two branches.
                let [lo, hi] = program.range(a as usize);
                fail.push(asm.byte_within(BYTE, lo, hi, negated))?;
            } else {
                // Each range jumps out when the byte is inside it. Falling past
                // all of them means the byte is inside none.
                let mut inside = [Patch::default(); MAX_RANGES];
                for (slot, index) in inside.iter_mut().zip(a..a + count) {
                    let [lo, hi] = program.range(index as usize);
                    asm.compare_byte(BYTE, lo);
                    let below = asm.branch_if(Cond::Less);
                    asm.compare_byte(BYTE, hi);
                    *slot = asm.branch_if(Cond::AtMost);
                    asm.bind(below);
                }
                if negated {
                    let accepted = asm.branch();
                    for patch in &inside[..count as usize] {
                        asm.bind(*patch);
                    }
                    fail.push(asm.branch())?;
                    asm.bind(accepted);
                } else {
                    fail.push(asm.branch())?;
                    for patch in &inside[..count as usize] {
                        asm.bind(*patch);
                    }
                }
            }
        }
        _ => return Err(EmitError::Unsupported),
    }
    Ok(())
}

/// What the generated code returns when there is no match.
pub const NO_MATCH: isize = -1;
/// What the generated code returns when its budget ran out before the search
/// was decided. The interpreter runs the same search from the start.
pub const EXHAUSTED: isize = -2;

/// Subjects up to this many bytes are where generated code beats the
/// interpreter for a program that has to try every start.
///
/// Generated code tries starts one at a time; the interpreter skips them in
/// bulk, searching raw bytes for what a match must contain. So the tier wins
/// where the flat cost of entering a search dominates and loses where the
/// scan does, and the crossing is a length. Measured over subjects that match
/// nothing, which is the case that tries every start: `[0-9]+[A-Z]+` and `a+!`
/// cross at 24 to 32 bytes, `needle` at 64, a folded literal past 128, and a
/// two-capture pattern between 48 and 64. Thirty-two is the shortest of those
/// rounded to a power of two, so at worst a host gives up a small win on the
/// two shapes that cross earliest — 1.06x and 1.13x on the measured pair —
/// rather than taking a large loss on any of them. See `docs/compilation.md`.
const SHORT_SUBJECT: usize = 32;

/// Whether a host holding generated code for this program should run it for a
/// subject of this many bytes, or hand the search to the interpreter.
///
/// The decision is the program's and the length's, never the subject's
/// contents or how a search is going: both paths answer identically, so this
/// only chooses which one is faster, and a wrong choice costs time rather than
/// correctness. It is false for a program the tier cannot emit at all.
///
/// A program that does not try every start — one anchored at the subject's
/// beginning, or one whose match must end at its end — keeps the tier at any
/// length, because what generated code loses on a long subject is the walk.
pub fn preferred(program: Program<'_>, bytes: usize) -> bool {
    supported(program)
        && (bytes <= SHORT_SUBJECT
            || program.end_bound().is_some()
            || derive_start_anchored(program.words(), program.instructions()))
}

/// Emit a whole search for `program` into `code`, returning its byte length.
///
/// The generated code is called as
/// `extern "C" fn(subject: *const u8, length: usize, start: usize,
/// registers: *mut usize, budget: usize) -> isize`, and returns the position a
/// match began at, [`NO_MATCH`], or [`EXHAUSTED`] once it has taken `budget`
/// backward branches without deciding. It writes only into the first
/// `program.register_count()` registers, reads only within the length, and
/// calls nothing.
///
/// Before returning, the code is checked by [`verify`], which needs one
/// [`Facts`] for every four bytes emitted. Code that fails is cleared and
/// reported rather than returned.
pub fn emit_search(
    program: Program<'_>,
    code: &mut [u8],
    facts: &mut [Facts],
) -> Result<usize, EmitError> {
    let length = generate(program, code)?;
    if let Err(error) = verify(&code[..length], program.register_count(), facts) {
        // Zero is permanently undefined on this architecture, so a host that
        // maps the buffer regardless faults instead of running unchecked code.
        code[..length].fill(0);
        return Err(EmitError::Unverified(error));
    }
    Ok(length)
}

fn generate(program: Program<'_>, code: &mut [u8]) -> Result<usize, EmitError> {
    if !supported(program) {
        return Err(EmitError::Unsupported);
    }
    // What every match must consume, which is what proves each load below is
    // inside the subject.
    let least = least_from(program, 0);
    if least >= 1 << 12 {
        return Err(EmitError::TooLarge);
    }

    let mut asm = Assembler::new(code);
    let mut tables = Tables::new();
    // Failures with no repeat left to retreat give up on this start.
    let mut next_start = Patches::new();
    // Starts with no match left to find, and loops with no budget left.
    let mut no_match = Patches::new();
    let mut out_of_budget = Patches::new();
    let mut repeats: [Option<Repeat>; MAX_REPEATS] = [None, None, None];
    let mut depth = 0usize;

    // An end-anchored match of bounded length cannot begin more than that many
    // positions from the subject's end, so the whole prefix before that is
    // skipped rather than scanned. Without this the generated code tries every
    // start of a long subject where the interpreter tries one, and is four
    // thousand times slower for it.
    if let Some(bound) = program.end_bound() {
        if bound >= 1 << 12 {
            return Err(EmitError::TooLarge);
        }
        asm.compare_position(LENGTH, bound as u32);
        let whole = asm.branch_if(Cond::Less);
        asm.behind(TMP, LENGTH, bound as u32);
        asm.compare(FROM, TMP);
        let already = asm.branch_if(Cond::AtLeast);
        asm.copy(FROM, TMP);
        asm.bind(already);
        asm.bind(whole);
    }

    // No room for what every match must consume means no room at any later
    // start either, so passing the last start with room ends the search. That
    // is also what makes a start past the end no match, as it is to the
    // interpreter, rather than an address to read from.
    no_match.push(asm.fix_room(least as u32))?;

    // Every later start of a start-anchored program fails at the same `^`, so
    // there is one start to try, which is what the interpreter does. Trying
    // them all is what made generated code four orders of magnitude slower
    // than the interpreter over a long subject. A sticky program has one start
    // too, but `supported` refuses those before this.
    let one_start = derive_start_anchored(program.words(), program.instructions());

    let outer = asm.here();
    no_match.push(asm.start_without_room())?;
    asm.copy(AT, FROM);

    let mut pc = 0;
    loop {
        let [op, a, b] = program.instruction(pc);
        match op {
            SAVE => {
                asm.store_position(AT, a);
                pc += 1;
            }
            MATCH => {
                asm.return_start();
                break;
            }
            // `^` without `m`: only the subject's own beginning.
            START => {
                asm.compare_position(AT, 0);
                let elsewhere = asm.branch_if(Cond::NotEqual);
                give_up(&mut next_start, &mut repeats, depth, elsewhere)?;
                pc += 1;
            }
            // `$` without `m`: only the subject's own end.
            END => {
                asm.compare(AT, LENGTH);
                let elsewhere = asm.branch_if(Cond::NotEqual);
                give_up(&mut next_start, &mut repeats, depth, elsewhere)?;
                pc += 1;
            }
            ASSERT => {
                let (body, length) =
                    literal_lookbehind(program, pc).ok_or(EmitError::Unsupported)?;
                let negative = b & 1 != 0;
                let mut absent = Patches::new();
                // Too near the beginning for the text to be there at all.
                asm.compare_position(AT, length as u32);
                absent.push(asm.branch_if(Cond::Less))?;
                asm.window_before(AT, length as u32);
                // The body is stored in the order a lookbehind reads it, which
                // is backwards: its first character is the one just before the
                // position. So body `offset` is the byte `length - 1 - offset`
                // into the window this compares.
                for offset in 0..length {
                    asm.load_window_byte(BYTE, (length - 1 - offset) as u32);
                    asm.compare_byte(BYTE, program.instruction(body + offset)[1]);
                    absent.push(asm.branch_if(Cond::NotEqual))?;
                }
                if negative {
                    // Every character matched, so the text this describes is
                    // there, which is what the negative form fails on.
                    let present = asm.branch();
                    absent.bind(&mut asm);
                    let accepted = asm.branch();
                    asm.bind(present);
                    give_up(&mut next_start, &mut repeats, depth, asm.branch())?;
                    asm.bind(accepted);
                } else {
                    for index in 0..absent.count {
                        let patch = absent.list[index];
                        give_up(&mut next_start, &mut repeats, depth, patch)?;
                    }
                }
                // The assertion consumes nothing, and its body and end are
                // replaced by the comparison above.
                pc = a as usize;
            }
            ATOM_REPEAT => {
                let record = program.repeat(a as usize);
                let (fewest, most, infinite) = (record[0], record[1], record[2] & 1 != 0);
                let [body_op, body_a, body_b] = program.instruction(pc + 3);
                let end = repeat_end(depth);
                let floor = repeat_floor(depth);

                // The earliest end the minimum allows, and the latest anything
                // allows. Consuming past what the rest of the pattern still
                // needs could only be given back again, and stopping there is
                // also what keeps every load in the continuation inside the
                // subject: the upfront bound covers the minimum, and a greedy
                // repeat can take a subject to its end.
                asm.ahead(floor, AT, fewest);
                let after = least_from(program, record[4] as usize);
                if after >= 1 << 12 {
                    return Err(EmitError::TooLarge);
                }
                asm.behind(TMP, LENGTH, after as u32);
                if !infinite {
                    asm.limit_to(TMP, AT, most);
                }

                // A repeated class of several ranges is tested through a
                // table, whose address is taken once here rather than in the
                // loop that reads it.
                let scan_table = if body_op == CLASS && body_b & !NEGATED >= TABLE_RANGES {
                    tables.address(&mut asm, program, TABLE, body_a, body_b)?;
                    Some(TABLE)
                } else {
                    None
                };

                // Consume greedily up to that limit.
                let scan = asm.here();
                let mut stop = Patches::new();
                asm.compare(AT, TMP);
                stop.push(asm.branch_if(Cond::AtLeast))?;
                emit_atom(
                    &mut asm, program, body_op, body_a, body_b, scan_table, &mut stop,
                )?;
                asm.ahead(AT, AT, 1);
                out_of_budget.push(asm.branch_back(scan))?;
                stop.bind(&mut asm);

                // Short of the minimum is failure, and retreating cannot help.
                asm.copy(end, AT);
                asm.compare(end, floor);
                let short = asm.branch_if(Cond::Less);
                match depth {
                    0 => next_start.push(short)?,
                    _ => repeats[depth - 1]
                        .as_mut()
                        .expect("an open repeat")
                        .failures
                        .push(short)?,
                }

                // Everything after this is the continuation, re-entered from
                // here each time this repeat gives a character back.
                let retry = asm.here();
                asm.copy(AT, end);
                repeats[depth] = Some(Repeat {
                    retry,
                    failures: Patches::new(),
                });
                depth += 1;
                pc = record[4] as usize;
            }
            _ => {
                let mut fail = Patches::new();
                emit_atom(&mut asm, program, op, a, b, None, &mut fail)?;
                asm.ahead(AT, AT, 1);
                // A consuming atom that fails retreats the innermost repeat, or
                // gives up on this start when there is none.
                match depth {
                    0 => next_start.take(&mut fail)?,
                    _ => repeats[depth - 1]
                        .as_mut()
                        .expect("an open repeat")
                        .failures
                        .take(&mut fail)?,
                }
                pc += 1;
            }
        }
    }

    // Retreat code, innermost first: give a character back and re-run the
    // continuation, until the minimum stops it and the failure goes outward.
    while depth > 0 {
        depth -= 1;
        let mut repeat = repeats[depth].take().expect("an open repeat");
        repeat.failures.bind(&mut asm);
        let end = repeat_end(depth);
        let floor = repeat_floor(depth);
        asm.compare(end, floor);
        let spent = asm.branch_if(Cond::AtMost);
        asm.behind(end, end, 1);
        out_of_budget.push(asm.branch_back(repeat.retry))?;
        match depth {
            0 => next_start.push(spent)?,
            _ => repeats[depth - 1]
                .as_mut()
                .expect("an open repeat")
                .failures
                .push(spent)?,
        }
    }

    next_start.bind(&mut asm);
    if !one_start {
        asm.ahead(FROM, FROM, 1);
        out_of_budget.push(asm.branch_back(outer))?;
    }

    no_match.bind(&mut asm);
    asm.return_code(NO_MATCH as i32);
    out_of_budget.bind(&mut asm);
    asm.return_code(EXHAUSTED as i32);
    tables.place(&mut asm);
    asm.done().map_err(EmitError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Budget;
    use crate::compiler::{Node, Range, compile};
    use crate::executor::{Frame, Scratch, Undo, find};
    use crate::input::Input;
    use crate::span::Span;

    /// Enough of AArch64 to run what [`emit_search`] emits, so the generated
    /// code can be checked against the interpreter without this crate mapping,
    /// protecting or calling anything.
    ///
    /// It decodes rather than trusting the encoder's own idea of what it wrote:
    /// an emulator built from the same constants as the emitter would agree
    /// with it whatever either of them did. It shares no code with the
    /// verifier's decoder either, because the mutation test below runs what the
    /// verifier accepted here.
    ///
    /// Everything the verifier promises cannot happen is a [`Fault`] rather than
    /// a panic, so that mutated code can be run: a load outside the subject and
    /// the code, a store outside the registers, a write to a register the
    /// calling convention preserves, and running past the budget's bound.
    ///
    /// The three regions sit at distinct base addresses, so an address says
    /// which one it is in. Real hardware needs no such tag; this is the
    /// emulator standing in for one address space.
    const SUBJECT_BASE: u64 = 1 << 36;
    const CODE_BASE: u64 = 1 << 40;
    const REGISTERS_BASE: u64 = 1 << 44;

    type Fault = &'static str;

    struct Machine<'a> {
        x: [u64; 31],
        z: bool,
        c: bool,
        subject: &'a [u8],
        code: &'a [u8],
        registers: &'a mut [u64],
    }

    impl Machine<'_> {
        fn read(&self, index: u32) -> Result<u64, Fault> {
            match index {
                31 => Err("register 31 as an operand"),
                _ => Ok(self.x[index as usize]),
            }
        }
        fn write(&mut self, index: u32, value: u64) -> Result<(), Fault> {
            if index >= 18 {
                return Err("wrote a register the calling convention preserves");
            }
            self.x[index as usize] = value;
            Ok(())
        }
        fn flags(&mut self, a: u64, b: u64) {
            self.z = a == b;
            self.c = a >= b;
        }
        /// Read one byte from whichever region the address names.
        fn load(&self, at: u64) -> Result<u8, Fault> {
            let inside = |base: u64, region: &[u8]| {
                let offset = usize::try_from(at.checked_sub(base)?).ok()?;
                region.get(offset).copied()
            };
            inside(SUBJECT_BASE, self.subject)
                .or_else(|| inside(CODE_BASE, self.code))
                .ok_or("load outside the subject and the code")
        }
        fn store(&mut self, at: u64, value: u64) -> Result<(), Fault> {
            let offset = at
                .checked_sub(REGISTERS_BASE)
                .filter(|offset| offset % 8 == 0)
                .ok_or("store outside the registers")?;
            let slot = usize::try_from(offset / 8).map_err(|_| "store outside the registers")?;
            *self
                .registers
                .get_mut(slot)
                .ok_or("store outside the registers")? = value;
            Ok(())
        }
        fn holds(&self, cond: u32) -> Result<bool, Fault> {
            Ok(match cond {
                0 => self.z,
                1 => !self.z,
                2 => self.c,
                3 => !self.c,
                8 => self.c && !self.z,
                9 => !self.c || self.z,
                _ => return Err("a condition the emulator does not model"),
            })
        }

        /// Run until `RET`, returning X0 and the instructions executed, or a
        /// fault after `limit` of them.
        fn run(&mut self, limit: usize) -> Result<(i64, usize), Fault> {
            let mut pc = 0usize;
            for step in 1..=limit {
                let bytes = self.code.get(pc..pc + 4).ok_or("ran outside the code")?;
                let w = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                let here = pc as i64;
                pc += 4;
                let (rd, rn, rm) = (w & 31, (w >> 5) & 31, (w >> 16) & 31);
                let imm12 = u64::from((w >> 10) & 0xfff);
                let jump = |words: i64| {
                    usize::try_from(here + words * 4).map_err(|_| "branched outside the code")
                };
                if w == 0xd65f_03c0 {
                    return Ok((self.x[0] as i64, step));
                } else if w & 0xffe0_0000 == 0xd280_0000 {
                    self.write(rd, u64::from((w >> 5) & 0xffff))?;
                } else if w & 0xffe0_0000 == 0x9280_0000 {
                    self.write(rd, !u64::from((w >> 5) & 0xffff))?;
                } else if w & 0xffe0_ffe0 == 0xaa00_03e0 {
                    let value = self.read(rm)?;
                    self.write(rd, value)?;
                } else if w & 0xffc0_001f == 0x7100_001f {
                    let value = self.read(rn)? as u32;
                    self.flags(u64::from(value), imm12);
                } else if w & 0xffc0_001f == 0xf100_001f {
                    let value = self.read(rn)?;
                    self.flags(value, imm12);
                } else if w & 0xffc0_0000 == 0x9100_0000 {
                    let value = self.read(rn)?.wrapping_add(imm12);
                    self.write(rd, value)?;
                } else if w & 0xffc0_0000 == 0xd100_0000 {
                    let value = self.read(rn)?.wrapping_sub(imm12);
                    self.write(rd, value)?;
                } else if w & 0xffe0_fc1f == 0xeb00_001f {
                    let (a, b) = (self.read(rn)?, self.read(rm)?);
                    self.flags(a, b);
                } else if w & 0xffe0_fc00 == 0x3860_6800 {
                    let at = self.read(rn)?.wrapping_add(self.read(rm)?);
                    let byte = self.load(at)?;
                    self.write(rd, u64::from(byte))?;
                } else if w & 0xffc0_0000 == 0x3940_0000 {
                    let at = self.read(rn)?.wrapping_add(imm12);
                    let byte = self.load(at)?;
                    self.write(rd, u64::from(byte))?;
                } else if w & 0xffc0_0000 == 0x5100_0000 {
                    let value = (self.read(rn)? as u32).wrapping_sub(imm12 as u32);
                    self.write(rd, u64::from(value))?;
                } else if w & 0xffe0_fc00 == 0x8b00_0000 {
                    let value = self.read(rn)?.wrapping_add(self.read(rm)?);
                    self.write(rd, value)?;
                } else if w & 0x9f00_0000 == 0x1000_0000 {
                    let immlo = (w >> 29) & 3;
                    let immhi = (w >> 5) & 0x7_ffff;
                    let offset = (((immhi << 2) | immlo) as i32) << 11 >> 11;
                    let at = (here + i64::from(offset)) as u64;
                    self.write(rd, CODE_BASE.wrapping_add(at))?;
                } else if w & 0xffc0_0000 == 0xf900_0000 {
                    let at = self.read(rn)?.wrapping_add(imm12 * 8);
                    let value = self.read(rd)?;
                    self.store(at, value)?;
                } else if w & 0xff00_0010 == 0x5400_0000 {
                    let offset = ((((w >> 5) & 0x7_ffff) as i32) << 13) >> 13;
                    if self.holds(w & 15)? {
                        pc = jump(i64::from(offset))?;
                    }
                } else if w & 0xfc00_0000 == 0x1400_0000 {
                    let offset = (((w & 0x3ff_ffff) as i32) << 6) >> 6;
                    pc = jump(i64::from(offset))?;
                } else if w & 0xff00_0000 == 0xb400_0000 {
                    let offset = ((((w >> 5) & 0x7_ffff) as i32) << 13) >> 13;
                    if self.read(rd)? == 0 {
                        pc = jump(i64::from(offset))?;
                    }
                } else {
                    return Err("an instruction the emulator does not decode");
                }
            }
            Err("ran past its bound")
        }
    }

    /// One execution of generated code.
    struct Run {
        answer: i64,
        registers: [u64; 16],
        steps: usize,
    }

    /// Execute `code` against one subject, with the instruction bound the
    /// verifier proves for this budget as the emulator's own limit.
    fn execute(
        code: &[u8],
        register_count: usize,
        subject: &str,
        start: u64,
        budget: u64,
    ) -> Result<Run, Fault> {
        let instructions = (code.len() / 4) as u64;
        let bound = budget.saturating_add(1).saturating_mul(instructions);
        let limit = bound.min(2_000_000) as usize;
        let mut registers = [u64::MAX; 16];
        let mut machine = Machine {
            x: [0; 31],
            z: false,
            c: false,
            subject: subject.as_bytes(),
            code,
            registers: &mut registers[..register_count],
        };
        machine.x[0] = SUBJECT_BASE;
        machine.x[1] = subject.len() as u64;
        machine.x[2] = start;
        machine.x[3] = REGISTERS_BASE;
        machine.x[4] = budget;
        let (answer, steps) = machine.run(limit)?;
        Ok(Run {
            answer,
            registers,
            steps,
        })
    }

    /// Compile `pattern` and generate code for it into `code`, returning the
    /// code's length and the program's register count.
    fn generate_for(
        pattern: &str,
        flags: &str,
        code: &mut [u8],
    ) -> Result<(usize, usize), EmitError> {
        let source = Input::utf8(pattern);
        let mut nodes = [Node::default(); 256];
        let mut ranges = [Range::default(); 512];
        let mut words = [0u32; 2048];
        let mut budget = Budget::new(10_000_000);
        let program = compile(
            source,
            flags,
            &mut nodes,
            &mut ranges,
            &mut words,
            &mut budget,
        )
        .expect("pattern compiles");
        let mut facts = [Facts::default(); 1024];
        let length = emit_search(program, code, &mut facts)?;
        Ok((length, program.register_count()))
    }

    /// The same search through the interpreter, which is what a pattern means.
    fn interpret(
        pattern: &str,
        flags: &str,
        subject: &str,
        start: usize,
    ) -> (bool, [Option<Span>; 8]) {
        let source = Input::utf8(pattern);
        let mut nodes = [Node::default(); 256];
        let mut ranges = [Range::default(); 512];
        let mut words = [0u32; 2048];
        let mut budget = Budget::new(10_000_000);
        let program = compile(
            source,
            flags,
            &mut nodes,
            &mut ranges,
            &mut words,
            &mut budget,
        )
        .expect("pattern compiles");
        let mut registers = [0usize; 64];
        let mut frames = [Frame::default(); 256];
        let mut undo = [Undo::default(); 1024];
        let mut captures = [None; 8];
        let mut budget = Budget::new(100_000_000);
        let found = find(
            program,
            Input::utf8(subject),
            start,
            Scratch {
                registers: &mut registers[..program.register_count()],
                frames: &mut frames,
                undo: &mut undo,
            },
            &mut captures[..program.capture_count()],
            &mut budget,
        )
        .expect("search runs");
        (found, captures)
    }

    /// Which search an assertion is about.
    #[derive(Clone, Copy)]
    struct Case<'a> {
        pattern: &'a str,
        flags: &'a str,
        subject: &'a str,
        start: u64,
        budget: u64,
    }

    impl core::fmt::Display for Case<'_> {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(
                f,
                "/{}/{} against {:?} from {}, budget {}",
                self.pattern, self.flags, self.subject, self.start, self.budget
            )
        }
    }

    const PATTERNS: &[(&str, &str)] = &[
        ("a", ""),
        ("abc", ""),
        ("needle", ""),
        ("NeEdLe", "i"),
        ("aA", "i"),
        ("(a)", ""),
        ("(a)(b)", ""),
        ("(ab)c", ""),
        ("x", ""),
        ("", ""),
        ("[a-z]", ""),
        ("[a-z][0-9]", ""),
        ("[^a-z]", ""),
        ("[^0-9]x", ""),
        ("[abc]", ""),
        ("[a-z0-9_]", ""),
        (".", ""),
        ("a.c", ""),
        (".", "s"),
        ("a.", "s"),
        ("([a-z])(.)", ""),
        ("x[^\n]y", ""),
        ("a+", ""),
        ("a*", ""),
        ("a?", ""),
        ("a{2}", ""),
        ("a{2,3}", ""),
        ("[a-z]+", ""),
        ("[a-z]+[0-9]+", ""),
        ("[a-z]+x", ""),
        ("a+b", ""),
        ("a*b", ""),
        (".+z", ""),
        ("[0-9]*[a-z]", ""),
        ("(a+)(b+)", ""),
        ("([a-z]+)@([a-z]+)", ""),
        ("x[a-z]*y", ""),
        ("[^0-9]+9", ""),
        ("a+a", ""),
        ("a{1,2}b", ""),
        ("^a", ""),
        ("a$", ""),
        ("^abc$", ""),
        ("^a+$", ""),
        ("[a-z]+$", ""),
        ("^[a-z]+[0-9]$", ""),
        ("needle$", ""),
        ("^(a)(b)$", ""),
        ("a$", ""),
        // Repeated multi-range classes, which are the only shape that
        // reaches the membership table.
        (r"\w+", ""),
        (r"\w+!", ""),
        (r"\d+", ""),
        ("[a-z0-9_]+x", ""),
        ("[^a-z0-9]+", ""),
        (r"(\w+)@(\w+)", ""),
        ("[a-cx-z]+q", ""),
        ("(?<=abc)d", ""),
        ("(?<!abc)d", ""),
        ("(?<=0123456789)abc", ""),
        ("(?<=a)b", ""),
        ("(?<!a)b", ""),
        ("x(?<=x)y", ""),
        ("(?<=ab)c$", ""),
        // End-anchored and unrepeated, so the start skip applies. A class or
        // `.` counts two units towards its bound, since either could match an
        // astral character, so on ASCII storage a subject can match while
        // being shorter than the bound.
        ("[a-z]$", ""),
        (".$", ""),
        ("x[0-9]$", ""),
        // Three open repeats of overlapping classes, which the compiler cannot
        // merge, over a run none of them can finish: every division of the run
        // among them is tried.
        ("[ab]*[ac]*a*x", ""),
    ];

    const SUBJECTS: &[&str] = &[
        "",
        "a",
        "b",
        "abc",
        "aabc",
        "xxabcxx",
        "needle",
        "haystack with a needle inside",
        "NEEDLE",
        "nEeDlE",
        "ab",
        "ba",
        "aaaa",
        "cba",
        "a1",
        "1a",
        "a\nb",
        "a\rb",
        "_z9",
        "A",
        "  ",
        "xy",
        "x\ny",
        "abcdef",
        "aaa",
        "aaab",
        "b",
        "ab123",
        "abc123xyz",
        "xaaay",
        "xy",
        "user@example",
        "9",
        "a9",
        "zzz9",
        "aab",
        "aaaaab",
        "0a",
        "0123a",
        "az",
        "aaz",
        "  z",
        // Bytes immediately outside the ranges above, which an off-by-one
        // on a bound would accept or reject wrongly.
        "{",
        "a{",
        "`a",
        "z{",
        "09:",
        "/0",
        "AZ[",
        "@A",
        "_",
        "^_`",
        "a_9Z",
        "  \t ",
        "w+x",
        "abc@def",
        "xyzq",
        "abcq",
        "abcd",
        "xabcd",
        "d",
        "bd",
        "0123456789abc",
        "x0123456789abc",
        "ab",
        "xb",
        "xy",
        "xxy",
        "abc",
    ];

    /// Thirty `a`s, which `[ab]*[ac]*a*x` divides every possible way.
    const RUN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn generated_code_agrees_with_the_interpreter() {
        let mut code = [0u8; 4096];
        for &(pattern, flags) in PATTERNS {
            let (length, count) = generate_for(pattern, flags, &mut code).unwrap_or_else(|error| {
                panic!("/{pattern}/{flags} should have code generated for it: {error:?}")
            });
            let emitted = &code[..length];
            for &subject in SUBJECTS {
                // Every start, and starts past the end, which are no match
                // rather than addresses to read from.
                for start in (0..=subject.len() + 1).chain([usize::MAX]) {
                    let context = || Case {
                        pattern,
                        flags,
                        subject,
                        start: start as u64,
                        budget: u64::MAX,
                    };
                    let run = execute(emitted, count, subject, start as u64, u64::MAX)
                        .unwrap_or_else(|fault| panic!("{}: {fault}", context()));
                    let (found, captures) = interpret(pattern, flags, subject, start);
                    assert_eq!(
                        run.answer >= 0,
                        found,
                        "{}: compiled said {}, interpreted said {found}",
                        context(),
                        run.answer
                    );
                    if !found {
                        assert_eq!(run.answer, NO_MATCH as i64, "{}", context());
                        continue;
                    }
                    let span = captures[0].expect("a match has a span");
                    assert_eq!(run.answer as usize, span.start(), "{}: start", context());
                    for (index, capture) in captures.iter().enumerate() {
                        let Some(span) = capture else { continue };
                        assert_eq!(
                            run.registers[index * 2] as usize,
                            span.start(),
                            "{}: capture {index} start",
                            context()
                        );
                        assert_eq!(
                            run.registers[index * 2 + 1] as usize,
                            span.end(),
                            "{}: capture {index} end",
                            context()
                        );
                    }
                }
            }
        }
    }

    /// A budget ends a search early or not at all. A run that runs out says
    /// so, a run that decides agrees with an unlimited one, and no run executes
    /// more than the verifier's bound, which [`execute`] enforces.
    #[test]
    fn a_budget_never_changes_an_answer() {
        let mut code = [0u8; 4096];
        let mut exhausted = 0;
        for &(pattern, flags) in PATTERNS {
            let (length, count) = generate_for(pattern, flags, &mut code).expect("generated");
            let emitted = &code[..length];
            for &subject in SUBJECTS {
                let full = execute(emitted, count, subject, 0, u64::MAX).expect("runs");
                let mut decided = false;
                for budget in 0..12 {
                    let context = || Case {
                        pattern,
                        flags,
                        subject,
                        start: 0,
                        budget,
                    };
                    let run = execute(emitted, count, subject, 0, budget)
                        .unwrap_or_else(|fault| panic!("{}: {fault}", context()));
                    if run.answer == EXHAUSTED as i64 {
                        assert!(!decided, "{}: a smaller budget decided this", context());
                        exhausted += 1;
                        continue;
                    }
                    decided = true;
                    assert_eq!(run.answer, full.answer, "{}", context());
                    if run.answer >= 0 {
                        assert_eq!(
                            run.registers[..count],
                            full.registers[..count],
                            "{}",
                            context()
                        );
                    }
                }
            }
        }
        assert!(exhausted > 0, "no budget was small enough to stop anything");
    }

    #[test]
    fn a_budget_stops_what_backtracking_would_not() {
        let mut code = [0u8; 4096];
        let (length, count) = generate_for("[ab]*[ac]*a*x", "", &mut code).expect("generated");
        let emitted = &code[..length];
        let (found, _) = interpret("[ab]*[ac]*a*x", "", RUN, 0);
        let full = execute(emitted, count, RUN, 0, u64::MAX).expect("runs to completion");
        assert!(!found);
        assert_eq!(full.answer, NO_MATCH as i64);
        // What that took, against what a small allowance is allowed to take.
        let stopped = execute(emitted, count, RUN, 0, 1000).expect("stops within its bound");
        assert_eq!(stopped.answer, EXHAUSTED as i64);
        assert!(stopped.steps <= 1001 * length / 4);
        assert!(
            full.steps > 20 * stopped.steps,
            "{} steps to finish, {} to stop",
            full.steps,
            stopped.steps
        );
    }

    /// The verifier's promises, checked against execution rather than argued.
    /// Bits are flipped in generated code, and every mutant the verifier
    /// accepts is run: whatever it now computes, it must load, store, return
    /// and finish within what was proved.
    #[test]
    fn code_the_verifier_accepts_keeps_its_promises() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let subjects = ["", "a", "abc", "aaaa", "user@example", "0123456789abc"];
        let mut code = [0u8; 4096];
        let mut facts = [Facts::default(); 1024];
        let (mut accepted, mut refused, mut different) = (0usize, 0usize, 0usize);
        for &(pattern, flags) in PATTERNS {
            let (length, count) = generate_for(pattern, flags, &mut code).expect("generated");
            for _ in 0..MUTANTS {
                let mut mutant = code;
                for _ in 0..1 + random() % 2 {
                    let bit = random() as usize % (length * 8);
                    mutant[bit / 8] ^= 1 << (bit % 8);
                }
                let mutant = &mutant[..length];
                if verify(mutant, count, &mut facts).is_err() {
                    refused += 1;
                    continue;
                }
                accepted += 1;
                let mut differs = false;
                for subject in subjects {
                    for start in [0, 1, subject.len() as u64, u64::MAX] {
                        for budget in [0, 5, 200] {
                            let context = || Case {
                                pattern,
                                flags,
                                subject,
                                start,
                                budget,
                            };
                            let run = execute(mutant, count, subject, start, budget)
                                .unwrap_or_else(|fault| panic!("{}: {fault}", context()));
                            let original = execute(&code[..length], count, subject, start, budget)
                                .expect("the original runs");
                            differs |= run.answer != original.answer;
                            assert!(
                                run.answer == NO_MATCH as i64
                                    || run.answer == EXHAUSTED as i64
                                    || (0..=subject.len() as i64).contains(&run.answer),
                                "mutated, {}: returned {}",
                                context(),
                                run.answer
                            );
                            for &value in &run.registers[..count] {
                                assert!(
                                    value == u64::MAX || value <= subject.len() as u64,
                                    "{}: stored {value}",
                                    context()
                                );
                            }
                        }
                    }
                }
                different += usize::from(differs);
            }
        }
        // A test that only ever ran refused or unchanged code would prove
        // nothing, so it has to have run mutants that behave differently.
        assert!(
            refused > 0 && accepted > 0 && different > 0,
            "{accepted} accepted, {refused} refused, {different} different"
        );
    }

    const MUTANTS: usize = 100;

    #[test]
    fn refuses_what_it_cannot_generate() {
        let mut code = [0u8; 4096];
        let mut facts = [Facts::default(); 1024];
        for (pattern, flags) in [
            ("a+?", ""),
            ("a|b", ""),
            ("[a-z]", "i"),
            ("a", "y"),
            ("^a", "m"),
            ("a$", "m"),
            ("\\ba", ""),
            // Classes reaching past ASCII, which this storage cannot hold but
            // the table would have to describe.
            (r"\W", ""),
            (r"\s", ""),
            (r"\W+", ""),
            ("\\p{L}", "u"),
        ] {
            let source = Input::utf8(pattern);
            let mut nodes = [Node::default(); 256];
            let mut ranges = [Range::default(); 512];
            let mut words = [0u32; 2048];
            let mut budget = Budget::new(10_000_000);
            let program = compile(
                source,
                flags,
                &mut nodes,
                &mut ranges,
                &mut words,
                &mut budget,
            )
            .unwrap();
            assert_eq!(
                emit_search(program, &mut code, &mut facts),
                Err(EmitError::Unsupported),
                "/{pattern}/{flags}"
            );
        }
    }
}
