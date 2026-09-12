//! AArch64 instruction encoding for the compilation tier described in
//! `docs/compilation.md`.
//!
//! This emits bytes into a caller-owned buffer and nothing else. It does not
//! map memory, does not make anything executable and does not call anything:
//! those are the host's, which is what keeps `#![forbid(unsafe_code)]` on this
//! crate. Nothing here is reachable from a search yet; stage 2 is the encoder,
//! and the code generator that drives it is stage 2's remainder.
//!
//! Every encoding below was checked against the system assembler rather than
//! written from memory, and the tests assert those exact words.
#![allow(dead_code)]

/// A general-purpose register. `X0`-`X30`, and 31 which reads as the zero
/// register or the stack pointer depending on the instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Reg(pub u8);

pub(crate) const X0: Reg = Reg(0);
pub(crate) const X1: Reg = Reg(1);
pub(crate) const X2: Reg = Reg(2);
pub(crate) const X3: Reg = Reg(3);
pub(crate) const X4: Reg = Reg(4);
pub(crate) const X5: Reg = Reg(5);
pub(crate) const ZR: Reg = Reg(31);
/// The link register, which `RET` returns through.
pub(crate) const LR: Reg = Reg(30);

/// A branch condition, in its architectural encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Cond {
    Eq = 0,
    Ne = 1,
    /// Unsigned lower.
    Lo = 3,
    /// Unsigned higher or same.
    Hs = 2,
    /// Unsigned higher.
    Hi = 8,
    /// Unsigned lower or same.
    Ls = 9,
}

/// A position in the emitted code that a branch can be pointed at.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Label(usize);

/// What an unbound reference will become once its target is known.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Kind {
    #[default]
    CondBranch,
    Branch,
    /// A PC-relative address rather than a jump, whose offset counts bytes.
    Address,
}

/// A reference that has been emitted but whose target is not known yet. The
/// default is one that was never emitted, which [`Assembler::bind`] ignores, so
/// a generator can carry a fixed array of them.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Patch {
    at: usize,
    kind: Kind,
    live: bool,
}

/// Why an encoding could not be produced. Every one of these is a bug in the
/// code generator rather than anything a pattern can cause, but the encoder
/// reports instead of wrapping so that a bug cannot silently emit a different
/// instruction than it meant to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeError {
    /// The caller's buffer could not hold the code.
    Capacity,
    /// An immediate did not fit the field the instruction has for it.
    Immediate,
    /// A branch was further from its target than its field can reach.
    Range,
}

/// Emits AArch64 instructions into a caller-owned byte buffer.
///
/// Instructions are four bytes, little-endian, which is the only order AArch64
/// is defined for. The buffer is the host's; this never grows it, and reports
/// [`EncodeError::Capacity`] rather than truncating.
pub(crate) struct Assembler<'a> {
    code: &'a mut [u8],
    at: usize,
    failed: Option<EncodeError>,
}

impl<'a> Assembler<'a> {
    pub(crate) fn new(code: &'a mut [u8]) -> Self {
        Self {
            code,
            at: 0,
            failed: None,
        }
    }

    /// Bytes emitted so far, which is also where the next instruction goes.
    pub(crate) fn position(&self) -> usize {
        self.at
    }

    /// The bytes written, or the first error that stopped them being written.
    /// Errors are held rather than returned per instruction so a generator can
    /// emit a whole program and check once, without a failure part-way leaving
    /// a buffer that looks complete.
    pub(crate) fn finish(self) -> Result<usize, EncodeError> {
        match self.failed {
            Some(error) => Err(error),
            None => Ok(self.at),
        }
    }

    fn fail(&mut self, error: EncodeError) {
        self.failed.get_or_insert(error);
    }

    fn word(&mut self, instruction: u32) {
        if self.failed.is_some() {
            return;
        }
        let Some(slot) = self.code.get_mut(self.at..self.at + 4) else {
            self.fail(EncodeError::Capacity);
            return;
        };
        slot.copy_from_slice(&instruction.to_le_bytes());
        self.at += 4;
    }

    /// The position the next instruction will occupy, as a branch target.
    pub(crate) fn here(&self) -> Label {
        Label(self.at)
    }

    // -- Moves and arithmetic ------------------------------------------------

    /// `MOV Xd, #imm16`, which is `MOVZ` with no shift.
    pub(crate) fn movz(&mut self, rd: Reg, imm: u16) {
        self.word(0xd280_0000 | (u32::from(imm) << 5) | u32::from(rd.0));
    }

    /// `MOV Xd, #-(imm+1)`, which is `MOVN` with no shift. `movn(rd, 0)` is
    /// `MOV Xd, #-1`.
    pub(crate) fn movn(&mut self, rd: Reg, imm: u16) {
        self.word(0x9280_0000 | (u32::from(imm) << 5) | u32::from(rd.0));
    }

    /// `MOV Xd, Xn`, which is `ORR Xd, XZR, Xn`.
    pub(crate) fn mov(&mut self, rd: Reg, rn: Reg) {
        self.word(0xaa00_03e0 | (u32::from(rn.0) << 16) | u32::from(rd.0));
    }

    /// `ADD Xd, Xn, #imm12`.
    pub(crate) fn add_imm(&mut self, rd: Reg, rn: Reg, imm: u32) {
        if imm >= 1 << 12 {
            self.fail(EncodeError::Immediate);
            return;
        }
        self.word(0x9100_0000 | (imm << 10) | (u32::from(rn.0) << 5) | u32::from(rd.0));
    }

    /// `SUB Xd, Xn, #imm12`.
    pub(crate) fn sub_imm(&mut self, rd: Reg, rn: Reg, imm: u32) {
        if imm >= 1 << 12 {
            self.fail(EncodeError::Immediate);
            return;
        }
        self.word(0xd100_0000 | (imm << 10) | (u32::from(rn.0) << 5) | u32::from(rd.0));
    }

    // -- Comparison ----------------------------------------------------------

    /// `CMP Wn, #imm12`, which is `SUBS WZR, Wn, #imm12`. The 32-bit form,
    /// because the values it compares are bytes.
    pub(crate) fn cmp_imm32(&mut self, rn: Reg, imm: u32) {
        if imm >= 1 << 12 {
            self.fail(EncodeError::Immediate);
            return;
        }
        self.word(0x7100_001f | (imm << 10) | (u32::from(rn.0) << 5));
    }

    /// `CMP Xn, Xm`, which is `SUBS XZR, Xn, Xm`.
    pub(crate) fn cmp(&mut self, rn: Reg, rm: Reg) {
        self.word(0xeb00_001f | (u32::from(rm.0) << 16) | (u32::from(rn.0) << 5));
    }

    // -- Memory --------------------------------------------------------------

    /// `LDRB Wt, [Xn, Xm]`, a zero-extending byte load at a register offset.
    pub(crate) fn ldrb(&mut self, rt: Reg, rn: Reg, rm: Reg) {
        self.word(0x3860_6800 | (u32::from(rm.0) << 16) | (u32::from(rn.0) << 5) | u32::from(rt.0));
    }

    /// `STR Xt, [Xn, #index * 8]`, the scaled unsigned-offset form, so `index`
    /// counts registers rather than bytes.
    pub(crate) fn str_index(&mut self, rt: Reg, rn: Reg, index: u32) {
        if index >= 1 << 12 {
            self.fail(EncodeError::Immediate);
            return;
        }
        self.word(0xf900_0000 | (index << 10) | (u32::from(rn.0) << 5) | u32::from(rt.0));
    }

    /// `LDRB Wt, [Xn, #offset]`, a zero-extending byte load at a fixed offset.
    pub(crate) fn ldrb_imm(&mut self, rt: Reg, rn: Reg, offset: u32) {
        if offset >= 1 << 12 {
            self.fail(EncodeError::Immediate);
            return;
        }
        self.word(0x3940_0000 | (offset << 10) | (u32::from(rn.0) << 5) | u32::from(rt.0));
    }

    /// `ADD Xd, Xn, Xm`.
    pub(crate) fn add_reg(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.word(0x8b00_0000 | (u32::from(rm.0) << 16) | (u32::from(rn.0) << 5) | u32::from(rd.0));
    }

    /// `SUB Wd, Wn, #imm12`, the 32-bit form, for reducing a byte to an offset
    /// within a range.
    pub(crate) fn sub_imm32(&mut self, rd: Reg, rn: Reg, imm: u32) {
        if imm >= 1 << 12 {
            self.fail(EncodeError::Immediate);
            return;
        }
        self.word(0x5100_0000 | (imm << 10) | (u32::from(rn.0) << 5) | u32::from(rd.0));
    }

    /// `ADR Xd, label`, a PC-relative address for something not emitted yet.
    pub(crate) fn adr_forward(&mut self, rd: Reg) -> Patch {
        let at = self.at;
        self.word(0x1000_0000 | u32::from(rd.0));
        Patch {
            at,
            kind: Kind::Address,
            live: true,
        }
    }

    /// Raw bytes, for a table the code reads rather than executes.
    pub(crate) fn data(&mut self, bytes: &[u8]) {
        if self.failed.is_some() {
            return;
        }
        let Some(slot) = self.code.get_mut(self.at..self.at + bytes.len()) else {
            self.fail(EncodeError::Capacity);
            return;
        };
        slot.copy_from_slice(bytes);
        self.at += bytes.len();
    }

    /// `LDR Xt, [Xn, #index * 8]`, the scaled unsigned-offset form, so `index`
    /// counts registers rather than bytes.
    pub(crate) fn ldr_index(&mut self, rt: Reg, rn: Reg, index: u32) {
        if index >= 1 << 12 {
            self.fail(EncodeError::Immediate);
            return;
        }
        self.word(0xf940_0000 | (index << 10) | (u32::from(rn.0) << 5) | u32::from(rt.0));
    }

    // -- Control flow --------------------------------------------------------

    /// `RET`, through the link register.
    pub(crate) fn ret(&mut self) {
        self.word(0xd65f_0000 | (u32::from(LR.0) << 5));
    }

    /// `B.cond` to a label already placed.
    pub(crate) fn b_cond_back(&mut self, cond: Cond, to: Label) {
        let Some(offset) = self.offset_words(to.0) else {
            return;
        };
        if !fits_signed(offset, 19) {
            self.fail(EncodeError::Range);
            return;
        }
        self.word(0x5400_0000 | ((offset as u32 & 0x7_ffff) << 5) | (cond as u32));
    }

    /// `B` to a label already placed.
    pub(crate) fn b_back(&mut self, to: Label) {
        let Some(offset) = self.offset_words(to.0) else {
            return;
        };
        if !fits_signed(offset, 26) {
            self.fail(EncodeError::Range);
            return;
        }
        self.word(0x1400_0000 | (offset as u32 & 0x3ff_ffff));
    }

    /// `B.cond` to somewhere not emitted yet. Bind the returned patch once the
    /// target is known; a patch that is never bound leaves a branch to itself,
    /// which is why the generator is responsible for binding every one it takes.
    pub(crate) fn b_cond_forward(&mut self, cond: Cond) -> Patch {
        let at = self.at;
        self.word(0x5400_0000 | (cond as u32));
        Patch {
            at,
            kind: Kind::CondBranch,
            live: true,
        }
    }

    /// `B` to somewhere not emitted yet.
    pub(crate) fn b_forward(&mut self) -> Patch {
        let at = self.at;
        self.word(0x1400_0000);
        Patch {
            at,
            kind: Kind::Branch,
            live: true,
        }
    }

    /// Point a forward branch at the next instruction to be emitted.
    pub(crate) fn bind(&mut self, patch: Patch) {
        if self.failed.is_some() || !patch.live {
            return;
        }
        let Some(distance) = (self.at as isize).checked_sub(patch.at as isize) else {
            self.fail(EncodeError::Range);
            return;
        };
        let (value, bits, shift, mask) = match patch.kind {
            // A branch counts instructions; an address counts bytes.
            Kind::CondBranch => (distance / 4, 19, 5, 0x7_ffff),
            Kind::Branch => (distance / 4, 26, 0, 0x3ff_ffff),
            Kind::Address => (distance, 21, 0, 0),
        };
        if !fits_signed(value, bits) {
            self.fail(EncodeError::Range);
            return;
        }
        let Some(slot) = self.code.get_mut(patch.at..patch.at + 4) else {
            self.fail(EncodeError::Capacity);
            return;
        };
        let mut instruction = u32::from_le_bytes([slot[0], slot[1], slot[2], slot[3]]);
        instruction |= match patch.kind {
            Kind::Address => {
                let value = value as u32;
                ((value & 3) << 29) | ((value >> 2) << 5)
            }
            _ => (value as u32 & mask) << shift,
        };
        slot.copy_from_slice(&instruction.to_le_bytes());
    }

    fn offset_words(&mut self, to: usize) -> Option<isize> {
        let distance = (to as isize).checked_sub(self.at as isize)?;
        Some(distance / 4)
    }
}

/// Whether a word offset fits a signed immediate of `bits` bits.
fn fits_signed(value: isize, bits: u32) -> bool {
    let limit = 1isize << (bits - 1);
    value >= -limit && value < limit
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every expectation here came from the system assembler, not from reading
    /// the encoding tables: `as -arch arm64` on the same instruction text.
    fn assemble<const N: usize>(build: impl FnOnce(&mut Assembler<'_>)) -> [u32; N] {
        let mut code = [0u8; 256];
        let mut assembler = Assembler::new(&mut code);
        build(&mut assembler);
        let length = assembler.finish().expect("encodes");
        assert_eq!(length, N * 4, "emitted a different number of instructions");
        let mut words = [0u32; N];
        for (word, bytes) in words.iter_mut().zip(code[..length].chunks_exact(4)) {
            *word = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        words
    }

    #[test]
    fn encodes_what_the_assembler_encodes() {
        assert_eq!(assemble(|a| a.ret()), [0xd65f_03c0]);
        assert_eq!(assemble(|a| a.movz(X0, 0)), [0xd280_0000]);
        assert_eq!(assemble(|a| a.movz(X0, 1)), [0xd280_0020]);
        assert_eq!(assemble(|a| a.movz(X3, 65535)), [0xd29f_ffe3]);
        assert_eq!(assemble(|a| a.ldrb(X4, X1, X2)), [0x3862_6824]);
        assert_eq!(assemble(|a| a.ldrb(X0, X5, Reg(6))), [0x3866_68a0]);
        assert_eq!(assemble(|a| a.cmp_imm32(X4, 97)), [0x7101_849f]);
        assert_eq!(assemble(|a| a.cmp_imm32(X0, 0)), [0x7100_001f]);
        assert_eq!(assemble(|a| a.cmp(X1, X2)), [0xeb02_003f]);
        assert_eq!(assemble(|a| a.add_imm(X0, X1, 8)), [0x9100_2020]);
        assert_eq!(assemble(|a| a.sub_imm(X0, X1, 1)), [0xd100_0420]);
        assert_eq!(assemble(|a| a.str_index(X2, X3, 2)), [0xf900_0862]);
        assert_eq!(assemble(|a| a.mov(Reg(7), Reg(8))), [0xaa08_03e7]);
        assert_eq!(assemble(|a| a.ldrb_imm(Reg(7), X4, 0)), [0x3940_0087]);
        assert_eq!(assemble(|a| a.ldrb_imm(Reg(7), X4, 1)), [0x3940_0487]);
        assert_eq!(assemble(|a| a.ldrb_imm(Reg(7), Reg(7), 7)), [0x3940_1ce7]);
        assert_eq!(assemble(|a| a.add_reg(X4, X0, Reg(6))), [0x8b06_0004]);
        assert_eq!(assemble(|a| a.sub_imm32(Reg(7), Reg(7), 97)), [0x5101_84e7]);
        assert_eq!(assemble(|a| a.movn(X0, 0)), [0x9280_0000]);
        assert_eq!(assemble(|a| a.movn(Reg(6), 1)), [0x9280_0026]);
        assert_eq!(assemble(|a| a.ldr_index(Reg(9), X3, 3)), [0xf940_0c69]);
    }

    #[test]
    fn encodes_a_forward_branch_over_one_instruction() {
        // `b.eq .+8` and `b .+8` are what the assembler emits for a branch over
        // a single instruction, which is what binding one here produces.
        assert_eq!(
            assemble(|a| {
                let patch = a.b_cond_forward(Cond::Eq);
                a.ret();
                a.bind(patch);
            }),
            [0x5400_0040, 0xd65f_03c0]
        );
        assert_eq!(
            assemble(|a| {
                let patch = a.b_forward();
                a.ret();
                a.bind(patch);
            }),
            [0x1400_0002, 0xd65f_03c0]
        );
    }

    #[test]
    fn encodes_a_backward_branch() {
        // Back to the instruction before the branch: minus one word.
        assert_eq!(
            assemble(|a| {
                let top = a.here();
                a.ret();
                a.b_back(top);
            }),
            [0xd65f_03c0, 0x17ff_ffff]
        );
        assert_eq!(
            assemble(|a| {
                let top = a.here();
                a.ret();
                a.b_cond_back(Cond::Ne, top);
            }),
            [0xd65f_03c0, 0x54ff_ffe1]
        );
    }

    #[test]
    fn encodes_a_pc_relative_address() {
        // `adr x5, .+8` over one instruction, and `adr x9, .+16` over three.
        assert_eq!(
            assemble(|a| {
                let patch = a.adr_forward(X5);
                a.ret();
                a.bind(patch);
            }),
            [0x1000_0045, 0xd65f_03c0]
        );
        assert_eq!(
            assemble(|a| {
                let patch = a.adr_forward(Reg(9));
                a.ret();
                a.ret();
                a.ret();
                a.bind(patch);
            }),
            [0x1000_0089, 0xd65f_03c0, 0xd65f_03c0, 0xd65f_03c0]
        );
    }

    #[test]
    fn writes_data_after_the_code_that_reads_it() {
        let mut code = [0u8; 32];
        let mut assembler = Assembler::new(&mut code);
        let table = assembler.adr_forward(X5);
        assembler.ret();
        assembler.bind(table);
        assembler.data(&[1, 0, 1, 0]);
        assert_eq!(assembler.finish(), Ok(12));
        assert_eq!(&code[8..12], &[1, 0, 1, 0]);
    }

    #[test]
    fn reports_a_buffer_too_small_instead_of_truncating() {
        let mut code = [0u8; 4];
        let mut assembler = Assembler::new(&mut code);
        assembler.ret();
        assembler.ret();
        assert_eq!(assembler.finish(), Err(EncodeError::Capacity));
    }

    #[test]
    fn reports_an_immediate_that_does_not_fit() {
        let mut code = [0u8; 16];
        let mut assembler = Assembler::new(&mut code);
        assembler.add_imm(X0, X1, 1 << 12);
        assert_eq!(assembler.finish(), Err(EncodeError::Immediate));
    }

    #[test]
    fn keeps_the_first_failure() {
        // A generator emits a whole program and checks once, so a later error
        // must not replace the one that actually stopped it.
        let mut code = [0u8; 16];
        let mut assembler = Assembler::new(&mut code);
        assembler.add_imm(X0, X1, 1 << 12);
        assembler.str_index(X0, X1, 1 << 13);
        assert_eq!(assembler.finish(), Err(EncodeError::Immediate));
    }
}
