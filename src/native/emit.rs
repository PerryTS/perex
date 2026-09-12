//! Generating a whole search for a straight-line program, in the instructions
//! [`super::a64`] encodes. See `docs/compilation.md`.
//!
//! A straight-line program consumes a fixed sequence of characters and cannot
//! fail part-way and try something else at the same start: no repeat, no
//! branch, no assertion. That is the subset worth doing first, because it needs
//! none of the backtracking machinery and already covers most of the cases
//! measured behind V8.
//!
//! The generated code owns the whole search, not one attempt. Compiling only
//! the attempt would leave the per-start phase machinery in place, and
//! `docs/performance.md` measures that to be most of what the short cases cost.
#![allow(dead_code)]

use super::a64::{Assembler, Cond, EncodeError, Patch, Reg, X0, X1, X2, X3};
use crate::program::{CHAR, CHAR_I, MATCH, Program, SAVE, Y};

/// Where the arguments arrive, and what the generated code keeps in each
/// register. The first six follow the C calling convention this targets.
const SUBJECT: Reg = X0;
const LENGTH: Reg = X1;
const START: Reg = X2;
const REGISTERS: Reg = X3;
/// The position inside the current attempt.
const AT: Reg = Reg(6);
/// The byte just loaded from the subject.
const BYTE: Reg = Reg(7);

/// Failure branches waiting for the "try the next start" label. A straight-line
/// program emits at most two per character, and a program with more characters
/// than this is not one worth compiling.
const MAX_PATCHES: usize = 256;

/// Why a program could not have code generated for it. Distinct from
/// [`EncodeError`], which is about instructions rather than programs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EmitError {
    /// The program is outside the subset this generator handles. The
    /// interpreter runs it, as it runs everything.
    Unsupported,
    /// The program needed more failure branches than the generator carries.
    TooLarge,
    /// The instructions could not be encoded.
    Encode(EncodeError),
}

impl From<EncodeError> for EmitError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

/// Whether this program consumes a fixed sequence of characters, with capture
/// bookkeeping around them and nothing else.
pub(crate) fn straight_line(program: Program<'_>) -> bool {
    // A sticky search tries only its requested position, which is a different
    // search from the one generated here.
    if program.words()[2] & Y != 0 {
        return false;
    }
    (0..program.instructions()).all(|pc| {
        let [op, a, _] = program.instruction(pc);
        match op {
            MATCH | SAVE => true,
            CHAR | CHAR_I => a < 128,
            _ => false,
        }
    })
}

/// Emit a whole search for `program` into `code`, returning its byte length.
///
/// The generated code takes the subject, its length, the start position, the
/// register array and its count, and returns the position a match began at or
/// `-1`. It writes only into the registers it was given, reads only within the
/// length, and calls nothing.
///
/// Registers are not cleared here: every `SAVE` a straight-line program holds
/// runs on the path that reports a match, so each one it names is written. The
/// caller clears any others, exactly as it does for the interpreter.
pub(crate) fn emit_search(program: Program<'_>, code: &mut [u8]) -> Result<usize, EmitError> {
    if !straight_line(program) {
        return Err(EmitError::Unsupported);
    }
    let consumed = (0..program.instructions())
        .filter(|&pc| matches!(program.instruction(pc)[0], CHAR | CHAR_I))
        .count();
    if consumed >= 1 << 12 {
        return Err(EmitError::TooLarge);
    }

    let mut asm = Assembler::new(code);
    let mut patches = [Patch::default(); MAX_PATCHES];
    let mut waiting = 0;

    let outer = asm.here();
    // No room for the characters this must consume means no room at any later
    // start either, so the same test ends the search.
    asm.add_imm(AT, START, consumed as u32);
    asm.cmp(AT, LENGTH);
    let exhausted = asm.b_cond_forward(Cond::Hi);

    asm.mov(AT, START);
    for pc in 0..program.instructions() {
        let [op, a, _] = program.instruction(pc);
        match op {
            SAVE => asm.str_index(AT, REGISTERS, a),
            CHAR | CHAR_I => {
                asm.ldrb(BYTE, SUBJECT, AT);
                let byte = a as u8;
                let folded = op == CHAR_I && byte.is_ascii_alphabetic();
                if folded {
                    // Either case, which over this storage is the whole of what
                    // folding means: the two non-ASCII characters that fold into
                    // ASCII cannot occur in it.
                    asm.cmp_imm32(BYTE, u32::from(byte.to_ascii_lowercase()));
                    let matched = asm.b_cond_forward(Cond::Eq);
                    asm.cmp_imm32(BYTE, u32::from(byte.to_ascii_uppercase()));
                    if waiting == MAX_PATCHES {
                        return Err(EmitError::TooLarge);
                    }
                    patches[waiting] = asm.b_cond_forward(Cond::Ne);
                    waiting += 1;
                    asm.bind(matched);
                } else {
                    asm.cmp_imm32(BYTE, u32::from(byte));
                    if waiting == MAX_PATCHES {
                        return Err(EmitError::TooLarge);
                    }
                    patches[waiting] = asm.b_cond_forward(Cond::Ne);
                    waiting += 1;
                }
                asm.add_imm(AT, AT, 1);
            }
            MATCH => {
                asm.mov(X0, START);
                asm.ret();
            }
            // `straight_line` admitted every instruction, so nothing else can
            // appear; treating it as unsupported rather than skipping it keeps
            // the two in step if one of them changes.
            _ => return Err(EmitError::Unsupported),
        }
    }

    // A character did not match. The next start is one position on, and the
    // bound above is what ends the loop.
    for patch in &patches[..waiting] {
        asm.bind(*patch);
    }
    asm.add_imm(START, START, 1);
    asm.b_back(outer);

    asm.bind(exhausted);
    asm.movn(X0, 0);
    asm.ret();
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
    struct Machine<'a> {
        x: [u64; 32],
        z: bool,
        c: bool,
        subject: &'a [u8],
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
                    let at = self.read(rn).wrapping_add(self.read(rm)) as usize;
                    let byte = *self.subject.get(at).expect("load inside the subject");
                    self.write(rd, u64::from(byte));
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
        let mut machine = Machine {
            x: [0; 32],
            z: false,
            c: false,
            subject: subject.as_bytes(),
            registers: &mut registers,
        };
        machine.x[1] = subject.len() as u64;
        let answer = machine.run(&code[..length]);
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
        for (pattern, flags) in [("a+", ""), ("a|b", ""), ("[a-z]", ""), ("a", "y")] {
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
