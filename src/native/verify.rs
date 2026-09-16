//! Checking generated code before anything executes it. See
//! `docs/compilation.md`.
//!
//! The emitter is tested; this proves. It decodes the bytes it is given rather
//! than trusting the emitter's idea of them, follows every path through them,
//! and refuses code for which it cannot show all of the following for every
//! subject, start, budget and register contents:
//!
//! - every load reads inside the subject, or inside the code buffer itself;
//! - every store writes one of the program's registers, and what it writes is a
//!   position no further than the subject's length;
//! - every branch lands on an instruction inside the code, and execution never
//!   runs off its end;
//! - it returns through the link register with a position or one of the two
//!   codes, and writes no register the calling convention preserves;
//! - every backward branch spends one unit of budget and is not taken once none
//!   is left, so the code executes at most `(budget + 1) * instructions`
//!   instructions whatever the pattern and subject are.
//!
//! It does not show that the answer is right. The interpreter defines that, and
//! the differential tests hold the generated code to it.
//!
//! Two targets are checked by one analysis. What differs is the decoder and
//! what the calling convention says: AArch64 takes its arguments in `X0`-`X4`
//! and may write `X0`-`X17` freely, while x86-64 under the System V convention
//! takes them in `RDI`, `RSI`, `RDX`, `RCX` and `R8` and must give `RBX`, `RBP`
//! and `R12`-`R15` back as it found them. Generated code does that by saving
//! them on entry and restoring them before each return, and this follows the
//! saving rather than assuming it: what was pushed is what may be popped, the
//! stack is where it started at every return, and every preserved register
//! holds what it held. A branch that skips the restore is refused for that
//! reason.
//!
//! What it assumes is the host's half of the call: the subject register
//! addresses `length` readable bytes, and the length is a slice length and so
//! at most `isize::MAX`; the register argument addresses `register_count`
//! writable 64-bit slots; the code is mapped as exactly the bytes verified; and
//! it is entered at its first byte, returning through the link register on
//! AArch64 and the stack on x86-64. The start and the budget may hold anything.
//!
//! The analysis is abstract interpretation over caller-owned [`Facts`], one per
//! [`Target::stride`] bytes of code — one per instruction on AArch64, one per
//! byte on x86-64, whose instructions vary in length — so it allocates nothing
//! and its work is bounded by the code's length. A branch into the middle of an
//! instruction is followed, and what the processor would decode from there is
//! what is checked; bytes reached both as an instruction and as part of one are
//! refused, because that is two instruction streams where this checks one.

use core::cmp::{max, min};

/// Registers the analysis follows: `X0` through `X17`, which are the ones a
/// leaf function may overwrite. Writing any other is refused.
const TRACKED: usize = 18;
/// The budget register.
/// Updates to one instruction's facts after which any fact that still changes
/// is given up on, so loops the analysis cannot bound still finish.
const WIDEN: u16 = 32;

/// Why code was refused, and the byte offset of the instruction that failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifyError {
    pub at: usize,
    pub reason: Reason,
}

/// What could not be shown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    /// Fewer [`Facts`] than the code has instructions.
    Scratch,
    /// No code at all, or a reachable word that is not one of the instructions
    /// this accepts.
    Undecodable,
    /// A load not shown to be inside the subject or the code.
    Load,
    /// A store not shown to be into one of the program's registers.
    Store,
    /// A store of a value not shown to be a position within the subject.
    Stored,
    /// A write to a register the calling convention preserves.
    Written,
    /// A 32-bit comparison of a value not shown to fit in 32 bits.
    Compare,
    /// A branch whose target is outside the code.
    Branch,
    /// Execution can continue past the last instruction.
    FallsOffEnd,
    /// A return whose value is not a position or one of the two codes.
    Return,
    /// A backward branch that does not spend budget, a write to the budget
    /// outside that, or a branch into the middle of it.
    Budget,
}

/// What the analysis knows about one register's value.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Value {
    #[default]
    Top,
    /// Exactly this.
    Small(u32),
    /// Exactly `!n`, which is how `-1` and `-2` are loaded.
    Negative(u16),
    /// At most 255.
    Byte,
    /// Less than 2^32.
    Fits32,
    /// Exactly the subject address.
    Subject,
    /// The subject address plus `t`, where `t` is as a `Pos` with these bounds.
    SubjectAt { low: u32, slack: i32 },
    /// Exactly the subject's length.
    Length,
    /// Some `v` with `low <= v` and `v + slack <= length`.
    Pos { low: u32, slack: i32 },
    /// Exactly the register array's address.
    Registers,
    /// Exactly the code's address plus this many bytes.
    Code(u32),
    /// Somewhere in `[code + at, code + at + width)`.
    CodeSpan { at: u32, width: u32 },
}

/// What the last comparison compared, while neither side has been overwritten.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Flags {
    #[default]
    Unknown,
    Registers(u8, u8),
    Immediate(u8, u16),
}

/// Registers a target's convention preserves, which a prologue saves and an
/// epilogue restores. Deeper than any generated code needs, so that a mutant
/// which pushes more is refused rather than tracked.
const SAVED: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct State {
    x: [Value; TRACKED],
    flags: Flags,
    /// A lower bound on the subject's length.
    length_low: u32,
    /// What has been pushed and not yet popped, innermost last, and how much
    /// of it there is. `None` is a stack this could not follow, which no
    /// return may happen on.
    stack: Option<([u8; SAVED], u8)>,
    /// Registers known to hold what they held on entry: every one at the
    /// start, cleared by writing one and set again by popping it back.
    restored: u16,
}

impl Default for State {
    fn default() -> Self {
        State {
            x: [Value::default(); TRACKED],
            flags: Flags::default(),
            length_low: 0,
            stack: Some(([0; SAVED], 0)),
            restored: u16::MAX,
        }
    }
}

/// What the verifier knows at one instruction. Scratch, one per instruction.
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts {
    state: State,
    reached: bool,
    pending: bool,
    updates: u16,
    /// Set on the second and third instructions of a budget guard: the first
    /// may write the budget, and nothing may branch to either.
    guard: bool,
    /// Set on a byte inside a reached instruction rather than at its start.
    /// Reaching one would mean two decodings of the same bytes, which is two
    /// instruction streams where this checks one.
    interior: bool,
}

/// A condition, as the analysis means it rather than as a target spells it.
/// Every comparison generated code makes is unsigned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Cond {
    Equal,
    NotEqual,
    /// Below, unsigned.
    Less,
    /// Above or equal, unsigned.
    AtLeast,
    /// Above, unsigned.
    Greater,
    /// Below or equal, unsigned.
    AtMost,
}

impl Cond {
    /// The condition that holds exactly when this one does not.
    fn inverse(self) -> Self {
        match self {
            Cond::Equal => Cond::NotEqual,
            Cond::NotEqual => Cond::Equal,
            Cond::Less => Cond::AtLeast,
            Cond::AtLeast => Cond::Less,
            Cond::Greater => Cond::AtMost,
            Cond::AtMost => Cond::Greater,
        }
    }
}

/// The instructions accepted, decoded into what they do. A register a target
/// refuses as an operand — AArch64's register 31, which is the stack pointer or
/// the zero register depending on the instruction, or x86-64's stack pointer —
/// makes the whole instruction undecodable rather than appearing here.
///
/// Branch targets are absolute byte offsets into the code: the decoder resolves
/// them, so the analysis never repeats a target's own arithmetic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Insn {
    Ret,
    /// A constant, which is how the two codes a search returns are written.
    MovImm {
        rd: u8,
        value: i64,
    },
    Mov {
        rd: u8,
        rm: u8,
    },
    /// A copy of a low half, which leaves the rest zero.
    Mov32 {
        rd: u8,
        rm: u8,
    },
    AddImm {
        rd: u8,
        rn: u8,
        imm: u32,
    },
    SubImm {
        rd: u8,
        rn: u8,
        imm: u32,
    },
    SubImm32 {
        rd: u8,
        rn: u8,
        imm: u32,
    },
    AddReg {
        rd: u8,
        rn: u8,
        rm: u8,
    },
    CmpImm {
        rn: u8,
        imm: u32,
    },
    CmpImm32 {
        rn: u8,
        imm: u32,
    },
    CmpReg {
        rn: u8,
        rm: u8,
    },
    /// Compare a register with zero, without writing anything.
    Test {
        rn: u8,
    },
    /// One less, which is what spends a unit of budget.
    Dec {
        rd: u8,
    },
    /// One byte, zero-extended, from `base + index + disp`.
    Load {
        rt: u8,
        base: u8,
        index: Option<u8>,
        disp: i64,
    },
    /// Eight bytes from `base + index`, which a scan reads at once.
    LoadWord {
        rt: u8,
        base: u8,
        index: u8,
    },
    /// Something this follows only as far as which register it writes: the
    /// arithmetic a scan does over eight bytes at a time, whose result is data
    /// rather than a position or an address, and whose flags nothing may be
    /// refined from. `reads` names its other operands, so that a register a
    /// target refuses is refused here too; an unused one repeats `rd`.
    Compute {
        rd: u8,
        reads: [u8; 2],
    },
    /// A whole register into `base + disp`.
    Store {
        rt: u8,
        base: u8,
        disp: u32,
    },
    /// The address of a byte of the code itself.
    Address {
        rd: u8,
        at: u32,
    },
    Branch {
        cond: Option<Cond>,
        to: usize,
    },
    /// Branch when a register is zero, which AArch64 does in one instruction.
    BranchIfZero {
        rt: u8,
        to: usize,
    },
    /// Save and restore, which only a prologue and an epilogue may hold.
    Push {
        r: u8,
    },
    Pop {
        r: u8,
    },
}

fn sign_extend(value: u32, bits: u32) -> i32 {
    ((value << (32 - bits)) as i32) >> (32 - bits)
}

/// Decode one AArch64 word, exactly: every bit outside an instruction's
/// operand fields must be the value this accepts, so a word that would execute
/// as something else is not decoded as this. `at` is the word's own byte
/// offset, which resolves the branches and the code addresses.
fn decode_a64(code: &[u8], at: usize) -> Option<(Insn, usize)> {
    let w = u32::from_le_bytes([
        *code.get(at)?,
        *code.get(at + 1)?,
        *code.get(at + 2)?,
        *code.get(at + 3)?,
    ]);
    let rd = (w & 31) as u8;
    let rn = ((w >> 5) & 31) as u8;
    let rm = ((w >> 16) & 31) as u8;
    let imm12 = (w >> 10) & 0xfff;
    let away = |offset: i32| at.checked_add_signed(offset as isize * 4);
    let insn = if w == 0xd65f_03c0 {
        Insn::Ret
    } else if w & 0xffe0_0000 == 0xd280_0000 {
        Insn::MovImm {
            rd,
            value: i64::from((w >> 5) as u16),
        }
    } else if w & 0xffe0_0000 == 0x9280_0000 {
        Insn::MovImm {
            rd,
            value: !i64::from((w >> 5) as u16),
        }
    } else if w & 0xffe0_ffe0 == 0xaa00_03e0 {
        Insn::Mov { rd, rm }
    } else if w & 0xffc0_001f == 0xf100_001f {
        Insn::CmpImm { rn, imm: imm12 }
    } else if w & 0xffc0_001f == 0x7100_001f {
        Insn::CmpImm32 { rn, imm: imm12 }
    } else if w & 0xffc0_0000 == 0x9100_0000 {
        Insn::AddImm { rd, rn, imm: imm12 }
    } else if w & 0xffc0_0000 == 0xd100_0000 {
        Insn::SubImm { rd, rn, imm: imm12 }
    } else if w & 0xffc0_0000 == 0x5100_0000 {
        Insn::SubImm32 { rd, rn, imm: imm12 }
    } else if w & 0xffe0_fc1f == 0xeb00_001f {
        Insn::CmpReg { rn, rm }
    } else if w & 0xffe0_fc00 == 0x8b00_0000 {
        Insn::AddReg { rd, rn, rm }
    } else if w & 0xffe0_fc00 == 0x3860_6800 {
        Insn::Load {
            rt: rd,
            base: rn,
            index: Some(rm),
            disp: 0,
        }
    } else if w & 0xffc0_0000 == 0x3940_0000 {
        Insn::Load {
            rt: rd,
            base: rn,
            index: None,
            disp: i64::from(imm12),
        }
    } else if w & 0xffe0_fc00 == 0xf860_6800 {
        Insn::LoadWord {
            rt: rd,
            base: rn,
            index: rm,
        }
    } else if w & 0xffe0_fc00 == 0xca00_0000
        || w & 0xffe0_fc00 == 0xcb00_0000
        || w & 0xffe0_fc00 == 0x8a20_0000
    {
        // Exclusive or, subtraction and bit clear, over whole registers.
        Insn::Compute {
            rd,
            reads: [rn, rm],
        }
    } else if w & 0xffff_fc00 == 0xdac0_0000
        || w & 0xffff_fc00 == 0xdac0_1000
        || w & 0xffc0_fc00 == 0xd340_fc00
        || w & 0xffff_fc00 == 0xf201_c000
    {
        // Bit reverse and count, a shift, and the repeating mask a scan tests
        // with, each of one source.
        Insn::Compute {
            rd,
            reads: [rn, rn],
        }
    } else if w & 0xffff_ffe0 == 0xb200_c3e0 || w & 0xff80_0000 == 0xf280_0000 {
        // The other repeating mask, and the halves of a splatted byte, which
        // read nothing.
        Insn::Compute {
            rd,
            reads: [rd, rd],
        }
    } else if w & 0xffc0_0000 == 0xf900_0000 {
        Insn::Store {
            rt: rd,
            base: rn,
            disp: imm12 * 8,
        }
    } else if w & 0x9f00_0000 == 0x1000_0000 {
        let value = (((w >> 5) & 0x7_ffff) << 2) | ((w >> 29) & 3);
        let offset = sign_extend(value, 21);
        // A page-relative address is not this form, and this one counts bytes.
        Insn::Address {
            rd,
            at: u32::try_from(at.checked_add_signed(offset as isize)?).ok()?,
        }
    } else if w & 0xff00_0010 == 0x5400_0000 {
        let cond = match w & 15 {
            0 => Cond::Equal,
            1 => Cond::NotEqual,
            2 => Cond::AtLeast,
            3 => Cond::Less,
            8 => Cond::Greater,
            9 => Cond::AtMost,
            _ => return None,
        };
        Insn::Branch {
            cond: Some(cond),
            to: away(sign_extend((w >> 5) & 0x7_ffff, 19))?,
        }
    } else if w & 0xfc00_0000 == 0x1400_0000 {
        Insn::Branch {
            cond: None,
            to: away(sign_extend(w & 0x3ff_ffff, 26))?,
        }
    } else if w & 0xff00_0000 == 0xb400_0000 {
        Insn::BranchIfZero {
            rt: rd,
            to: away(sign_extend((w >> 5) & 0x7_ffff, 19))?,
        }
    } else {
        return None;
    };
    // Register 31 is the stack pointer or the zero register, and this accepts
    // neither as an operand.
    (!insn.registers().contains(&31)).then_some((insn, 4))
}

/// Decode one x86-64 instruction, exactly: every prefix, operand field and
/// addressing form must be one this accepts, so a byte sequence that would
/// execute as something else is not decoded as this. Instructions vary in
/// length, so this reports how long the one at `at` is, and a caller that
/// followed a branch into the middle of one decodes something different — which
/// is why every branch target is required to be a start this reached.
fn decode_x64(code: &[u8], at: usize) -> Option<(Insn, usize)> {
    let byte = |offset: usize| code.get(at + offset).copied();
    let signed32 = |offset: usize| {
        Some(i64::from(i32::from_le_bytes([
            byte(offset)?,
            byte(offset + 1)?,
            byte(offset + 2)?,
            byte(offset + 3)?,
        ])))
    };
    let mut cursor = 0;
    let (mut wide, mut reg_high, mut index_high, mut base_high) = (false, 0u8, 0u8, 0u8);
    let mut opcode = byte(cursor)?;
    if opcode & 0xf0 == 0x40 {
        wide = opcode & 8 != 0;
        reg_high = (opcode >> 2) & 1;
        index_high = (opcode >> 1) & 1;
        base_high = opcode & 1;
        cursor += 1;
        opcode = byte(cursor)?;
    }
    cursor += 1;
    // `ModRM` and what follows it: the register operand, the memory operand it
    // names, and where the instruction ends.
    let operand = |cursor: usize| -> Option<(u8, u8, Option<u8>, i64, usize, bool)> {
        let modrm = byte(cursor)?;
        let mode = modrm >> 6;
        let reg = ((modrm >> 3) & 7) | reg_high << 3;
        let rm = modrm & 7;
        let mut cursor = cursor + 1;
        if mode == 3 {
            return Some((reg, rm | base_high << 3, None, 0, cursor, true));
        }
        // A position-relative operand, whose displacement counts from the end.
        if mode == 0 && rm == 5 {
            let disp = signed32(cursor)?;
            return Some((reg, 0, None, disp, cursor + 4, false));
        }
        let (base, index) = if rm == 4 {
            let sib = byte(cursor)?;
            cursor += 1;
            // Only an unscaled index, which is all the generator emits.
            if sib >> 6 != 0 {
                return None;
            }
            let index = ((sib >> 3) & 7) | index_high << 3;
            let base = (sib & 7) | base_high << 3;
            (base, (index != 4).then_some(index))
        } else {
            (rm | base_high << 3, None)
        };
        let disp = match mode {
            0 => 0,
            1 => {
                let value = byte(cursor)? as i8;
                cursor += 1;
                i64::from(value)
            }
            _ => {
                let value = signed32(cursor)?;
                cursor += 4;
                value
            }
        };
        Some((reg, base, index, disp, cursor, false))
    };
    let insn = match opcode {
        0x50..=0x57 => Insn::Push {
            r: (opcode - 0x50) | base_high << 3,
        },
        0x58..=0x5f => Insn::Pop {
            r: (opcode - 0x58) | base_high << 3,
        },
        // `mov reg, r/m`: eight bytes of the subject at once.
        0x8b if wide => {
            let (reg, base, index, disp, end, register) = operand(cursor)?;
            cursor = end;
            match (register, index, disp) {
                (false, Some(index), 0) => Insn::LoadWord {
                    rt: reg,
                    base,
                    index,
                },
                _ => return None,
            }
        }
        // `add r/m, reg`, which moves a position on by a distance.
        0x01 if wide => {
            let (reg, base, _, _, end, register) = operand(cursor)?;
            cursor = end;
            if !register {
                return None;
            }
            Insn::AddReg {
                rd: base,
                rn: base,
                rm: reg,
            }
        }
        // The arithmetic a scan does over eight bytes at a time.
        0x31 | 0x29 | 0x21 if wide => {
            let (reg, base, _, _, end, register) = operand(cursor)?;
            cursor = end;
            if !register {
                return None;
            }
            Insn::Compute {
                rd: base,
                reads: [base, reg],
            }
        }
        // `not r/m` and `shr r/m, imm8`.
        0xf7 | 0xc1 if wide => {
            let (reg, base, _, _, end, register) = operand(cursor)?;
            if !register || (opcode == 0xf7 && reg & 7 != 2) || (opcode == 0xc1 && reg & 7 != 5) {
                return None;
            }
            cursor = end + usize::from(opcode == 0xc1);
            Insn::Compute {
                rd: base,
                reads: [base, base],
            }
        }
        // A whole 64-bit constant, which the scan's masks need.
        0xb8..=0xbf if wide => {
            cursor += 8;
            let rd = (opcode - 0xb8) | base_high << 3;
            Insn::Compute {
                rd,
                reads: [rd, rd],
            }
        }
        // `mov r/m, reg`: a register pair, or a store.
        0x89 => {
            let (reg, base, index, disp, end, register) = operand(cursor)?;
            cursor = end;
            match (register, wide) {
                (true, true) => Insn::Mov { rd: base, rm: reg },
                (true, false) => Insn::Mov32 { rd: base, rm: reg },
                (false, true) if index.is_none() && disp >= 0 => Insn::Store {
                    rt: reg,
                    base,
                    disp: u32::try_from(disp).ok()?,
                },
                _ => return None,
            }
        }
        // `mov r/m, imm32`, sign-extended.
        0xc7 if wide => {
            let (reg, base, _, _, end, register) = operand(cursor)?;
            if !register || reg & 7 != 0 {
                return None;
            }
            cursor = end + 4;
            Insn::MovImm {
                rd: base,
                value: signed32(end)?,
            }
        }
        // `lea reg, mem`, which is how a position moves and how the code's own
        // address is taken.
        0x8d if wide => {
            let (reg, base, index, disp, end, register) = operand(cursor)?;
            cursor = end;
            if register || index.is_some() {
                return None;
            }
            let modrm = byte(1 + usize::from(opcode != byte(0)?))?;
            let _ = modrm;
            if base == 0 && disp != 0 && code.get(at + cursor - 4).is_some() && is_rip(code, at)? {
                Insn::Address {
                    rd: reg,
                    at: u32::try_from(at as i64 + cursor as i64 + disp).ok()?,
                }
            } else if disp >= 0 {
                Insn::AddImm {
                    rd: reg,
                    rn: base,
                    imm: u32::try_from(disp).ok()?,
                }
            } else {
                Insn::SubImm {
                    rd: reg,
                    rn: base,
                    imm: u32::try_from(-disp).ok()?,
                }
            }
        }
        // `cmp r/m, reg`.
        0x39 if wide => {
            let (reg, base, _, _, end, register) = operand(cursor)?;
            cursor = end;
            if !register {
                return None;
            }
            Insn::CmpReg { rn: base, rm: reg }
        }
        // `test r/m, reg`, which this subset only uses against itself.
        0x85 if wide => {
            let (reg, base, _, _, end, register) = operand(cursor)?;
            cursor = end;
            if !register || reg != base {
                return None;
            }
            Insn::Test { rn: base }
        }
        // `cmp` and `sub` against a constant, in both widths.
        0x81 | 0x83 => {
            let (reg, base, _, _, end, register) = operand(cursor)?;
            if !register {
                return None;
            }
            let value = if opcode == 0x83 {
                cursor = end + 1;
                i64::from(byte(end)? as i8)
            } else {
                cursor = end + 4;
                signed32(end)?
            };
            let imm = u32::try_from(value).ok()?;
            match (reg & 7, wide) {
                (7, true) => Insn::CmpImm { rn: base, imm },
                (7, false) => Insn::CmpImm32 { rn: base, imm },
                (5, false) => Insn::SubImm32 {
                    rd: base,
                    rn: base,
                    imm,
                },
                _ => return None,
            }
        }
        // `dec r/m`.
        0xff if wide => {
            let (reg, base, _, _, end, register) = operand(cursor)?;
            cursor = end;
            if !register || reg & 7 != 1 {
                return None;
            }
            Insn::Dec { rd: base }
        }
        0x0f => {
            let second = byte(cursor)?;
            cursor += 1;
            match second {
                // `bsf reg, r/m`, which finds the byte a scan matched.
                0xbc if wide => {
                    let (reg, base, _, _, end, register) = operand(cursor)?;
                    cursor = end;
                    if !register {
                        return None;
                    }
                    Insn::Compute {
                        rd: reg,
                        reads: [base, base],
                    }
                }
                // `movzx reg, byte r/m`.
                0xb6 if !wide => {
                    let (reg, base, index, disp, end, register) = operand(cursor)?;
                    cursor = end;
                    if register {
                        return None;
                    }
                    Insn::Load {
                        rt: reg,
                        base,
                        index,
                        disp,
                    }
                }
                // `jcc rel32`.
                0x80..=0x8f if !wide => {
                    let offset = signed32(cursor)?;
                    cursor += 4;
                    let cond = match second & 0xf {
                        0x4 => Cond::Equal,
                        0x5 => Cond::NotEqual,
                        0x2 => Cond::Less,
                        0x3 => Cond::AtLeast,
                        0x7 => Cond::Greater,
                        0x6 => Cond::AtMost,
                        _ => return None,
                    };
                    Insn::Branch {
                        cond: Some(cond),
                        to: usize::try_from(at as i64 + cursor as i64 + offset).ok()?,
                    }
                }
                _ => return None,
            }
        }
        // `jmp rel32`.
        0xe9 if !wide => {
            let offset = signed32(cursor)?;
            cursor += 4;
            Insn::Branch {
                cond: None,
                to: usize::try_from(at as i64 + cursor as i64 + offset).ok()?,
            }
        }
        0xc3 if !wide => Insn::Ret,
        _ => return None,
    };
    // The stack pointer is never an operand of generated code.
    (!insn.registers().contains(&4)).then_some((insn, cursor))
}

/// Whether the instruction at `at` addresses memory relative to its own end,
/// which is `ModRM` mode zero naming register five.
fn is_rip(code: &[u8], at: usize) -> Option<bool> {
    let first = *code.get(at)?;
    let modrm = if first & 0xf0 == 0x40 {
        *code.get(at + 2)?
    } else {
        *code.get(at + 1)?
    };
    Some(modrm >> 6 == 0 && modrm & 7 == 5)
}

impl Insn {
    /// Every register this names, so that a target can refuse one it does not
    /// allow as an operand at all.
    fn registers(self) -> [u8; 3] {
        match self {
            Insn::Ret | Insn::Branch { .. } => [0; 3],
            Insn::MovImm { rd, .. } | Insn::Address { rd, .. } | Insn::Dec { rd } => [rd, 0, 0],
            Insn::Compute { rd, reads } => [rd, reads[0], reads[1]],
            Insn::BranchIfZero { rt, .. } => [rt, 0, 0],
            Insn::Test { rn } => [rn, 0, 0],
            Insn::Push { r } | Insn::Pop { r } => [r, 0, 0],
            Insn::Mov { rd, rm } | Insn::Mov32 { rd, rm } => [rd, rm, 0],
            Insn::AddImm { rd, rn, .. }
            | Insn::SubImm { rd, rn, .. }
            | Insn::SubImm32 { rd, rn, .. } => [rd, rn, 0],
            Insn::CmpImm { rn, .. } | Insn::CmpImm32 { rn, .. } => [rn, 0, 0],
            Insn::CmpReg { rn, rm } => [rn, rm, 0],
            Insn::AddReg { rd, rn, rm } => [rd, rn, rm],
            Insn::Load {
                rt,
                base,
                index: Some(index),
                ..
            }
            | Insn::LoadWord { rt, base, index } => [rt, base, index],
            Insn::Load { rt, base, .. } | Insn::Store { rt, base, .. } => [rt, base, 0],
        }
    }

    /// The register this writes, if any.
    fn written(self) -> Option<u8> {
        match self {
            Insn::MovImm { rd, .. }
            | Insn::Mov { rd, .. }
            | Insn::Mov32 { rd, .. }
            | Insn::AddImm { rd, .. }
            | Insn::SubImm { rd, .. }
            | Insn::SubImm32 { rd, .. }
            | Insn::AddReg { rd, .. }
            | Insn::Dec { rd }
            | Insn::Compute { rd, .. }
            | Insn::Address { rd, .. } => Some(rd),
            Insn::Load { rt, .. } | Insn::LoadWord { rt, .. } | Insn::Pop { r: rt } => Some(rt),
            _ => None,
        }
    }

    /// Where a branch goes, as a byte offset, if it is one.
    fn target(self) -> Option<usize> {
        match self {
            Insn::Branch { to, .. } | Insn::BranchIfZero { to, .. } => Some(to),
            _ => None,
        }
    }

    /// Whether execution continues at the next instruction after this one.
    fn falls_through(self) -> bool {
        !matches!(self, Insn::Ret | Insn::Branch { cond: None, .. })
    }
}

fn narrow(value: i64) -> Option<i32> {
    i32::try_from(value).ok()
}

impl State {
    fn get(&self, r: u8) -> Value {
        self.x.get(r as usize).copied().unwrap_or(Value::Top)
    }

    fn set(&mut self, r: u8, value: Value) {
        if let Some(slot) = self.x.get_mut(r as usize) {
            *slot = value;
        }
        self.restored &= !(1u16 << (r & 15));
        match self.flags {
            Flags::Registers(a, b) if a == r || b == r => self.flags = Flags::Unknown,
            Flags::Immediate(a, _) if a == r => self.flags = Flags::Unknown,
            _ => {}
        }
    }

    /// The largest lower bound on the length this state implies: its own, and
    /// every position's `low + slack`.
    fn length_at_least(&self) -> u64 {
        let mut least = u64::from(self.length_low);
        for value in self.x {
            if let Value::Pos { low, slack } | Value::SubjectAt { low, slack } = value {
                let implied = i64::from(low) + i64::from(slack);
                if implied > 0 {
                    least = max(least, implied as u64);
                }
            }
        }
        least
    }

    fn join(&self, other: &State) -> State {
        let mut joined = State {
            flags: if self.flags == other.flags {
                self.flags
            } else {
                Flags::Unknown
            },
            length_low: min(self.length_low, other.length_low),
            // Paths that saved different things, or different amounts, cannot
            // be told apart afterwards, and a return on one is refused.
            stack: if self.stack == other.stack {
                self.stack
            } else {
                None
            },
            // Only what every path restored.
            restored: self.restored & other.restored,
            ..State::default()
        };
        for r in 0..TRACKED {
            joined.x[r] = join(self.x[r], other.x[r]);
        }
        joined
    }

    /// Learn `value(a) + strict <= value(b)`.
    fn below(&mut self, a: u8, b: u8, strict: u32) {
        let upper = match self.get(b) {
            Value::Length => Some(0),
            Value::Pos { slack, .. } => Some(i64::from(slack)),
            _ => None,
        };
        if let Some(upper) = upper {
            // Clamping a slack down claims less, which is always sound.
            let slack = min(upper + i64::from(strict), i64::from(i32::MAX)) as i32;
            let learned = match self.get(a) {
                Value::Pos { low, slack: old } => Some(Value::Pos {
                    low,
                    slack: max(old, slack),
                }),
                Value::Top | Value::Byte | Value::Fits32 => Some(Value::Pos { low: 0, slack }),
                _ => None,
            };
            if let (Some(learned), Some(slot)) = (learned, self.x.get_mut(a as usize)) {
                *slot = learned;
            }
        }
        let lower = match self.get(a) {
            Value::Pos { low, .. } => u64::from(low),
            Value::Length => u64::from(self.length_low),
            Value::Small(value) => u64::from(value),
            _ => 0,
        } + u64::from(strict);
        self.at_least(b, lower);
    }

    /// Learn `value(a) >= bound`.
    fn at_least(&mut self, a: u8, bound: u64) {
        let bound = min(bound, u64::from(u32::MAX)) as u32;
        match self.get(a) {
            Value::Pos { low, slack } => {
                self.x[a as usize] = Value::Pos {
                    low: max(low, bound),
                    slack,
                }
            }
            Value::Length => self.length_low = max(self.length_low, bound),
            _ => {}
        }
    }

    /// Learn what holds when a condition on the last comparison does.
    fn refine(&mut self, cond: Cond) {
        match self.flags {
            Flags::Registers(a, b) => match cond {
                Cond::Equal => {
                    self.below(a, b, 0);
                    self.below(b, a, 0);
                }
                Cond::AtLeast => self.below(b, a, 0),
                Cond::Greater => self.below(b, a, 1),
                Cond::Less => self.below(a, b, 1),
                Cond::AtMost => self.below(a, b, 0),
                Cond::NotEqual => {}
            },
            Flags::Immediate(a, imm) => match cond {
                Cond::Equal | Cond::AtLeast => self.at_least(a, u64::from(imm)),
                Cond::Greater => self.at_least(a, u64::from(imm) + 1),
                _ => {}
            },
            Flags::Unknown => {}
        }
    }
}

fn join(a: Value, b: Value) -> Value {
    use Value::*;
    if a == b {
        return a;
    }
    let position = |v| match v {
        Length => Some((0, 0)),
        Pos { low, slack } => Some((low, slack)),
        _ => None,
    };
    let address = |v| match v {
        Subject => Some((0, 0)),
        SubjectAt { low, slack } => Some((low, slack)),
        _ => None,
    };
    let number = |v| match v {
        Small(n) if n <= 255 => Some(0),
        Byte => Some(0),
        Small(_) | Fits32 => Some(1),
        _ => None,
    };
    if let (Some((la, sa)), Some((lb, sb))) = (position(a), position(b)) {
        return Pos {
            low: min(la, lb),
            slack: min(sa, sb),
        };
    }
    if let (Some((la, sa)), Some((lb, sb))) = (address(a), address(b)) {
        return SubjectAt {
            low: min(la, lb),
            slack: min(sa, sb),
        };
    }
    match (number(a), number(b)) {
        (Some(0), Some(0)) => Byte,
        (Some(_), Some(_)) => Fits32,
        _ => Top,
    }
}

/// `value + k`, for a `k` below 2^32.
fn add_const(value: Value, k: u64) -> Value {
    use Value::*;
    let shifted = |low: u32, slack: i32| {
        let slack = narrow(i64::from(slack) - k as i64)?;
        let low = min(u64::from(low) + k, u64::from(u32::MAX)) as u32;
        Some((low, slack))
    };
    match value {
        Length => shifted(0, 0).map_or(Top, |(low, slack)| Pos { low, slack }),
        Pos { low, slack } => shifted(low, slack).map_or(Top, |(low, slack)| Pos { low, slack }),
        Subject => shifted(0, 0).map_or(Top, |(low, slack)| SubjectAt { low, slack }),
        SubjectAt { low, slack } => {
            shifted(low, slack).map_or(Top, |(low, slack)| SubjectAt { low, slack })
        }
        Code(at) => u32::try_from(u64::from(at) + k).map_or(Top, Code),
        Small(n) => u32::try_from(u64::from(n) + k).map_or(Top, Small),
        Byte if k < 1 << 24 => Fits32,
        _ => Top,
    }
}

/// `value - k`, which is only anything when it cannot wrap.
fn sub_const(state: &State, value: Value, k: u64) -> Value {
    use Value::*;
    match value {
        Length if state.length_at_least() >= k => Pos {
            low: (min(state.length_at_least(), u64::from(u32::MAX)) - k) as u32,
            slack: min(k, i32::MAX as u64) as i32,
        },
        Pos { low, slack } if u64::from(low) >= k => Pos {
            low: low - k as u32,
            slack: min(i64::from(slack) + k as i64, i64::from(i32::MAX)) as i32,
        },
        SubjectAt { low, slack } if u64::from(low) >= k => SubjectAt {
            low: low - k as u32,
            slack: min(i64::from(slack) + k as i64, i64::from(i32::MAX)) as i32,
        },
        Code(at) if u64::from(at) >= k => Code(at - k as u32),
        Small(n) if u64::from(n) >= k => Small(n - k as u32),
        _ => Top,
    }
}

fn add_values(a: Value, b: Value) -> Value {
    use Value::*;
    match (a, b) {
        (Subject, Length) | (Length, Subject) => SubjectAt { low: 0, slack: 0 },
        (Subject, Pos { low, slack }) | (Pos { low, slack }, Subject) => SubjectAt { low, slack },
        (Code(at), Byte) | (Byte, Code(at)) => CodeSpan { at, width: 256 },
        (value, Small(n)) | (Small(n), value) => add_const(value, u64::from(n)),
        _ => Top,
    }
}

/// Whether `width` bytes at this address are inside the subject or the code.
fn readable(value: Value, code_length: usize, width: i32) -> bool {
    match value {
        Value::SubjectAt { slack, .. } => slack >= width,
        Value::Code(at) => (at as usize) + width as usize <= code_length,
        Value::CodeSpan { at, width: span } => at as usize + span as usize <= code_length,
        _ => false,
    }
}

fn fits32(value: Value) -> bool {
    matches!(value, Value::Small(_) | Value::Byte | Value::Fits32)
}

fn is_position(value: Value) -> bool {
    matches!(value, Value::Length) || matches!(value, Value::Pos { slack, .. } if slack >= 0)
}

/// What an instruction leaves in the registers, along the path it falls through
/// to and along the one it branches to.
fn transfer(insn: Insn, state: &State) -> (State, State) {
    let mut next = *state;
    let mut taken = *state;
    match insn {
        Insn::Ret | Insn::Store { .. } => {}
        Insn::Push { r } => {
            next.stack = match next.stack {
                Some((mut saved, depth)) if (depth as usize) < SAVED => {
                    saved[depth as usize] = r;
                    Some((saved, depth + 1))
                }
                _ => None,
            }
        }
        Insn::Pop { r } => {
            // What comes back is what was pushed, and only then is the
            // register known to hold what it held on entry again.
            let popped = match next.stack {
                Some((mut saved, depth)) if depth > 0 && saved[depth as usize - 1] == r => {
                    // Cleared as it leaves, so that two paths which saved the
                    // same things are the same state whatever they popped.
                    saved[depth as usize - 1] = 0;
                    next.stack = Some((saved, depth - 1));
                    true
                }
                _ => {
                    next.stack = None;
                    false
                }
            };
            next.set(r, Value::Top);
            if popped {
                next.restored |= 1u16 << (r & 15);
            }
        }
        Insn::MovImm { rd, value } => next.set(
            rd,
            match (u32::try_from(value), u16::try_from(!value)) {
                (Ok(small), _) => Value::Small(small),
                (_, Ok(complement)) => Value::Negative(complement),
                _ => Value::Top,
            },
        ),
        Insn::Mov { rd, rm } => next.set(rd, state.get(rm)),
        Insn::Mov32 { rd, rm } => next.set(
            rd,
            if fits32(state.get(rm)) {
                state.get(rm)
            } else {
                Value::Fits32
            },
        ),
        Insn::AddImm { rd, rn, imm } => next.set(rd, add_const(state.get(rn), u64::from(imm))),
        Insn::SubImm { rd, rn, imm } => {
            next.set(rd, sub_const(state, state.get(rn), u64::from(imm)))
        }
        Insn::SubImm32 { rd, .. } => next.set(rd, Value::Fits32),
        Insn::Dec { rd } => next.set(rd, sub_const(state, state.get(rd), 1)),
        Insn::AddReg { rd, rn, rm } => next.set(rd, add_values(state.get(rn), state.get(rm))),
        Insn::CmpImm { rn, imm } => next.flags = Flags::Immediate(rn, imm as u16),
        Insn::CmpImm32 { .. } => next.flags = Flags::Unknown,
        Insn::CmpReg { rn, rm } => next.flags = Flags::Registers(rn, rm),
        Insn::Test { rn } => next.flags = Flags::Immediate(rn, 0),
        Insn::Load { rt, .. } => next.set(rt, Value::Byte),
        Insn::LoadWord { rt, .. } => next.set(rt, Value::Top),
        Insn::Compute { rd, .. } => {
            next.set(rd, Value::Top);
            next.flags = Flags::Unknown;
        }
        Insn::Address { rd, at } => next.set(rd, Value::Code(at)),
        Insn::BranchIfZero { .. } => {}
        Insn::Branch { cond: None, .. } => {}
        Insn::Branch {
            cond: Some(cond), ..
        } => {
            taken.refine(cond);
            next.refine(cond.inverse());
        }
    }
    (next, taken)
}

fn merge(facts: &mut [Facts], target: usize, incoming: &State) {
    let fact = &mut facts[target];
    if !fact.reached {
        *fact = Facts {
            state: *incoming,
            reached: true,
            pending: true,
            updates: 0,
            guard: false,
            interior: fact.interior,
        };
        return;
    }
    let mut joined = fact.state.join(incoming);
    if joined == fact.state {
        return;
    }
    fact.updates += 1;
    if fact.updates > WIDEN {
        for r in 0..TRACKED {
            if joined.x[r] != fact.state.x[r] {
                joined.x[r] = Value::Top;
            }
        }
        if joined.length_low != fact.state.length_low {
            joined.length_low = 0;
        }
    }
    fact.state = joined;
    fact.pending = true;
}

/// What one reached instruction must satisfy, given everything that can reach
/// it.
fn obligations(
    insn: Insn,
    at: usize,
    length: usize,
    target: Target,
    state: &State,
    code_length: usize,
    register_count: usize,
) -> Result<(), Reason> {
    let require = |holds: bool, reason| if holds { Ok(()) } else { Err(reason) };
    if let Some(rd) = insn.written() {
        require(
            (rd as usize) < TRACKED && target.writable(rd),
            Reason::Written,
        )?;
    }
    // A branch outside the code, or into the middle of an instruction, is
    // refused; the caller has already decoded every instruction start.
    if let Some(to) = insn.target() {
        require(to < code_length, Reason::Branch)?;
    }
    require(
        !insn.falls_through() || at + length < code_length,
        Reason::FallsOffEnd,
    )?;
    match insn {
        Insn::Ret => {
            let answer = state.get(target.result());
            require(
                is_position(answer) || matches!(answer, Value::Negative(0 | 1)),
                Reason::Return,
            )?;
            // Nothing saved is still on the stack, and everything the
            // convention preserves holds what it held on entry.
            require(
                state.stack.is_some_and(|(_, depth)| depth == 0),
                Reason::Return,
            )?;
            let preserved = target
                .preserved()
                .iter()
                .fold(0u16, |mask, &r| mask | 1u16 << (r & 15));
            require(state.restored & preserved == preserved, Reason::Return)
        }
        Insn::Push { .. } | Insn::Pop { .. } => require(state.stack.is_some(), Reason::Return),
        Insn::Load {
            base, index, disp, ..
        } => {
            let address = match index {
                Some(index) => add_values(state.get(base), state.get(index)),
                None => state.get(base),
            };
            let address = match disp {
                0 => address,
                _ if disp > 0 => add_const(address, disp.unsigned_abs()),
                _ => sub_const(state, address, disp.unsigned_abs()),
            };
            require(readable(address, code_length, 1), Reason::Load)
        }
        Insn::Store { rt, base, disp } => {
            require(
                state.get(base) == Value::Registers
                    && disp % 8 == 0
                    && ((disp / 8) as usize) < register_count,
                Reason::Store,
            )?;
            require(is_position(state.get(rt)), Reason::Stored)
        }
        Insn::LoadWord { base, index, .. } => require(
            readable(
                add_values(state.get(base), state.get(index)),
                code_length,
                8,
            ),
            Reason::Load,
        ),
        Insn::CmpImm32 { rn, .. } => require(fits32(state.get(rn)), Reason::Compare),
        _ => Ok(()),
    }
}

/// Which instruction set generated code is written in, and what its calling
/// convention says about it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    /// AArch64: arguments in `X0`-`X4`, the result in `X0`, and `X0`-`X17`
    /// free for a leaf function to write.
    A64,
    /// x86-64 under the System V convention, which Linux and macOS use:
    /// arguments in `RDI`, `RSI`, `RDX`, `RCX` and `R8`, the result in `RAX`,
    /// and `RBX`, `RBP` and `R12`-`R15` preserved by saving and restoring them.
    X64,
}

impl Target {
    /// Bytes between the code two neighbouring [`Facts`] describe. AArch64
    /// instructions are four bytes; x86-64's vary, so one per byte is what
    /// covers every instruction start.
    pub fn stride(self) -> usize {
        match self {
            Target::A64 => 4,
            Target::X64 => 1,
        }
    }

    fn decode(self, code: &[u8], at: usize) -> Option<(Insn, usize)> {
        match self {
            Target::A64 => decode_a64(code, at),
            Target::X64 => decode_x64(code, at),
        }
    }

    /// What the host's half of the call leaves in the registers.
    fn entry(self) -> State {
        let mut state = State::default();
        let (subject, length, registers) = match self {
            Target::A64 => (0, 1, 3),
            Target::X64 => (7, 6, 1),
        };
        state.x[subject] = Value::Subject;
        state.x[length] = Value::Length;
        state.x[registers] = Value::Registers;
        state
    }

    /// Whether generated code may write this register at all. What the
    /// convention preserves on x86-64 is written between a prologue that saves
    /// it and an epilogue that restores it, which is checked as a shape.
    fn writable(self, r: u8) -> bool {
        match self {
            Target::A64 => (r as usize) < TRACKED,
            // Everything but the stack pointer.
            Target::X64 => r < 16 && r != 4,
        }
    }

    /// The register holding the answer at a return.
    fn result(self) -> u8 {
        0
    }

    /// The register holding the backward branches left.
    fn budget(self) -> u8 {
        match self {
            Target::A64 => 4,
            Target::X64 => 8,
        }
    }

    /// The registers a prologue saves and an epilogue restores, in the order
    /// they are pushed.
    fn preserved(self) -> &'static [u8] {
        match self {
            Target::A64 => &[],
            Target::X64 => &[3, 5, 12, 13, 14, 15],
        }
    }

    /// Whether these instructions, ending at a backward branch, are the shape
    /// that spends a unit of budget: a test of the budget that leaves when it
    /// is spent, a decrement, and the branch itself. Anything else repeats
    /// without paying, which is what bounds how long a call runs.
    fn spends_budget(self, before: &[Insn]) -> bool {
        let budget = self.budget();
        match self {
            Target::A64 => matches!(
                before,
                [
                    Insn::BranchIfZero { rt, .. },
                    Insn::SubImm { rd, rn, imm: 1 },
                ] if *rt == budget && *rd == budget && *rn == budget
            ),
            Target::X64 => matches!(
                before,
                [
                    Insn::Test { rn },
                    Insn::Branch { cond: Some(Cond::Equal), .. },
                    Insn::Dec { rd },
                ] if *rn == budget && *rd == budget
            ),
        }
    }

    /// How many instructions before a backward branch its guard occupies.
    fn guard_length(self) -> usize {
        match self {
            Target::A64 => 2,
            Target::X64 => 3,
        }
    }
}

/// Check `code` as described in the module documentation, using one of
/// [`Facts`] per [`Target::stride`] bytes of it.
pub fn verify(
    target: Target,
    code: &[u8],
    register_count: usize,
    facts: &mut [Facts],
) -> Result<(), VerifyError> {
    let stride = target.stride();
    let count = code.len() / stride;
    let fail = |index: usize, reason| {
        Err(VerifyError {
            at: index * stride,
            reason,
        })
    };
    if count == 0 {
        return fail(0, Reason::Undecodable);
    }
    if facts.len() < count {
        return fail(0, Reason::Scratch);
    }
    let facts = &mut facts[..count];
    facts.fill(Facts::default());
    merge(facts, 0, &target.entry());

    // Every path, until nothing learned changes anything. Each update after
    // the widening point gives a register up entirely, so this ends.
    let mut changed = true;
    while changed {
        changed = false;
        for index in 0..count {
            if !facts[index].pending {
                continue;
            }
            facts[index].pending = false;
            changed = true;
            let Some((insn, length)) = target.decode(code, index * stride) else {
                continue;
            };
            let state = facts[index].state;
            let (next, taken) = transfer(insn, &state);
            let after = (index * stride + length) / stride;
            if insn.falls_through() && after < count {
                merge(facts, after, &next);
            }
            if let Some(to) = insn.target()
                && to % stride == 0
                && to / stride < count
            {
                merge(facts, to / stride, &taken);
            }
        }
    }

    // Where each reached instruction ends, so that reaching the inside of one
    // is visible: that would mean the same bytes decode two ways, which is two
    // instruction streams where this checks one.
    for index in 0..count {
        if !facts[index].reached {
            continue;
        }
        let at = index * stride;
        let Some((_, length)) = target.decode(code, at) else {
            return fail(index, Reason::Undecodable);
        };
        for byte in (at + stride..at + length).step_by(stride) {
            facts[byte / stride].interior = true;
        }
    }
    for (index, fact) in facts.iter().enumerate() {
        if fact.reached && fact.interior {
            return fail(index, Reason::Branch);
        }
    }

    // What every reached instruction must satisfy, against everything that can
    // reach it.
    for (index, fact) in facts.iter().enumerate() {
        if !fact.reached {
            continue;
        }
        let at = index * stride;
        let (insn, length) = target.decode(code, at).expect("decoded above");
        obligations(
            insn,
            at,
            length,
            target,
            &fact.state,
            code.len(),
            register_count,
        )
        .or_else(|reason| fail(index, reason))?;
    }

    // The budget. A backward branch must end a guard that tests the budget,
    // leaves when none is left, and spends one; nothing may branch into that
    // guard, and nothing else may write the budget. Then the budget falls by
    // exactly one before each backward branch and never wraps, and between two
    // backward branches execution only moves forward, which is the bound in
    // the module documentation.
    for index in 0..count {
        if !facts[index].reached {
            continue;
        }
        let (insn, _) = target.decode(code, index * stride).expect("decoded above");
        let Some(to) = insn.target() else { continue };
        if to > index * stride {
            continue;
        }
        let mut starts = [0usize; 8];
        let guard = preceding(target, code, facts, index * stride, &mut starts);
        let length = target.guard_length();
        let mut decoded = [Insn::Ret; 3];
        let enough = guard.len() >= length
            && guard[..length]
                .iter()
                .rev()
                .zip(decoded.iter_mut())
                .all(|(&start, slot)| match target.decode(code, start) {
                    Some((insn, _)) => {
                        *slot = insn;
                        true
                    }
                    None => false,
                });
        if !(matches!(insn, Insn::Branch { cond: None, .. })
            && enough
            && target.spends_budget(&decoded[..length]))
        {
            return fail(index, Reason::Budget);
        }
        facts[index].guard = true;
        for &start in &guard[..length - 1] {
            facts[start / stride].guard = true;
        }
    }
    for index in 0..count {
        if !facts[index].reached {
            continue;
        }
        let (insn, _) = target.decode(code, index * stride).expect("decoded above");
        if let Some(to) = insn.target()
            && to % stride == 0
            && facts[to / stride].guard
        {
            return fail(index, Reason::Budget);
        }
        if insn.written() == Some(target.budget()) && !facts[index].guard {
            return fail(index, Reason::Budget);
        }
    }
    Ok(())
}

/// The instruction starts immediately before `at`, nearest first, as far back
/// as `starts` holds. Only starts the analysis reached count: a byte that is
/// not one is not an instruction.
fn preceding<'s>(
    target: Target,
    code: &[u8],
    facts: &[Facts],
    at: usize,
    starts: &'s mut [usize; 8],
) -> &'s [usize] {
    let stride = target.stride();
    let mut found = 0;
    let mut before = at;
    while found < starts.len() && before >= stride {
        let mut walked = None;
        // The instruction that ends here, which is the one that starts at the
        // furthest byte back whose length reaches it.
        for back in 1..=16.min(before / stride) {
            let start = before - back * stride;
            if !facts[start / stride].reached {
                continue;
            }
            if let Some((_, length)) = target.decode(code, start)
                && start + length == before
            {
                walked = Some(start);
                break;
            }
        }
        let Some(start) = walked else { break };
        starts[found] = start;
        found += 1;
        before = start;
    }
    &starts[..found]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::a64::{Assembler, Cond as Encoded, Reg, X0, X1, X2, X3, X4, X5};

    const AT: Reg = Reg(6);
    const BYTE: Reg = Reg(7);
    const TMP: Reg = Reg(8);

    /// Verify what `build` assembles, with two registers.
    fn check(build: impl FnOnce(&mut Assembler<'_>)) -> Result<(), Reason> {
        let mut code = [0u8; 1024];
        let mut assembler = Assembler::new(&mut code);
        build(&mut assembler);
        let length = assembler.finish().expect("encodes");
        let mut facts = [Facts::default(); 256];
        verify(Target::A64, &code[..length], 2, &mut facts).map_err(|error| error.reason)
    }

    fn raw(asm: &mut Assembler<'_>, word: u32) {
        asm.data(&word.to_le_bytes());
    }

    /// Decode one word at a fixed address, so that a branch's absolute target
    /// is the offset this expects.
    fn decoded(word: u32) -> Option<Insn> {
        let mut code = [0u8; 64];
        code[32..36].copy_from_slice(&word.to_le_bytes());
        decode_a64(&code, 32).map(|(insn, length)| {
            assert_eq!(length, 4);
            insn
        })
    }

    #[test]
    fn decodes_what_the_assembler_encodes() {
        // The same words the encoder's tests take from the system assembler.
        assert_eq!(decoded(0xd65f_03c0), Some(Insn::Ret));
        assert_eq!(
            decoded(0x3862_6824),
            Some(Insn::Load {
                rt: 4,
                base: 1,
                index: Some(2),
                disp: 0
            })
        );
        assert_eq!(
            decoded(0x7101_849f),
            Some(Insn::CmpImm32 { rn: 4, imm: 97 })
        );
        assert_eq!(decoded(0xf100_183f), Some(Insn::CmpImm { rn: 1, imm: 6 }));
        assert_eq!(decoded(0xeb10_005f), Some(Insn::CmpReg { rn: 2, rm: 16 }));
        assert_eq!(
            decoded(0x9100_2020),
            Some(Insn::AddImm {
                rd: 0,
                rn: 1,
                imm: 8
            })
        );
        assert_eq!(
            decoded(0xd100_0484),
            Some(Insn::SubImm {
                rd: 4,
                rn: 4,
                imm: 1
            })
        );
        assert_eq!(
            decoded(0x5101_84e7),
            Some(Insn::SubImm32 {
                rd: 7,
                rn: 7,
                imm: 97
            })
        );
        assert_eq!(
            decoded(0xf900_0862),
            Some(Insn::Store {
                rt: 2,
                base: 3,
                disp: 16
            })
        );
        assert_eq!(decoded(0xaa08_03e7), Some(Insn::Mov { rd: 7, rm: 8 }));
        assert_eq!(
            decoded(0x3940_1ce7),
            Some(Insn::Load {
                rt: 7,
                base: 7,
                index: None,
                disp: 7
            })
        );
        assert_eq!(
            decoded(0x8b08_000d),
            Some(Insn::AddReg {
                rd: 13,
                rn: 0,
                rm: 8
            })
        );
        assert_eq!(
            decoded(0x9280_0020),
            Some(Insn::MovImm { rd: 0, value: -2 })
        );
        assert_eq!(
            decoded(0xd29f_ffe3),
            Some(Insn::MovImm {
                rd: 3,
                value: 65535
            })
        );
        assert_eq!(decoded(0x1000_0089), Some(Insn::Address { rd: 9, at: 48 }));
        assert_eq!(
            decoded(0x54ff_ffe1),
            Some(Insn::Branch {
                cond: Some(Cond::NotEqual),
                to: 28
            })
        );
        assert_eq!(
            decoded(0x17ff_ffff),
            Some(Insn::Branch { cond: None, to: 28 })
        );
        assert_eq!(
            decoded(0xb4ff_ffe4),
            Some(Insn::BranchIfZero { rt: 4, to: 28 })
        );
        assert_eq!(
            decoded(0x54ff_ffc2),
            Some(Insn::Branch {
                cond: Some(Cond::AtLeast),
                to: 24
            })
        );
        // Near misses that execute as something else: `ret x5`, a shifted
        // `add`, `add sp, sp, #8`, `bl`, and a signed condition.
        for word in [
            0xd65f_00a0,
            0x9140_2020,
            0x9100_23ff,
            0x9400_0000,
            0x5400_000b,
        ] {
            assert_eq!(decoded(word), None, "{word:08x}");
        }
    }

    mod x86_64 {
        use super::{Facts, Reason, Target, verify};
        use crate::native::x64::{
            Assembler, Cond, R8, R9, R10, R11, R12, R13, R14, R15, RAX, RBP, RBX, RCX, RDI, RDX,
            RSI,
        };

        /// The registers a call must give back, in the order generated code
        /// saves them.
        const PRESERVED: [crate::native::x64::Reg; 6] = [RBX, RBP, R12, R13, R14, R15];

        /// Verify what `build` assembles between a prologue and whatever it
        /// returns with, with two of the caller's registers.
        fn check(build: impl FnOnce(&mut Assembler<'_>)) -> Result<(), Reason> {
            extern crate std;
            let mut code = [0u8; 1024];
            let mut assembler = Assembler::new(&mut code);
            for reg in PRESERVED {
                assembler.push(reg);
            }
            build(&mut assembler);
            let length = assembler.finish().expect("encodes");
            let mut facts = std::vec![Facts::default(); 1024];
            verify(Target::X64, &code[..length], 2, &mut facts).map_err(|error| error.reason)
        }

        /// What every return has to end with.
        fn give_back(asm: &mut Assembler<'_>) {
            for reg in PRESERVED.into_iter().rev() {
                asm.pop(reg);
            }
            asm.ret();
        }

        /// Return no match, correctly.
        fn no_match(asm: &mut Assembler<'_>) {
            asm.mov_imm(RAX, -1);
            give_back(asm);
        }

        #[test]
        fn accepts_a_load_the_length_bounds() {
            assert_eq!(
                check(|a| {
                    a.cmp(RDX, RSI);
                    let none = a.jcc_forward(Cond::AtLeast);
                    a.movzx(R11, RDI, RDX, 0);
                    a.bind(none);
                    no_match(a);
                }),
                Ok(())
            );
        }

        #[test]
        fn refuses_a_load_nothing_bounds() {
            assert_eq!(
                check(|a| {
                    a.movzx(R11, RDI, RDX, 0);
                    no_match(a);
                }),
                Err(Reason::Load)
            );
        }

        #[test]
        fn refuses_a_load_one_byte_past_the_length() {
            assert_eq!(
                check(|a| {
                    a.cmp(RDX, RSI);
                    let none = a.jcc_forward(Cond::AtLeast);
                    // One further than the comparison allows.
                    a.movzx(R11, RDI, RDX, 1);
                    a.bind(none);
                    no_match(a);
                }),
                Err(Reason::Load)
            );
        }

        #[test]
        fn refuses_a_store_outside_the_callers_registers() {
            assert_eq!(
                check(|a| {
                    a.cmp(RDX, RSI);
                    let none = a.jcc_forward(Cond::Above);
                    // Two registers were asked for; this is the third.
                    a.store(RCX, 16, RDX);
                    a.bind(none);
                    no_match(a);
                }),
                Err(Reason::Store)
            );
        }

        #[test]
        fn refuses_a_store_of_something_that_is_not_a_position() {
            assert_eq!(
                check(|a| {
                    a.cmp(RDX, RSI);
                    let none = a.jcc_forward(Cond::AtLeast);
                    a.movzx(R11, RDI, RDX, 0);
                    // A byte read from the subject is not a position in it.
                    a.store(RCX, 0, R11);
                    a.bind(none);
                    no_match(a);
                }),
                Err(Reason::Stored)
            );
        }

        #[test]
        fn accepts_a_store_of_a_bounded_position() {
            assert_eq!(
                check(|a| {
                    a.cmp(RDX, RSI);
                    let none = a.jcc_forward(Cond::Above);
                    a.store(RCX, 0, RDX);
                    a.bind(none);
                    no_match(a);
                }),
                Ok(())
            );
        }

        #[test]
        fn refuses_a_return_that_does_not_give_the_registers_back() {
            assert_eq!(
                check(|a| {
                    a.mov_imm(RAX, -1);
                    // One short of what the prologue saved.
                    for reg in PRESERVED.into_iter().rev().skip(1) {
                        a.pop(reg);
                    }
                    a.ret();
                }),
                Err(Reason::Return)
            );
        }

        #[test]
        fn refuses_a_return_that_skips_the_restore() {
            assert_eq!(
                check(|a| {
                    a.mov_imm(RAX, -1);
                    // Jump over the pops to the return, which leaves both the
                    // stack and the preserved registers as the body left them.
                    let over = a.jmp_forward();
                    for reg in PRESERVED.into_iter().rev() {
                        a.pop(reg);
                    }
                    a.bind(over);
                    a.ret();
                }),
                Err(Reason::Return)
            );
        }

        #[test]
        fn refuses_a_return_of_something_that_is_not_an_answer() {
            assert_eq!(
                check(|a| {
                    // The subject's own address is not a position in it.
                    a.mov(RAX, RDI);
                    give_back(a);
                }),
                Err(Reason::Return)
            );
        }

        #[test]
        fn refuses_a_loop_that_does_not_spend_budget() {
            assert_eq!(
                check(|a| {
                    let top = a.here();
                    a.jmp_back(top);
                }),
                Err(Reason::Budget)
            );
        }

        #[test]
        fn refuses_a_loop_that_tests_the_budget_without_spending_it() {
            assert_eq!(
                check(|a| {
                    let top = a.here();
                    a.test(R8);
                    let out = a.jcc_forward(Cond::Equal);
                    a.jmp_back(top);
                    a.bind(out);
                    no_match(a);
                }),
                Err(Reason::Budget)
            );
        }

        #[test]
        fn accepts_a_loop_that_spends_budget() {
            assert_eq!(
                check(|a| {
                    let top = a.here();
                    a.test(R8);
                    let out = a.jcc_forward(Cond::Equal);
                    a.dec(R8);
                    a.jmp_back(top);
                    a.bind(out);
                    no_match(a);
                }),
                Ok(())
            );
        }

        #[test]
        fn refuses_writing_the_budget_outside_its_guard() {
            assert_eq!(
                check(|a| {
                    a.dec(R8);
                    no_match(a);
                }),
                Err(Reason::Budget)
            );
        }

        #[test]
        fn refuses_a_branch_into_the_middle_of_a_guard() {
            assert_eq!(
                check(|a| {
                    let top = a.here();
                    a.test(R8);
                    let out = a.jcc_forward(Cond::Equal);
                    let decrement = a.here();
                    a.dec(R8);
                    a.jmp_back(top);
                    a.bind(out);
                    // Straight to the decrement, which skips the test.
                    a.jmp_back(decrement);
                    no_match(a);
                }),
                Err(Reason::Budget)
            );
        }

        #[test]
        fn refuses_a_branch_that_leaves_the_code() {
            assert_eq!(
                check(|a| {
                    // A displacement no instruction of this code is at.
                    a.data(&[0xe9, 0x00, 0x01, 0x00, 0x00]);
                    no_match(a);
                }),
                Err(Reason::Branch)
            );
        }

        /// A branch into the middle of an instruction is not waved through:
        /// what the processor would decode from there is what this checks, and
        /// here those bytes are a narrow comparison of a position, which can
        /// exceed four gigabytes.
        #[test]
        fn checks_the_instructions_a_branch_into_the_middle_creates() {
            assert_eq!(
                check(|a| {
                    // One byte into the four-byte `cmp` that follows, whose
                    // tail is `cmp edx, 32`.
                    a.data(&[0xe9, 0x01, 0x00, 0x00, 0x00]);
                    a.cmp_imm(RDX, 32);
                    no_match(a);
                }),
                Err(Reason::Compare)
            );
        }

        #[test]
        fn refuses_code_that_runs_off_its_end() {
            assert_eq!(
                check(|a| {
                    a.mov(R10, RDX);
                }),
                Err(Reason::FallsOffEnd)
            );
        }

        #[test]
        fn refuses_a_narrow_comparison_of_something_wider() {
            assert_eq!(
                check(|a| {
                    // A position can exceed four gigabytes; a byte cannot.
                    a.cmp32_imm(RDX, 97);
                    no_match(a);
                }),
                Err(Reason::Compare)
            );
        }

        #[test]
        fn accepts_a_narrow_comparison_of_a_byte() {
            assert_eq!(
                check(|a| {
                    a.cmp(RDX, RSI);
                    let none = a.jcc_forward(Cond::AtLeast);
                    a.movzx(R11, RDI, RDX, 0);
                    a.cmp32_imm(R11, 97);
                    a.bind(none);
                    no_match(a);
                }),
                Ok(())
            );
        }

        #[test]
        fn refuses_an_instruction_it_does_not_decode() {
            assert_eq!(
                check(|a| {
                    // `syscall`, which this subset has no business emitting.
                    a.data(&[0x0f, 0x05]);
                    no_match(a);
                }),
                Err(Reason::Undecodable)
            );
        }

        #[test]
        fn refuses_reading_through_a_register_that_is_not_an_address() {
            assert_eq!(
                check(|a| {
                    // `R9` holds nothing yet; a load through it is unbounded.
                    a.movzx(R11, R9, RDX, 0);
                    no_match(a);
                }),
                Err(Reason::Load)
            );
        }
    }

    #[test]
    fn accepts_a_load_after_the_comparison_that_bounds_it() {
        assert_eq!(
            check(|a| {
                a.cmp(X2, X1);
                let none = a.b_cond_forward(Encoded::Hs);
                a.ldrb(BYTE, X0, X2);
                a.bind(none);
                a.movn(X0, 0);
                a.ret();
            }),
            Ok(())
        );
    }

    #[test]
    fn refuses_a_load_not_bounded() {
        assert_eq!(
            check(|a| {
                a.ldrb(BYTE, X0, X2);
                a.movn(X0, 0);
                a.ret();
            }),
            Err(Reason::Load)
        );
        // Off by one: the start may equal the length, and there is no byte
        // there.
        assert_eq!(
            check(|a| {
                a.cmp(X2, X1);
                let none = a.b_cond_forward(Encoded::Hi);
                a.ldrb(BYTE, X0, X2);
                a.bind(none);
                a.movn(X0, 0);
                a.ret();
            }),
            Err(Reason::Load)
        );
        // Tested on the path that does not load, rather than the one that does.
        assert_eq!(
            check(|a| {
                a.cmp(X2, X1);
                let load = a.b_cond_forward(Encoded::Hs);
                a.movn(X0, 0);
                a.ret();
                a.bind(load);
                a.ldrb(BYTE, X0, X2);
                a.movn(X0, 0);
                a.ret();
            }),
            Err(Reason::Load)
        );
    }

    #[test]
    fn refuses_a_bound_that_wraps() {
        // The shape the emitter used to have: adding what a match needs to the
        // start and comparing that with the length. A start near the top of
        // the address space wraps the sum and passes.
        assert_eq!(
            check(|a| {
                a.add_imm(AT, X2, 2);
                a.cmp(AT, X1);
                let none = a.b_cond_forward(Encoded::Hi);
                a.ldrb(BYTE, X0, X2);
                a.bind(none);
                a.movn(X0, 0);
                a.ret();
            }),
            Err(Reason::Load)
        );
        // The last byte, when the length may be zero.
        assert_eq!(
            check(|a| {
                a.sub_imm(TMP, X1, 1);
                a.ldrb(BYTE, X0, TMP);
                a.movn(X0, 0);
                a.ret();
            }),
            Err(Reason::Load)
        );
        assert_eq!(
            check(|a| {
                a.cmp_imm(X1, 1);
                let none = a.b_cond_forward(Encoded::Lo);
                a.sub_imm(TMP, X1, 1);
                a.ldrb(BYTE, X0, TMP);
                a.bind(none);
                a.movn(X0, 0);
                a.ret();
            }),
            Ok(())
        );
    }

    #[test]
    fn bounds_a_table_load_by_the_table() {
        let table = |size: usize| {
            check(move |a| {
                a.cmp(X2, X1);
                let none = a.b_cond_forward(Encoded::Hs);
                a.ldrb(BYTE, X0, X2);
                let address = a.adr_forward(X5);
                a.ldrb(BYTE, X5, BYTE);
                a.bind(none);
                a.movn(X0, 0);
                a.ret();
                a.bind(address);
                a.data(&[1; 256][..size]);
            })
        };
        assert_eq!(table(256), Ok(()));
        assert_eq!(table(252), Err(Reason::Load));
    }

    #[test]
    fn refuses_a_store_outside_the_registers_or_of_a_non_position() {
        let store = |index: u32, bounded: bool| {
            check(move |a| {
                let mut none = None;
                if bounded {
                    a.cmp(X2, X1);
                    none = Some(a.b_cond_forward(Encoded::Hi));
                }
                a.str_index(X2, X3, index);
                if let Some(none) = none {
                    a.bind(none);
                }
                a.movn(X0, 0);
                a.ret();
            })
        };
        assert_eq!(store(1, true), Ok(()));
        assert_eq!(store(2, true), Err(Reason::Store));
        assert_eq!(store(0, false), Err(Reason::Stored));
        // Into the subject rather than the registers.
        assert_eq!(
            check(|a| {
                a.cmp(X2, X1);
                let none = a.b_cond_forward(Encoded::Hi);
                a.str_index(X2, X0, 0);
                a.bind(none);
                a.movn(X0, 0);
                a.ret();
            }),
            Err(Reason::Store)
        );
    }

    #[test]
    fn refuses_a_write_the_calling_convention_forbids() {
        for register in [18, 19, 29, 30] {
            assert_eq!(
                check(|a| {
                    a.movz(Reg(register), 1);
                    a.movn(X0, 0);
                    a.ret();
                }),
                Err(Reason::Written),
                "x{register}"
            );
        }
        // The stack pointer, which no accepted instruction may name.
        assert_eq!(
            check(|a| {
                raw(a, 0x9100_23ff);
                a.movn(X0, 0);
                a.ret();
            }),
            Err(Reason::Undecodable)
        );
    }

    #[test]
    fn refuses_a_narrow_comparison_of_a_position() {
        assert_eq!(
            check(|a| {
                a.cmp_imm32(X2, 0);
                a.movn(X0, 0);
                a.ret();
            }),
            Err(Reason::Compare)
        );
    }

    #[test]
    fn refuses_control_flow_that_leaves_the_code() {
        assert_eq!(
            check(|a| {
                raw(a, 0x1400_0064);
            }),
            Err(Reason::Branch)
        );
        assert_eq!(
            check(|a| {
                a.movz(X5, 1);
            }),
            Err(Reason::FallsOffEnd)
        );
        assert_eq!(check(|_| {}), Err(Reason::Undecodable));
        assert_eq!(
            check(|a| {
                raw(a, 0);
            }),
            Err(Reason::Undecodable)
        );
    }

    #[test]
    fn refuses_a_return_of_anything_but_a_position_or_a_code() {
        // The subject's address, which is what x0 holds on entry.
        assert_eq!(check(|a| a.ret()), Err(Reason::Return));
        assert_eq!(
            check(|a| {
                a.movn(X0, 2);
                a.ret();
            }),
            Err(Reason::Return)
        );
        assert_eq!(
            check(|a| {
                a.mov(X0, X1);
                a.ret();
            }),
            Ok(())
        );
    }

    #[test]
    fn requires_every_loop_to_spend_budget() {
        let guarded = |budget: Reg| {
            check(move |a| {
                let top = a.here();
                let out = a.cbz_forward(budget);
                a.sub_imm(budget, budget, 1);
                a.b_back(top);
                a.bind(out);
                a.movn(X0, 1);
                a.ret();
            })
        };
        assert_eq!(guarded(X4), Ok(()));
        assert_eq!(guarded(X5), Err(Reason::Budget));
        // No guard at all.
        assert_eq!(
            check(|a| {
                let top = a.here();
                a.cmp(X2, X1);
                a.b_cond_back(Encoded::Ne, top);
                a.movn(X0, 0);
                a.ret();
            }),
            Err(Reason::Budget)
        );
        // A decrement with no test, which wraps an empty budget to the
        // largest one.
        assert_eq!(
            check(|a| {
                let top = a.here();
                a.movz(X5, 0);
                a.sub_imm(X4, X4, 1);
                a.b_back(top);
            }),
            Err(Reason::Budget)
        );
        // A guard skipped by branching into it.
        assert_eq!(
            check(|a| {
                a.cmp(X2, X1);
                let skip = a.b_cond_forward(Encoded::Eq);
                let top = a.here();
                let out = a.cbz_forward(X4);
                a.bind(skip);
                a.sub_imm(X4, X4, 1);
                a.b_back(top);
                a.bind(out);
                a.movn(X0, 1);
                a.ret();
            }),
            Err(Reason::Budget)
        );
        // A budget refilled inside the loop it bounds.
        assert_eq!(
            check(|a| {
                let top = a.here();
                a.movz(X4, 100);
                let out = a.cbz_forward(X4);
                a.sub_imm(X4, X4, 1);
                a.b_back(top);
                a.bind(out);
                a.movn(X0, 1);
                a.ret();
            }),
            Err(Reason::Budget)
        );
    }

    #[test]
    fn needs_scratch_for_every_instruction() {
        let mut code = [0u8; 8];
        let mut assembler = Assembler::new(&mut code);
        assembler.movn(X0, 0);
        assembler.ret();
        let length = assembler.finish().unwrap();
        let mut facts = [Facts::default(); 1];
        assert_eq!(
            verify(Target::A64, &code[..length], 0, &mut facts).map_err(|error| error.reason),
            Err(Reason::Scratch)
        );
    }
}
