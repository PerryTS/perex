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
use super::machine;
use super::machine::{EncodeError, Kind, Label, Patch};

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
pub(crate) const X16: Reg = Reg(16);
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

    /// `CMP Xn, #imm12`, the 64-bit form, for positions and lengths. Comparing
    /// only the low half of one would be wrong past four gigabytes.
    pub(crate) fn cmp_imm(&mut self, rn: Reg, imm: u32) {
        if imm >= 1 << 12 {
            self.fail(EncodeError::Immediate);
            return;
        }
        self.word(0xf100_001f | (imm << 10) | (u32::from(rn.0) << 5));
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

    /// `CBZ Xt` to somewhere not emitted yet. Its offset field is where a
    /// conditional branch keeps one, so it binds the same way.
    pub(crate) fn cbz_forward(&mut self, rt: Reg) -> Patch {
        let at = self.at;
        self.word(0xb400_0000 | u32::from(rt.0));
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

/// The abstract slots the generator names, in this target's registers. There
/// are more registers than slots here, so two are kept back: one for a window
/// into the subject, and one for the last start with room for a match.
fn slot(slot: machine::Slot) -> Reg {
    Reg(slot.0)
}
/// Scratch this encoder uses for operations the generator states as results
/// rather than as instructions.
const WINDOW: Reg = Reg(15);
/// The last start with room for everything a match must consume, fixed for the
/// whole search.
const LAST: Reg = Reg(16);

fn condition(cond: machine::Cond) -> Cond {
    match cond {
        machine::Cond::Equal => Cond::Eq,
        machine::Cond::NotEqual => Cond::Ne,
        machine::Cond::Less => Cond::Lo,
        machine::Cond::AtLeast => Cond::Hs,
        machine::Cond::Greater => Cond::Hi,
        machine::Cond::AtMost => Cond::Ls,
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
        // Everything this target uses is either an argument or scratch the
        // convention lets a call clobber, so there is nothing to save.
    }

    fn return_start(&mut self) {
        self.mov(X0, slot(machine::FROM));
        self.ret();
    }
    fn return_code(&mut self, code: i32) {
        // `MOVN` writes the bitwise complement, so the codes this returns —
        // -1 and -2 — are the complements of 0 and 1.
        self.movn(X0, (!code) as u16);
        self.ret();
    }

    fn copy(&mut self, dst: machine::Slot, src: machine::Slot) {
        self.mov(slot(dst), slot(src));
    }
    fn ahead(&mut self, dst: machine::Slot, src: machine::Slot, offset: u32) {
        self.add_imm(slot(dst), slot(src), offset);
    }
    fn behind(&mut self, dst: machine::Slot, src: machine::Slot, offset: u32) {
        self.sub_imm(slot(dst), slot(src), offset);
    }

    fn compare(&mut self, left: machine::Slot, right: machine::Slot) {
        Assembler::cmp(self, slot(left), slot(right));
    }
    fn compare_position(&mut self, left: machine::Slot, right: u32) {
        self.cmp_imm(slot(left), right);
    }
    fn compare_byte(&mut self, left: machine::Slot, right: u32) {
        self.cmp_imm32(slot(left), right);
    }
    fn byte_within(&mut self, byte: machine::Slot, lo: u32, hi: u32, inside: bool) -> Patch {
        // A byte is inside one range exactly when subtracting the low bound
        // leaves something no larger than the range is wide, which is one
        // subtraction and one comparison rather than two of each.
        self.sub_imm32(WINDOW, slot(byte), lo);
        self.cmp_imm32(WINDOW, hi - lo);
        self.b_cond_forward(if inside { Cond::Ls } else { Cond::Hi })
    }

    fn load_byte(&mut self, dst: machine::Slot, base: machine::Slot, index: machine::Slot) {
        self.ldrb(slot(dst), slot(base), slot(index));
    }
    fn window_before(&mut self, position: machine::Slot, distance: u32) {
        self.sub_imm(WINDOW, slot(position), distance);
        self.add_reg(WINDOW, slot(machine::SUBJECT), WINDOW);
    }
    fn load_window_byte(&mut self, dst: machine::Slot, offset: u32) {
        self.ldrb_imm(slot(dst), WINDOW, offset);
    }
    fn advance_by(&mut self, dst: machine::Slot, by: machine::Slot) {
        self.add_reg(slot(dst), slot(dst), slot(by));
    }
    fn load_word(&mut self, dst: machine::Slot, base: machine::Slot, index: machine::Slot) {
        self.ldr_reg(slot(dst), slot(base), slot(index));
    }
    fn splat(&mut self, dst: machine::Slot, byte: u8) {
        let word = u64::from(byte) * 0x0101_0101_0101_0101;
        self.movz(slot(dst), word as u16);
        for shift in [16, 32, 48] {
            self.movk(slot(dst), (word >> shift) as u16, shift);
        }
    }
    fn first_equal(
        &mut self,
        dst: machine::Slot,
        word: machine::Slot,
        splat: machine::Slot,
        scratch: machine::Slot,
    ) -> Patch {
        // A byte equals another exactly when their difference is zero, and a
        // word holds a zero byte exactly when subtracting one from each byte
        // borrows into its top bit where the byte itself has none.
        self.mov_low_bits(slot(scratch));
        self.eor(WINDOW, slot(word), slot(splat));
        self.sub_reg(slot(dst), WINDOW, slot(scratch));
        self.bic(slot(dst), slot(dst), WINDOW);
        self.ands_high_bits(slot(dst), slot(dst));
        let none = self.b_cond_forward(Cond::Eq);
        // The lowest such bit is in the first byte that matched, and this is a
        // little-endian load, so counting the bits below it gives its offset.
        self.rbit(slot(dst), slot(dst));
        self.clz(slot(dst), slot(dst));
        self.lsr(slot(dst), slot(dst), 3);
        none
    }
    fn store_position(&mut self, value: machine::Slot, index: u32) {
        self.str_index(slot(value), slot(machine::REGISTERS), index);
    }

    fn fix_room(&mut self, least: u32) -> Patch {
        self.cmp_imm(slot(machine::LENGTH), least);
        let short = self.b_cond_forward(Cond::Lo);
        self.sub_imm(LAST, slot(machine::LENGTH), least);
        short
    }
    fn start_without_room(&mut self) -> Patch {
        Assembler::cmp(self, slot(machine::FROM), LAST);
        self.b_cond_forward(Cond::Hi)
    }

    fn limit_to(&mut self, bound: machine::Slot, from: machine::Slot, offset: u32) {
        self.add_imm(WINDOW, slot(from), offset);
        Assembler::cmp(self, WINDOW, slot(bound));
        let already = self.b_cond_forward(Cond::Hs);
        self.mov(slot(bound), WINDOW);
        Assembler::bind(self, already);
    }

    fn branch_if(&mut self, cond: machine::Cond) -> Patch {
        self.b_cond_forward(condition(cond))
    }
    fn branch(&mut self) -> Patch {
        self.b_forward()
    }
    fn branch_back(&mut self, to: Label) -> Patch {
        // Every loop closes through this shape, so the budget is decremented
        // only when it is not already zero and never wraps, and between two
        // backward branches execution only moves forward. `verify` refuses a
        // backward branch in any other form.
        let out = self.cbz_forward(slot(machine::BUDGET));
        self.sub_imm(slot(machine::BUDGET), slot(machine::BUDGET), 1);
        self.b_back(to);
        out
    }
    fn bind(&mut self, patch: Patch) {
        Assembler::bind(self, patch);
    }

    fn data_address(&mut self, dst: machine::Slot) -> Patch {
        self.adr_forward(slot(dst))
    }
    fn data(&mut self, bytes: &[u8]) {
        Assembler::data(self, bytes);
    }
}

impl Assembler<'_> {
    /// Eight bytes at `[rn + rm]`, which a scan reads at once.
    pub(crate) fn ldr_reg(&mut self, rt: Reg, rn: Reg, rm: Reg) {
        self.word(0xf860_6800 | u32::from(rm.0) << 16 | u32::from(rn.0) << 5 | u32::from(rt.0));
    }
    pub(crate) fn eor(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.word(0xca00_0000 | u32::from(rm.0) << 16 | u32::from(rn.0) << 5 | u32::from(rd.0));
    }
    pub(crate) fn sub_reg(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.word(0xcb00_0000 | u32::from(rm.0) << 16 | u32::from(rn.0) << 5 | u32::from(rd.0));
    }
    /// `rd = rn & !rm`.
    pub(crate) fn bic(&mut self, rd: Reg, rn: Reg, rm: Reg) {
        self.word(0x8a20_0000 | u32::from(rm.0) << 16 | u32::from(rn.0) << 5 | u32::from(rd.0));
    }
    /// `rd = rn & 0x8080808080808080`, setting the flags. The immediate is the
    /// one repeating pattern this needs, so its encoding is written out rather
    /// than derived.
    pub(crate) fn ands_high_bits(&mut self, rd: Reg, rn: Reg) {
        self.word(0xf201_c000 | u32::from(rn.0) << 5 | u32::from(rd.0));
    }
    /// `rd = 0x0101010101010101`, likewise.
    pub(crate) fn mov_low_bits(&mut self, rd: Reg) {
        self.word(0xb200_c3e0 | u32::from(rd.0));
    }
    /// Sixteen bits of a constant, at `shift` bits up, leaving the rest.
    pub(crate) fn movk(&mut self, rd: Reg, imm: u16, shift: u32) {
        if !shift.is_multiple_of(16) || shift >= 64 {
            self.fail(EncodeError::Immediate);
            return;
        }
        self.word(0xf280_0000 | (shift / 16) << 21 | u32::from(imm) << 5 | u32::from(rd.0));
    }
    pub(crate) fn rbit(&mut self, rd: Reg, rn: Reg) {
        self.word(0xdac0_0000 | u32::from(rn.0) << 5 | u32::from(rd.0));
    }
    pub(crate) fn clz(&mut self, rd: Reg, rn: Reg) {
        self.word(0xdac0_1000 | u32::from(rn.0) << 5 | u32::from(rd.0));
    }
    /// `rd = rn >> shift`, unsigned.
    pub(crate) fn lsr(&mut self, rd: Reg, rn: Reg, shift: u32) {
        if shift >= 64 {
            self.fail(EncodeError::Immediate);
            return;
        }
        self.word(0xd340_fc00 | shift << 16 | u32::from(rn.0) << 5 | u32::from(rd.0));
    }
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
        for (word, bytes) in words.iter_mut().zip(code[..length].as_chunks::<4>().0) {
            *word = u32::from_le_bytes(*bytes);
        }
        words
    }

    #[test]
    fn encodes_what_the_assembler_encodes() {
        assert_eq!(assemble(|a| a.ret()), [0xd65f_03c0]);
        // The scan, whose words came from the assembler the same way.
        assert_eq!(assemble(|a| a.ldr_reg(Reg(7), X0, X2)), [0xf862_6807]);
        assert_eq!(assemble(|a| a.eor(Reg(15), Reg(7), X5)), [0xca05_00ef]);
        assert_eq!(
            assemble(|a| a.sub_reg(Reg(6), Reg(15), Reg(9))),
            [0xcb09_01e6]
        );
        assert_eq!(assemble(|a| a.bic(Reg(6), Reg(6), Reg(15))), [0x8a2f_00c6]);
        assert_eq!(
            assemble(|a| a.ands_high_bits(Reg(6), Reg(6))),
            [0xf201_c0c6]
        );
        assert_eq!(assemble(|a| a.mov_low_bits(Reg(9))), [0xb200_c3e9]);
        assert_eq!(assemble(|a| a.rbit(Reg(6), Reg(6))), [0xdac0_00c6]);
        assert_eq!(assemble(|a| a.clz(Reg(6), Reg(6))), [0xdac0_10c6]);
        assert_eq!(assemble(|a| a.lsr(Reg(6), Reg(6), 3)), [0xd343_fcc6]);
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
        assert_eq!(assemble(|a| a.cmp_imm(Reg(6), 0)), [0xf100_00df]);
        assert_eq!(assemble(|a| a.cmp_imm(X1, 6)), [0xf100_183f]);
        assert_eq!(assemble(|a| a.movn(X0, 1)), [0x9280_0020]);
        assert_eq!(assemble(|a| a.cmp(X2, Reg(16))), [0xeb10_005f]);
        assert_eq!(assemble(|a| a.sub_imm(Reg(16), X1, 3)), [0xd100_0c30]);
        assert_eq!(assemble(|a| a.sub_imm(X4, X4, 1)), [0xd100_0484]);
        assert_eq!(assemble(|a| a.add_reg(Reg(13), X0, Reg(8))), [0x8b08_000d]);
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
                let patch = a.cbz_forward(X4);
                a.ret();
                a.bind(patch);
            }),
            [0xb400_0044, 0xd65f_03c0]
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
