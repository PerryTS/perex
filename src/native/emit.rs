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

use super::a64::{Assembler, Cond, EncodeError, Label, Patch, Reg, X0, X1, X2, X3};
use crate::program::{
    ANY, ANY_S, ATOM_REPEAT, CHAR, CHAR_I, CLASS, MATCH, NEGATED, PROPERTY, Program, SAVE, Y,
    consuming,
};

/// Where the arguments arrive. The first four follow the C calling convention
/// this targets; the rest are scratch it may use freely.
const SUBJECT: Reg = X0;
const LENGTH: Reg = X1;
const START: Reg = X2;
const REGISTERS: Reg = X3;
/// The position inside the current attempt.
const AT: Reg = Reg(6);
/// The byte just loaded from the subject.
const BYTE: Reg = Reg(7);
/// Scratch within a single step.
const TMP: Reg = Reg(8);
const TMP2: Reg = Reg(15);
/// The membership table a repeated multi-range class is tested through, held
/// across that repeat's scan.
const TABLE: Reg = Reg(5);

/// Open repeats keep two registers each: where the run currently ends, and the
/// earliest end its minimum allows. Three is what X9 through X14 hold, and
/// three sequential repeats is more than the measured cases use.
const MAX_REPEATS: usize = 3;

fn repeat_end(depth: usize) -> Reg {
    Reg(9 + depth as u8 * 2)
}
fn repeat_floor(depth: usize) -> Reg {
    Reg(10 + depth as u8 * 2)
}

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
            SAVE => pc += 1,
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
        into: Reg,
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
        self.at[self.count] = asm.adr_forward(into);
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
    table: Option<Reg>,
    fail: &mut Patches,
) -> Result<(), EmitError> {
    match op {
        // Anything at all, so nothing to test.
        ANY_S => {}
        CHAR | CHAR_I => {
            asm.ldrb(BYTE, SUBJECT, AT);
            let byte = a as u8;
            if op == CHAR_I && byte.is_ascii_alphabetic() {
                // Either case, which over this storage is the whole of what
                // folding means: the two non-ASCII characters that fold into
                // ASCII cannot occur in it.
                asm.cmp_imm32(BYTE, u32::from(byte.to_ascii_lowercase()));
                let matched = asm.b_cond_forward(Cond::Eq);
                asm.cmp_imm32(BYTE, u32::from(byte.to_ascii_uppercase()));
                fail.push(asm.b_cond_forward(Cond::Ne))?;
                asm.bind(matched);
            } else {
                asm.cmp_imm32(BYTE, u32::from(byte));
                fail.push(asm.b_cond_forward(Cond::Ne))?;
            }
        }
        // Anything but a line terminator. On ASCII storage only the two
        // single-byte terminators can occur; U+2028 and U+2029 are not
        // representable in it.
        ANY => {
            asm.ldrb(BYTE, SUBJECT, AT);
            asm.cmp_imm32(BYTE, u32::from(b'\n'));
            fail.push(asm.b_cond_forward(Cond::Eq))?;
            asm.cmp_imm32(BYTE, u32::from(b'\r'));
            fail.push(asm.b_cond_forward(Cond::Eq))?;
        }
        CLASS => {
            asm.ldrb(BYTE, SUBJECT, AT);
            let count = b & !NEGATED;
            let negated = b & NEGATED != 0;
            if let Some(table) = table {
                // One load, whatever the class holds.
                asm.ldrb(BYTE, table, BYTE);
                asm.cmp_imm32(BYTE, 0);
                fail.push(asm.b_cond_forward(Cond::Eq))?;
            } else if count == 1 {
                // A byte is inside one range exactly when subtracting the low
                // bound leaves something no larger than the range is wide,
                // which is one subtraction and one comparison rather than two
                // comparisons and two branches.
                let [lo, hi] = program.range(a as usize);
                asm.sub_imm32(TMP2, BYTE, lo);
                asm.cmp_imm32(TMP2, hi - lo);
                let outside = if negated { Cond::Ls } else { Cond::Hi };
                fail.push(asm.b_cond_forward(outside))?;
            } else {
                // Each range jumps out when the byte is inside it. Falling past
                // all of them means the byte is inside none.
                let mut inside = [Patch::default(); MAX_RANGES];
                for (slot, index) in inside.iter_mut().zip(a..a + count) {
                    let [lo, hi] = program.range(index as usize);
                    asm.cmp_imm32(BYTE, lo);
                    let below = asm.b_cond_forward(Cond::Lo);
                    asm.cmp_imm32(BYTE, hi);
                    *slot = asm.b_cond_forward(Cond::Ls);
                    asm.bind(below);
                }
                if negated {
                    let accepted = asm.b_forward();
                    for patch in &inside[..count as usize] {
                        asm.bind(*patch);
                    }
                    fail.push(asm.b_forward())?;
                    asm.bind(accepted);
                } else {
                    fail.push(asm.b_forward())?;
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

/// Emit a whole search for `program` into `code`, returning its byte length.
///
/// The generated code takes the subject, its length, the start position and the
/// register array, and returns the position a match began at or `-1`. It writes
/// only into the registers it was given, reads only within the length, and
/// calls nothing.
pub fn emit_search(program: Program<'_>, code: &mut [u8]) -> Result<usize, EmitError> {
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
    let mut repeats: [Option<Repeat>; MAX_REPEATS] = [None, None, None];
    let mut depth = 0usize;

    let outer = asm.here();
    // No room for what every match must consume means no room at any later
    // start either, so the same test ends the search.
    asm.add_imm(AT, START, least as u32);
    asm.cmp(AT, LENGTH);
    let exhausted = asm.b_cond_forward(Cond::Hi);
    asm.mov(AT, START);

    let mut pc = 0;
    loop {
        let [op, a, b] = program.instruction(pc);
        match op {
            SAVE => {
                asm.str_index(AT, REGISTERS, a);
                pc += 1;
            }
            MATCH => {
                asm.mov(X0, START);
                asm.ret();
                break;
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
                asm.add_imm(floor, AT, fewest);
                let after = least_from(program, record[4] as usize);
                if after >= 1 << 12 {
                    return Err(EmitError::TooLarge);
                }
                asm.sub_imm(TMP, LENGTH, after as u32);
                if !infinite {
                    asm.add_imm(TMP2, AT, most);
                    asm.cmp(TMP2, TMP);
                    let room = asm.b_cond_forward(Cond::Hs);
                    asm.mov(TMP, TMP2);
                    asm.bind(room);
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
                asm.cmp(AT, TMP);
                stop.push(asm.b_cond_forward(Cond::Hs))?;
                emit_atom(
                    &mut asm, program, body_op, body_a, body_b, scan_table, &mut stop,
                )?;
                asm.add_imm(AT, AT, 1);
                asm.b_back(scan);
                stop.bind(&mut asm);

                // Short of the minimum is failure, and retreating cannot help.
                asm.mov(end, AT);
                asm.cmp(end, floor);
                let short = asm.b_cond_forward(Cond::Lo);
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
                asm.mov(AT, end);
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
                asm.add_imm(AT, AT, 1);
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
        asm.cmp(end, floor);
        let spent = asm.b_cond_forward(Cond::Ls);
        asm.sub_imm(end, end, 1);
        asm.b_back(repeat.retry);
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
    asm.add_imm(START, START, 1);
    asm.b_back(outer);

    asm.bind(exhausted);
    asm.movn(X0, 0);
    asm.ret();
    tables.place(&mut asm);
    asm.finish().map_err(EmitError::from)
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
    /// with it whatever either of them did.
    /// A table lives in the code buffer, so a base address says which of the
    /// two regions a load reads. Real hardware needs no such tag; this is the
    /// emulator standing in for one address space.
    const CODE_BASE: u64 = 1 << 40;

    struct Machine<'a> {
        x: [u64; 32],
        z: bool,
        c: bool,
        subject: &'a [u8],
        code: &'a [u8],
        registers: &'a mut [u64],
    }

    impl Machine<'_> {
        fn read(&self, index: u32) -> u64 {
            if index == 31 {
                0
            } else {
                self.x[index as usize]
            }
        }
        fn write(&mut self, index: u32, value: u64) {
            if index != 31 {
                self.x[index as usize] = value;
            }
        }
        fn flags(&mut self, a: u64, b: u64) {
            self.z = a == b;
            self.c = a >= b;
        }
        /// Read one byte from whichever region the address names.
        fn load(&self, at: u64) -> u8 {
            if at >= CODE_BASE {
                *self
                    .code
                    .get((at - CODE_BASE) as usize)
                    .expect("load inside the code")
            } else {
                *self
                    .subject
                    .get(at as usize)
                    .expect("load inside the subject")
            }
        }
        fn holds(&self, cond: u32) -> bool {
            match cond {
                0 => self.z,
                1 => !self.z,
                2 => self.c,
                3 => !self.c,
                8 => self.c && !self.z,
                9 => !self.c || self.z,
                other => panic!("condition {other} is not one the emitter uses"),
            }
        }

        /// Run until `RET`, returning X0. Bounded so a generator bug is a
        /// failing test rather than a hang.
        fn run(&mut self, code: &[u8]) -> i64 {
            let mut pc = 0usize;
            for _ in 0..1_000_000 {
                let bytes = &code[pc..pc + 4];
                let w = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                pc += 4;
                let (rd, rn, rm) = (w & 31, (w >> 5) & 31, (w >> 16) & 31);
                let imm12 = (w >> 10) & 0xfff;
                if w & 0xffff_fc1f == 0xd65f_0000 {
                    return self.x[0] as i64;
                } else if w & 0xff80_0000 == 0xd280_0000 {
                    self.write(rd, u64::from((w >> 5) & 0xffff));
                } else if w & 0xff80_0000 == 0x9280_0000 {
                    self.write(rd, !u64::from((w >> 5) & 0xffff));
                } else if w & 0xffe0_ffe0 == 0xaa00_03e0 {
                    let value = self.read(rm);
                    self.write(rd, value);
                } else if w & 0xff80_001f == 0x7100_001f {
                    let value = self.read(rn) as u32;
                    self.flags(u64::from(value), u64::from(imm12));
                } else if w & 0xff80_0000 == 0x9100_0000 {
                    let value = self.read(rn).wrapping_add(u64::from(imm12));
                    self.write(rd, value);
                } else if w & 0xff80_0000 == 0xd100_0000 {
                    let value = self.read(rn).wrapping_sub(u64::from(imm12));
                    self.write(rd, value);
                } else if w & 0xffe0_fc1f == 0xeb00_001f {
                    let (a, b) = (self.read(rn), self.read(rm));
                    self.flags(a, b);
                } else if w & 0xffe0_fc00 == 0x3860_6800 {
                    let at = self.read(rn).wrapping_add(self.read(rm));
                    self.write(rd, u64::from(self.load(at)));
                } else if w & 0xffc0_0000 == 0x3940_0000 {
                    let at = self.read(rn).wrapping_add(u64::from(imm12));
                    self.write(rd, u64::from(self.load(at)));
                } else if w & 0xff80_0000 == 0x5100_0000 {
                    let value = (self.read(rn) as u32).wrapping_sub(imm12);
                    self.write(rd, u64::from(value));
                } else if w & 0x8b20_0000 == 0x8b00_0000 && w & 0x7fe0_fc00 == 0x0b00_0000 {
                    let value = self.read(rn).wrapping_add(self.read(rm));
                    self.write(rd, value);
                } else if w & 0x9f00_0000 == 0x1000_0000 {
                    let immlo = (w >> 29) & 3;
                    let immhi = (w >> 5) & 0x7_ffff;
                    let offset = (((immhi << 2) | immlo) as i32) << 11 >> 11;
                    let at = (pc as i64 - 4 + i64::from(offset)) as u64;
                    self.write(rd, CODE_BASE + at);
                } else if w & 0xffc0_0000 == 0xf900_0000 {
                    let slot = (self.read(rn) as usize) / 8 + imm12 as usize;
                    let value = self.read(rd);
                    *self
                        .registers
                        .get_mut(slot)
                        .expect("store inside registers") = value;
                } else if w & 0xff00_0010 == 0x5400_0000 {
                    let offset = ((w >> 5) & 0x7_ffff) as i32;
                    let offset = (offset << 13) >> 13;
                    if self.holds(w & 15) {
                        pc = (pc as i64 - 4 + i64::from(offset) * 4) as usize;
                    }
                } else if w & 0xfc00_0000 == 0x1400_0000 {
                    let offset = (w & 0x3ff_ffff) as i32;
                    let offset = (offset << 6) >> 6;
                    pc = (pc as i64 - 4 + i64::from(offset) * 4) as usize;
                } else {
                    panic!("emitted an instruction the emulator does not decode: {w:08x}");
                }
            }
            panic!("generated code did not return");
        }
    }

    /// What the generated code says about one subject: where a match began, and
    /// the registers it wrote.
    fn emulate(pattern: &str, flags: &str, subject: &str) -> Option<(i64, [u64; 16])> {
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
        let mut code = [0u8; 4096];
        let length = emit_search(program, &mut code).ok()?;
        let mut registers = [u64::MAX; 16];
        let emitted = &code[..length];
        let mut machine = Machine {
            x: [0; 32],
            z: false,
            c: false,
            subject: subject.as_bytes(),
            code: emitted,
            registers: &mut registers,
        };
        machine.x[1] = subject.len() as u64;
        let answer = machine.run(emitted);
        Some((answer, registers))
    }

    /// The same search through the interpreter, which is what a pattern means.
    fn interpret(pattern: &str, flags: &str, subject: &str) -> (bool, [Option<Span>; 8]) {
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
        let mut budget = Budget::new(10_000_000);
        let found = find(
            program,
            Input::utf8(subject),
            0,
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

    #[test]
    fn generated_code_agrees_with_the_interpreter() {
        let patterns = [
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
        ];
        let subjects = [
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
        ];
        for (pattern, flags) in patterns {
            for subject in subjects {
                let Some((answer, registers)) = emulate(pattern, flags, subject) else {
                    panic!("/{pattern}/{flags} should have code generated for it");
                };
                let (found, captures) = interpret(pattern, flags, subject);
                assert_eq!(
                    answer >= 0,
                    found,
                    "/{pattern}/{flags} against {subject:?}: compiled said {answer}, \
                     interpreted said {found}"
                );
                if !found {
                    continue;
                }
                let span = captures[0].expect("a match has a span");
                assert_eq!(
                    answer as usize,
                    span.start(),
                    "/{pattern}/{flags} against {subject:?}: start"
                );
                for (index, capture) in captures.iter().enumerate() {
                    let Some(span) = capture else { continue };
                    assert_eq!(
                        registers[index * 2] as usize,
                        span.start(),
                        "/{pattern}/{flags} against {subject:?}: capture {index} start"
                    );
                    assert_eq!(
                        registers[index * 2 + 1] as usize,
                        span.end(),
                        "/{pattern}/{flags} against {subject:?}: capture {index} end"
                    );
                }
            }
        }
    }

    #[test]
    fn refuses_what_it_cannot_generate() {
        let mut code = [0u8; 4096];
        for (pattern, flags) in [
            ("a+?", ""),
            ("a|b", ""),
            ("[a-z]", "i"),
            ("a", "y"),
            ("^a", ""),
            ("(?<=a)b", ""),
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
                emit_search(program, &mut code),
                Err(EmitError::Unsupported),
                "/{pattern}/{flags}"
            );
        }
    }
}
