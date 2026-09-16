//! x86-64 instruction encoding for the compilation tier described in
//! `docs/compilation.md`.
//!
//! This emits bytes into a caller-owned buffer and nothing else. It does not
//! map memory, does not make anything executable and does not call anything:
//! those are the host's, which is what keeps `#![forbid(unsafe_code)]` on this
//! crate.
//!
//! The target is the System V calling convention, which is what Linux and
//! macOS use: arguments in `RDI`, `RSI`, `RDX`, `RCX` and `R8`, the result in
//! `RAX`, and `RBX`, `RBP` and `R12`-`R15` preserved across a call. Generated
//! code uses those six, so it saves and restores them; it touches no other
//! memory than the subject and the caller's registers, and never the stack
//! beyond that.
//!
//! Every encoding below was checked against the system assembler rather than
//! written from memory, and the tests assert those exact bytes.
#![allow(dead_code)]
use super::machine;
use super::machine::{EncodeError, Kind, Label, Patch};

/// A general-purpose register, in the architectural numbering: the low three
/// bits go in an instruction's fields and the fourth in its `REX` prefix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Reg(pub u8);

pub(crate) const RAX: Reg = Reg(0);
pub(crate) const RCX: Reg = Reg(1);
pub(crate) const RDX: Reg = Reg(2);
pub(crate) const RBX: Reg = Reg(3);
pub(crate) const RSP: Reg = Reg(4);
pub(crate) const RBP: Reg = Reg(5);
pub(crate) const RSI: Reg = Reg(6);
pub(crate) const RDI: Reg = Reg(7);
pub(crate) const R8: Reg = Reg(8);
pub(crate) const R9: Reg = Reg(9);
pub(crate) const R10: Reg = Reg(10);
pub(crate) const R11: Reg = Reg(11);
pub(crate) const R12: Reg = Reg(12);
pub(crate) const R13: Reg = Reg(13);
pub(crate) const R14: Reg = Reg(14);
pub(crate) const R15: Reg = Reg(15);

/// A condition, in the encoding that follows `0F 8x`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Cond {
    Equal = 0x4,
    NotEqual = 0x5,
    /// Below, unsigned.
    Below = 0x2,
    /// Above or equal, unsigned.
    AtLeast = 0x3,
    /// Above, unsigned.
    Above = 0x7,
    /// Below or equal, unsigned.
    AtMost = 0x6,
}

/// Registers the convention preserves, which generated code saves on entry and
/// restores before every return, in this order.
const PRESERVED: [Reg; 6] = [RBX, RBP, R12, R13, R14, R15];

/// Emits x86-64 instructions into a caller-owned byte buffer.
///
/// Instructions are variable-length, so a branch's displacement sits at a
/// different offset in each form; a [`Patch`] records which form it came from
/// and `bind` fills the right field.
pub(crate) struct Assembler<'a> {
    code: &'a mut [u8],
    at: usize,
    failed: Option<EncodeError>,
    /// The window a lookbehind reads, as the position it ends at and how many
    /// bytes it holds. x86-64 addresses a byte of it directly, so establishing
    /// one emits nothing.
    window: Option<(Reg, u32)>,
}

impl<'a> Assembler<'a> {
    pub(crate) fn new(code: &'a mut [u8]) -> Self {
        Self {
            code,
            at: 0,
            failed: None,
            window: None,
        }
    }

    pub(crate) fn position(&self) -> usize {
        self.at
    }

    /// The bytes written, or the first error that stopped them being written.
    pub(crate) fn finish(self) -> Result<usize, EncodeError> {
        match self.failed {
            Some(error) => Err(error),
            None => Ok(self.at),
        }
    }

    fn fail(&mut self, error: EncodeError) {
        self.failed.get_or_insert(error);
    }

    fn byte(&mut self, value: u8) {
        if self.failed.is_some() {
            return;
        }
        let Some(slot) = self.code.get_mut(self.at) else {
            self.fail(EncodeError::Capacity);
            return;
        };
        *slot = value;
        self.at += 1;
    }

    fn bytes(&mut self, values: &[u8]) {
        for &value in values {
            self.byte(value);
        }
    }

    /// The position the next instruction will occupy, as a branch target.
    pub(crate) fn here(&self) -> Label {
        Label(self.at)
    }

    /// `REX`, emitted only when it says something: a 64-bit operand, or a
    /// register the three-bit fields cannot reach.
    fn rex(&mut self, wide: bool, reg: Reg, index: Reg, base: Reg) {
        let value =
            0x40 | u8::from(wide) << 3 | (reg.0 >> 3) << 2 | (index.0 >> 3) << 1 | (base.0 >> 3);
        if value != 0x40 {
            self.byte(value);
        }
    }

    fn modrm(&mut self, mode: u8, reg: Reg, rm: Reg) {
        self.byte(mode << 6 | (reg.0 & 7) << 3 | (rm.0 & 7));
    }

    fn sib(&mut self, scale: u8, index: Reg, base: Reg) {
        self.byte(scale << 6 | (index.0 & 7) << 3 | (base.0 & 7));
    }

    // -- Stack ---------------------------------------------------------------

    pub(crate) fn push(&mut self, reg: Reg) {
        if reg.0 >= 8 {
            self.byte(0x41);
        }
        self.byte(0x50 + (reg.0 & 7));
    }

    pub(crate) fn pop(&mut self, reg: Reg) {
        if reg.0 >= 8 {
            self.byte(0x41);
        }
        self.byte(0x58 + (reg.0 & 7));
    }

    // -- Moves and arithmetic ------------------------------------------------

    /// `dst = src`, whole registers.
    pub(crate) fn mov(&mut self, dst: Reg, src: Reg) {
        self.rex(true, src, Reg(0), dst);
        self.byte(0x89);
        self.modrm(3, src, dst);
    }

    /// `dst = src`, low halves, which is what a byte read leaves zero-extended.
    pub(crate) fn mov32(&mut self, dst: Reg, src: Reg) {
        self.rex(false, src, Reg(0), dst);
        self.byte(0x89);
        self.modrm(3, src, dst);
    }

    /// `dst = value`, sign-extended from 32 bits, which is how the two codes a
    /// search returns are written.
    pub(crate) fn mov_imm(&mut self, dst: Reg, value: i32) {
        self.rex(true, Reg(0), Reg(0), dst);
        self.byte(0xc7);
        self.modrm(3, Reg(0), dst);
        self.bytes(&value.to_le_bytes());
    }

    /// `dst = base + offset`, without reading memory.
    pub(crate) fn lea(&mut self, dst: Reg, base: Reg, offset: i32) {
        self.rex(true, dst, Reg(0), base);
        self.byte(0x8d);
        self.memory(dst, base, offset);
    }

    /// `dst = base + index`, without reading memory.
    pub(crate) fn lea_index(&mut self, dst: Reg, base: Reg, index: Reg) {
        self.rex(true, dst, index, base);
        self.byte(0x8d);
        self.modrm(0, dst, Reg(4));
        self.sib(0, index, base);
        if base.0 & 7 == 5 {
            self.fail(EncodeError::Immediate);
        }
    }

    /// `dst -= 1`.
    pub(crate) fn dec(&mut self, dst: Reg) {
        self.rex(true, Reg(0), Reg(0), dst);
        self.byte(0xff);
        self.modrm(3, Reg(1), dst);
    }

    /// `dst -= value`, low halves.
    pub(crate) fn sub32_imm(&mut self, dst: Reg, value: u32) {
        self.rex(false, Reg(0), Reg(0), dst);
        self.immediate(5, dst, value);
    }

    // -- Comparisons ---------------------------------------------------------

    /// Compare whole registers, for a branch that follows.
    pub(crate) fn cmp(&mut self, left: Reg, right: Reg) {
        self.rex(true, right, Reg(0), left);
        self.byte(0x39);
        self.modrm(3, right, left);
    }

    /// Compare a whole register with a constant.
    pub(crate) fn cmp_imm(&mut self, left: Reg, value: u32) {
        self.rex(true, Reg(0), Reg(0), left);
        self.immediate(7, left, value);
    }

    /// Compare the low half of a register with a constant.
    pub(crate) fn cmp32_imm(&mut self, left: Reg, value: u32) {
        self.rex(false, Reg(0), Reg(0), left);
        self.immediate(7, left, value);
    }

    /// The operand and immediate of an arithmetic instruction, in the short
    /// form when the constant fits a byte. Both forms exist for every
    /// operation here, and the assembler picks the short one.
    fn immediate(&mut self, operation: u8, reg: Reg, value: u32) {
        if let Ok(byte) = i8::try_from(value as i32) {
            self.byte(0x83);
            self.modrm(3, Reg(operation), reg);
            self.byte(byte as u8);
        } else {
            self.byte(0x81);
            self.modrm(3, Reg(operation), reg);
            self.bytes(&value.to_le_bytes());
        }
    }

    /// Set the flags from a register against itself, which is how a zero test
    /// is written.
    pub(crate) fn test(&mut self, reg: Reg) {
        self.rex(true, reg, Reg(0), reg);
        self.byte(0x85);
        self.modrm(3, reg, reg);
    }

    // -- Memory --------------------------------------------------------------

    /// The `ModRM`, `SIB` and displacement of `[base + offset]`.
    fn memory(&mut self, reg: Reg, base: Reg, offset: i32) {
        let mode = if offset == 0 && base.0 & 7 != 5 {
            0
        } else if i8::try_from(offset).is_ok() {
            1
        } else {
            2
        };
        if base.0 & 7 == 4 {
            self.modrm(mode, reg, Reg(4));
            self.sib(0, Reg(4), base);
        } else {
            self.modrm(mode, reg, base);
        }
        match mode {
            0 => {}
            1 => self.byte(offset as u8),
            _ => self.bytes(&offset.to_le_bytes()),
        }
    }

    /// `dst = base[index + offset]`, one byte, zero-extended.
    pub(crate) fn movzx(&mut self, dst: Reg, base: Reg, index: Reg, offset: i32) {
        self.rex(false, dst, index, base);
        self.bytes(&[0x0f, 0xb6]);
        let mode = if offset == 0 && base.0 & 7 != 5 {
            0
        } else if i8::try_from(offset).is_ok() {
            1
        } else {
            2
        };
        self.modrm(mode, dst, Reg(4));
        self.sib(0, index, base);
        match mode {
            0 => {}
            1 => self.byte(offset as u8),
            _ => self.bytes(&offset.to_le_bytes()),
        }
    }

    /// `base[offset] = src`, a whole register.
    pub(crate) fn store(&mut self, base: Reg, offset: i32, src: Reg) {
        self.rex(true, src, Reg(0), base);
        self.byte(0x89);
        self.memory(src, base, offset);
    }

    // -- Control flow --------------------------------------------------------

    pub(crate) fn ret(&mut self) {
        self.byte(0xc3);
    }

    /// A conditional branch to somewhere not emitted yet.
    pub(crate) fn jcc_forward(&mut self, cond: Cond) -> Patch {
        let at = self.at;
        self.bytes(&[0x0f, 0x80 | cond as u8, 0, 0, 0, 0]);
        Patch {
            at,
            kind: Kind::CondBranch,
            live: true,
        }
    }

    /// An unconditional branch to somewhere not emitted yet.
    pub(crate) fn jmp_forward(&mut self) -> Patch {
        let at = self.at;
        self.bytes(&[0xe9, 0, 0, 0, 0]);
        Patch {
            at,
            kind: Kind::Branch,
            live: true,
        }
    }

    /// An unconditional branch to somewhere already emitted.
    pub(crate) fn jmp_back(&mut self, to: Label) {
        let from = self.at as isize + 5;
        let offset = to.0 as isize - from;
        match i32::try_from(offset) {
            Ok(offset) => {
                self.byte(0xe9);
                self.bytes(&offset.to_le_bytes());
            }
            Err(_) => self.fail(EncodeError::Range),
        }
    }

    /// The address of data that follows the code, relative to this position.
    pub(crate) fn lea_rip(&mut self, dst: Reg) -> Patch {
        let at = self.at;
        self.rex(true, dst, Reg(0), Reg(0));
        self.byte(0x8d);
        self.modrm(0, dst, Reg(5));
        self.bytes(&[0, 0, 0, 0]);
        Patch {
            at,
            kind: Kind::Address,
            live: true,
        }
    }

    /// Bytes after the code that reads them.
    pub(crate) fn data(&mut self, bytes: &[u8]) {
        self.bytes(bytes);
    }

    /// Point an emitted reference at where the next instruction will go.
    pub(crate) fn bind(&mut self, patch: Patch) {
        if !patch.live || self.failed.is_some() {
            return;
        }
        // Every displacement here is four bytes counted from the end of the
        // instruction holding it, and each form puts it at its own offset.
        let field = patch.at
            + match patch.kind {
                Kind::CondBranch => 2,
                Kind::Branch => 1,
                // `REX`, opcode and `ModRM` precede it.
                Kind::Address => 3,
            };
        let offset = self.at as isize - (field + 4) as isize;
        let Ok(offset) = i32::try_from(offset) else {
            self.fail(EncodeError::Range);
            return;
        };
        let Some(slot) = self.code.get_mut(field..field + 4) else {
            self.fail(EncodeError::Capacity);
            return;
        };
        slot.copy_from_slice(&offset.to_le_bytes());
    }
}

/// The abstract slots the generator names, in this target's registers.
///
/// Thirteen slots and fifteen usable registers, so two are kept back: one for
/// operations the generator states as results rather than as instructions, and
/// one for the last start with room for a match. `RSP` is never touched after
/// the prologue, and `RAX` holds the result at every return.
fn slot(slot: machine::Slot) -> Reg {
    const REGISTERS: [Reg; machine::SLOTS] = [
        RDI, RSI, RDX, RCX, R8, R9, R10, R11, RBX, R12, R13, R14, R15,
    ];
    REGISTERS[slot.0 as usize]
}
/// Scratch this encoder uses for operations the generator states as results.
const SCRATCH: Reg = RAX;
/// The last start with room for everything a match must consume, fixed for the
/// whole search.
const LAST: Reg = RBP;

fn condition(cond: machine::Cond) -> Cond {
    match cond {
        machine::Cond::Equal => Cond::Equal,
        machine::Cond::NotEqual => Cond::NotEqual,
        machine::Cond::Less => Cond::Below,
        machine::Cond::AtLeast => Cond::AtLeast,
        machine::Cond::Greater => Cond::Above,
        machine::Cond::AtMost => Cond::AtMost,
    }
}

impl machine::Machine for Assembler<'_> {
    fn here(&self) -> Label {
        Assembler::here(self)
    }
    fn position(&self) -> usize {
        Assembler::position(self)
    }
    fn done(&mut self) -> Result<usize, EncodeError> {
        match self.failed {
            Some(error) => Err(error),
            None => Ok(self.at),
        }
    }

    fn enter(&mut self) {
        for reg in PRESERVED {
            self.push(reg);
        }
    }

    fn return_start(&mut self) {
        // The answer first, so that restoring what the convention preserves is
        // the last thing before the return and can be checked as one shape.
        self.mov(SCRATCH, slot(machine::FROM));
        self.leave();
        self.ret();
    }
    fn return_code(&mut self, code: i32) {
        self.mov_imm(SCRATCH, code);
        self.leave();
        self.ret();
    }

    fn copy(&mut self, dst: machine::Slot, src: machine::Slot) {
        self.mov(slot(dst), slot(src));
    }
    fn ahead(&mut self, dst: machine::Slot, src: machine::Slot, offset: u32) {
        match i32::try_from(offset) {
            Ok(offset) => self.lea(slot(dst), slot(src), offset),
            Err(_) => self.fail(EncodeError::Immediate),
        }
    }
    fn behind(&mut self, dst: machine::Slot, src: machine::Slot, offset: u32) {
        match i32::try_from(offset) {
            Ok(offset) => self.lea(slot(dst), slot(src), -offset),
            Err(_) => self.fail(EncodeError::Immediate),
        }
    }

    fn compare(&mut self, left: machine::Slot, right: machine::Slot) {
        Assembler::cmp(self, slot(left), slot(right));
    }
    fn compare_position(&mut self, left: machine::Slot, right: u32) {
        self.cmp_imm(slot(left), right);
    }
    fn compare_byte(&mut self, left: machine::Slot, right: u32) {
        self.cmp32_imm(slot(left), right);
    }
    fn byte_within(&mut self, byte: machine::Slot, lo: u32, hi: u32, inside: bool) -> Patch {
        // A byte is inside one range exactly when subtracting the low bound
        // leaves something no larger than the range is wide, which is one
        // branch rather than two.
        self.mov32(SCRATCH, slot(byte));
        self.sub32_imm(SCRATCH, lo);
        self.cmp32_imm(SCRATCH, hi - lo);
        self.jcc_forward(if inside { Cond::AtMost } else { Cond::Above })
    }

    fn load_byte(&mut self, dst: machine::Slot, base: machine::Slot, index: machine::Slot) {
        self.movzx(slot(dst), slot(base), slot(index), 0);
    }
    fn window_before(&mut self, position: machine::Slot, distance: u32) {
        // Nothing to emit: a byte of the window is one addressing mode away
        // from the position it ends at.
        self.window = Some((slot(position), distance));
    }
    fn load_window_byte(&mut self, dst: machine::Slot, offset: u32) {
        let Some((position, distance)) = self.window else {
            self.fail(EncodeError::Slots);
            return;
        };
        match (i32::try_from(offset), i32::try_from(distance)) {
            (Ok(offset), Ok(distance)) => self.movzx(
                slot(dst),
                slot(machine::SUBJECT),
                position,
                offset - distance,
            ),
            _ => self.fail(EncodeError::Immediate),
        }
    }
    fn store_position(&mut self, value: machine::Slot, index: u32) {
        match i32::try_from(index * 8) {
            Ok(offset) => self.store(slot(machine::REGISTERS), offset, slot(value)),
            Err(_) => self.fail(EncodeError::Immediate),
        }
    }

    fn fix_room(&mut self, least: u32) -> Patch {
        self.cmp_imm(slot(machine::LENGTH), least);
        let short = self.jcc_forward(Cond::Below);
        match i32::try_from(least) {
            Ok(least) => self.lea(LAST, slot(machine::LENGTH), -least),
            Err(_) => self.fail(EncodeError::Immediate),
        }
        short
    }
    fn start_without_room(&mut self) -> Patch {
        Assembler::cmp(self, slot(machine::FROM), LAST);
        self.jcc_forward(Cond::Above)
    }

    fn limit_to(&mut self, bound: machine::Slot, from: machine::Slot, offset: u32) {
        match i32::try_from(offset) {
            Ok(offset) => self.lea(SCRATCH, slot(from), offset),
            Err(_) => self.fail(EncodeError::Immediate),
        }
        Assembler::cmp(self, SCRATCH, slot(bound));
        let already = self.jcc_forward(Cond::AtLeast);
        self.mov(slot(bound), SCRATCH);
        Assembler::bind(self, already);
    }

    fn branch_if(&mut self, cond: machine::Cond) -> Patch {
        self.jcc_forward(condition(cond))
    }
    fn branch(&mut self) -> Patch {
        self.jmp_forward()
    }
    fn branch_back(&mut self, to: Label) -> Patch {
        // Every loop closes through this shape, so the budget is decremented
        // only when it is not already zero and never wraps, and between two
        // backward branches execution only moves forward.
        self.test(slot(machine::BUDGET));
        let out = self.jcc_forward(Cond::Equal);
        self.dec(slot(machine::BUDGET));
        self.jmp_back(to);
        out
    }
    fn bind(&mut self, patch: Patch) {
        Assembler::bind(self, patch);
    }

    fn data_address(&mut self, dst: machine::Slot) -> Patch {
        self.lea_rip(slot(dst))
    }
    fn data(&mut self, bytes: &[u8]) {
        Assembler::data(self, bytes);
    }
}

impl Assembler<'_> {
    /// Restore what [`machine::Machine::enter`] saved, before a return.
    fn leave(&mut self) {
        for reg in PRESERVED.into_iter().rev() {
            self.pop(reg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bytes a build emits, which the tests below compare with what the
    /// system assembler produces for the same instruction.
    fn assemble<const N: usize>(build: impl FnOnce(&mut Assembler<'_>)) -> [u8; N] {
        let mut code = [0u8; 128];
        let length = {
            let mut asm = Assembler::new(&mut code);
            build(&mut asm);
            asm.finish().expect("encodes")
        };
        assert_eq!(length, N, "emitted {length} bytes, expected {N}");
        let mut out = [0u8; N];
        out.copy_from_slice(&code[..N]);
        out
    }

    #[test]
    fn encodes_what_the_assembler_encodes() {
        // Each of these was taken from `clang -target x86_64` on the
        // corresponding instruction, not written from memory.
        assert_eq!(assemble(|a| a.push(RBX)), [0x53]);
        assert_eq!(assemble(|a| a.push(R12)), [0x41, 0x54]);
        assert_eq!(assemble(|a| a.pop(R12)), [0x41, 0x5c]);
        assert_eq!(assemble(|a| a.pop(RBX)), [0x5b]);
        assert_eq!(assemble(|a| a.mov(R10, RDX)), [0x49, 0x89, 0xd2]);
        assert_eq!(assemble(|a| a.mov(RBX, R8)), [0x4c, 0x89, 0xc3]);
        assert_eq!(
            assemble(|a| a.mov_imm(RAX, -1)),
            [0x48, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff]
        );
        assert_eq!(
            assemble(|a| a.mov_imm(RAX, -2)),
            [0x48, 0xc7, 0xc0, 0xfe, 0xff, 0xff, 0xff]
        );
        assert_eq!(assemble(|a| a.lea(R11, R10, 5)), [0x4d, 0x8d, 0x5a, 0x05]);
        assert_eq!(assemble(|a| a.lea(R11, R10, -5)), [0x4d, 0x8d, 0x5a, 0xfb]);
        assert_eq!(assemble(|a| a.cmp(RDX, RSI)), [0x48, 0x39, 0xf2]);
        assert_eq!(assemble(|a| a.cmp_imm(RDX, 32)), [0x48, 0x83, 0xfa, 0x20]);
        assert_eq!(assemble(|a| a.cmp32_imm(R11, 97)), [0x41, 0x83, 0xfb, 0x61]);
        assert_eq!(
            assemble(|a| a.movzx(R11, RDI, R10, 0)),
            [0x46, 0x0f, 0xb6, 0x1c, 0x17]
        );
        assert_eq!(
            assemble(|a| a.movzx(R11, RDI, R10, 7)),
            [0x46, 0x0f, 0xb6, 0x5c, 0x17, 0x07]
        );
        assert_eq!(
            assemble(|a| a.movzx(R11, R9, R11, 0)),
            [0x47, 0x0f, 0xb6, 0x1c, 0x19]
        );
        assert_eq!(
            assemble(|a| a.store(RCX, 24, R10)),
            [0x4c, 0x89, 0x51, 0x18]
        );
        assert_eq!(assemble(|a| a.test(R8)), [0x4d, 0x85, 0xc0]);
        assert_eq!(assemble(|a| a.dec(R8)), [0x49, 0xff, 0xc8]);
        assert_eq!(assemble(|a| a.ret()), [0xc3]);
    }

    #[test]
    fn encodes_a_forward_branch_over_one_instruction() {
        // `jne` over a three-byte `mov`, which its displacement counts from
        // the end of the branch rather than from its start.
        let code = assemble(|a| {
            let patch = a.jcc_forward(Cond::NotEqual);
            a.mov(R10, RDX);
            a.bind(patch);
            a.ret();
        });
        assert_eq!(
            code,
            [0x0f, 0x85, 0x03, 0x00, 0x00, 0x00, 0x49, 0x89, 0xd2, 0xc3]
        );
    }

    #[test]
    fn encodes_a_backward_branch() {
        let code = assemble(|a| {
            let top = a.here();
            a.dec(R8);
            a.jmp_back(top);
        });
        // Three bytes of `dec` and five of `jmp` behind it.
        assert_eq!(code, [0x49, 0xff, 0xc8, 0xe9, 0xf8, 0xff, 0xff, 0xff]);
    }

    #[test]
    fn writes_data_after_the_code_that_reads_it() {
        let code = assemble(|a| {
            let patch = a.lea_rip(R9);
            a.ret();
            a.bind(patch);
            a.data(&[1, 2, 3, 4]);
        });
        // The address counts from the end of the `lea`, so one byte of `ret`
        // separates them.
        assert_eq!(
            code,
            [0x4c, 0x8d, 0x0d, 0x01, 0x00, 0x00, 0x00, 0xc3, 1, 2, 3, 4]
        );
    }

    #[test]
    fn reports_a_buffer_too_small_instead_of_truncating() {
        let mut code = [0u8; 2];
        let mut asm = Assembler::new(&mut code);
        asm.mov(R10, RDX);
        assert_eq!(asm.finish(), Err(EncodeError::Capacity));
    }

    #[test]
    fn keeps_the_first_failure() {
        let mut code = [0u8; 4];
        let mut asm = Assembler::new(&mut code);
        asm.mov(R10, RDX);
        asm.mov(R10, RDX);
        asm.ret();
        assert_eq!(asm.finish(), Err(EncodeError::Capacity));
    }
}
