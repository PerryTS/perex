//! Enough of x86-64 to run what the generator emits through the x86-64
//! encoder, so that generated code can be compared with the interpreter on a
//! machine of any architecture.
//!
//! It decodes the bytes rather than the encoder's record of them, and refuses
//! anything outside the subset the generator emits: an instruction this does
//! not know is a fault, not a guess. The three memory regions are the same
//! ones the AArch64 emulator uses, so a load outside the subject or the code,
//! or a store outside the caller's registers, is a fault as well.
use super::tests::{CODE_BASE, Fault, REGISTERS_BASE, SUBJECT_BASE};

/// Registers the System V convention preserves, which generated code saves on
/// entry and must have restored before it returns.
const PRESERVED: [usize; 6] = [3, 5, 12, 13, 14, 15];

pub(super) struct Machine<'a> {
    pub r: [u64; 16],
    /// The last comparison, as its two unsigned operands, which is all the
    /// conditions this subset branches on need.
    pub compared: (u64, u64),
    pub stack: [u64; 32],
    pub sp: usize,
    pub entry: [u64; 16],
    pub subject: &'a [u8],
    pub code: &'a [u8],
    pub registers: &'a mut [u64],
}

impl Machine<'_> {
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

    fn holds(&self, cond: u8) -> Result<bool, Fault> {
        let (left, right) = self.compared;
        Ok(match cond {
            0x4 => left == right,
            0x5 => left != right,
            0x2 => left < right,
            0x3 => left >= right,
            0x7 => left > right,
            0x6 => left <= right,
            _ => return Err("a condition the emulator does not model"),
        })
    }

    fn byte(&self, at: usize) -> Result<u8, Fault> {
        self.code.get(at).copied().ok_or("ran past the code")
    }

    fn signed32(&self, at: usize) -> Result<i64, Fault> {
        let mut value = [0u8; 4];
        for (i, slot) in value.iter_mut().enumerate() {
            *slot = self.byte(at + i)?;
        }
        Ok(i64::from(i32::from_le_bytes(value)))
    }

    /// Run until `RET`, returning `RAX` and the instructions executed, or a
    /// fault after `limit` of them.
    pub(super) fn run(&mut self, limit: usize) -> Result<(i64, usize), Fault> {
        let mut pc = 0usize;
        self.entry = self.r;
        for steps in 1..=limit {
            let mut at = pc;
            let (mut wide, mut reg_high, mut index_high, mut base_high) = (false, 0, 0, 0);
            let mut byte = self.byte(at)?;
            if byte & 0xf0 == 0x40 {
                wide = byte & 8 != 0;
                reg_high = (byte >> 2) & 1;
                index_high = (byte >> 1) & 1;
                base_high = byte & 1;
                at += 1;
                byte = self.byte(at)?;
            }
            at += 1;
            match byte {
                // Save and restore, which bracket every call.
                0x50..=0x57 => {
                    let reg = (byte - 0x50) as usize | (base_high as usize) << 3;
                    self.sp = self.sp.checked_sub(1).ok_or("pushed past the stack")?;
                    self.stack[self.sp] = self.r[reg];
                }
                0x58..=0x5f => {
                    let reg = (byte - 0x58) as usize | (base_high as usize) << 3;
                    self.r[reg] = *self.stack.get(self.sp).ok_or("popped an empty stack")?;
                    self.sp += 1;
                }
                // `mov r/m, reg`, whole or low half.
                0x89 => {
                    let modrm = self.byte(at)?;
                    at += 1;
                    let source = ((modrm >> 3) & 7) as usize | (reg_high as usize) << 3;
                    let value = if wide {
                        self.r[source]
                    } else {
                        self.r[source] & 0xffff_ffff
                    };
                    if modrm >> 6 == 3 {
                        let target = (modrm & 7) as usize | (base_high as usize) << 3;
                        self.r[target] = value;
                    } else {
                        let (address, next) = self.address(modrm, at, index_high, base_high)?;
                        at = next;
                        self.store(address, value)?;
                    }
                }
                // `mov reg, r/m`: eight bytes of the subject at once.
                0x8b => {
                    let modrm = self.byte(at)?;
                    at += 1;
                    let target = ((modrm >> 3) & 7) as usize | (reg_high as usize) << 3;
                    let (address, next) = self.address(modrm, at, index_high, base_high)?;
                    at = next;
                    let mut value = 0u64;
                    for byte in 0..8 {
                        value |= u64::from(self.load(address + byte)?) << (byte * 8);
                    }
                    self.r[target] = value;
                }
                // `add`, `xor`, `sub` and `and` of a register pair, the last
                // of which sets the flags a scan branches on.
                0x01 | 0x31 | 0x29 | 0x21 => {
                    let modrm = self.byte(at)?;
                    at += 1;
                    if modrm >> 6 != 3 {
                        return Err("an arithmetic form the emulator does not decode");
                    }
                    let source = ((modrm >> 3) & 7) as usize | (reg_high as usize) << 3;
                    let target = (modrm & 7) as usize | (base_high as usize) << 3;
                    let (left, right) = (self.r[target], self.r[source]);
                    let value = match byte {
                        0x01 => left.wrapping_add(right),
                        0x31 => left ^ right,
                        0x29 => left.wrapping_sub(right),
                        _ => left & right,
                    };
                    self.r[target] = value;
                    if byte == 0x21 {
                        self.compared = (value, 0);
                    }
                }
                // `not r/m` and `shr r/m, imm8`.
                0xf7 | 0xc1 => {
                    let modrm = self.byte(at)?;
                    at += 1;
                    if modrm >> 6 != 3 {
                        return Err("a form the emulator does not decode");
                    }
                    let target = (modrm & 7) as usize | (base_high as usize) << 3;
                    match ((modrm >> 3) & 7, byte) {
                        (2, 0xf7) => self.r[target] = !self.r[target],
                        (5, 0xc1) => {
                            let shift = self.byte(at)?;
                            at += 1;
                            self.r[target] >>= u32::from(shift) & 63;
                        }
                        _ => return Err("a form the emulator does not decode"),
                    }
                }
                // A whole 64-bit constant.
                0xb8..=0xbf => {
                    let target = (byte - 0xb8) as usize | (base_high as usize) << 3;
                    let mut value = [0u8; 8];
                    for (i, slot) in value.iter_mut().enumerate() {
                        *slot = self.byte(at + i)?;
                    }
                    at += 8;
                    self.r[target] = u64::from_le_bytes(value);
                }
                // `mov r/m, imm32`, sign-extended.
                0xc7 => {
                    let modrm = self.byte(at)?;
                    at += 1;
                    if modrm >> 6 != 3 || (modrm >> 3) & 7 != 0 {
                        return Err("a `mov` form the emulator does not decode");
                    }
                    let target = (modrm & 7) as usize | (base_high as usize) << 3;
                    self.r[target] = self.signed32(at)? as u64;
                    at += 4;
                }
                // `lea reg, mem`, which computes an address without reading it.
                0x8d => {
                    let modrm = self.byte(at)?;
                    at += 1;
                    let target = ((modrm >> 3) & 7) as usize | (reg_high as usize) << 3;
                    let (address, next) = self.address(modrm, at, index_high, base_high)?;
                    // A position-relative address counts from the end of the
                    // instruction, which is only known here.
                    self.r[target] = if modrm >> 6 == 0 && modrm & 7 == 5 {
                        CODE_BASE.wrapping_add(next as u64).wrapping_add(address)
                    } else {
                        address
                    };
                    at = next;
                }
                // `cmp r/m, reg`.
                0x39 => {
                    let modrm = self.byte(at)?;
                    at += 1;
                    if modrm >> 6 != 3 {
                        return Err("a `cmp` form the emulator does not decode");
                    }
                    let right = ((modrm >> 3) & 7) as usize | (reg_high as usize) << 3;
                    let left = (modrm & 7) as usize | (base_high as usize) << 3;
                    self.compared = (self.r[left], self.r[right]);
                }
                // `test r/m, reg`, which this subset only uses against itself.
                0x85 => {
                    let modrm = self.byte(at)?;
                    at += 1;
                    let left = (modrm & 7) as usize | (base_high as usize) << 3;
                    self.compared = (self.r[left], 0);
                }
                // `cmp` and `sub` against a constant, in both widths.
                0x81 | 0x83 => {
                    let modrm = self.byte(at)?;
                    at += 1;
                    if modrm >> 6 != 3 {
                        return Err("an arithmetic form the emulator does not decode");
                    }
                    let target = (modrm & 7) as usize | (base_high as usize) << 3;
                    let value = if byte == 0x83 {
                        let immediate = self.byte(at)? as i8;
                        at += 1;
                        i64::from(immediate)
                    } else {
                        let immediate = self.signed32(at)?;
                        at += 4;
                        immediate
                    };
                    let left = if wide {
                        self.r[target]
                    } else {
                        self.r[target] & 0xffff_ffff
                    };
                    let right = if wide {
                        value as u64
                    } else {
                        value as u64 & 0xffff_ffff
                    };
                    match (modrm >> 3) & 7 {
                        7 => self.compared = (left, right),
                        5 => {
                            let result = left.wrapping_sub(right);
                            self.r[target] = if wide { result } else { result & 0xffff_ffff };
                        }
                        _ => return Err("an arithmetic operation the emulator does not decode"),
                    }
                }
                // `dec r/m`.
                0xff => {
                    let modrm = self.byte(at)?;
                    at += 1;
                    if modrm >> 6 != 3 || (modrm >> 3) & 7 != 1 {
                        return Err("an increment form the emulator does not decode");
                    }
                    let target = (modrm & 7) as usize | (base_high as usize) << 3;
                    self.r[target] = self.r[target].wrapping_sub(1);
                }
                0x0f => {
                    let second = self.byte(at)?;
                    at += 1;
                    match second {
                        // `bsf reg, r/m`: the lowest bit a scan matched.
                        0xbc => {
                            let modrm = self.byte(at)?;
                            at += 1;
                            if modrm >> 6 != 3 {
                                return Err("a `bsf` form the emulator does not decode");
                            }
                            let target = ((modrm >> 3) & 7) as usize | (reg_high as usize) << 3;
                            let source = (modrm & 7) as usize | (base_high as usize) << 3;
                            let value = self.r[source];
                            if value != 0 {
                                self.r[target] = u64::from(value.trailing_zeros());
                            }
                        }
                        // `movzx reg, byte r/m`.
                        0xb6 => {
                            let modrm = self.byte(at)?;
                            at += 1;
                            let target = ((modrm >> 3) & 7) as usize | (reg_high as usize) << 3;
                            let (address, next) = self.address(modrm, at, index_high, base_high)?;
                            at = next;
                            self.r[target] = u64::from(self.load(address)?);
                        }
                        // `jcc rel32`.
                        0x80..=0x8f => {
                            let offset = self.signed32(at)?;
                            at += 4;
                            if self.holds(second & 0xf)? {
                                at = (at as i64 + offset) as usize;
                            }
                        }
                        _ => return Err("a two-byte instruction the emulator does not decode"),
                    }
                }
                // `jmp rel32`.
                0xe9 => {
                    let offset = self.signed32(at)?;
                    at += 4;
                    at = (at as i64 + offset) as usize;
                }
                0xc3 => {
                    for reg in PRESERVED {
                        if self.r[reg] != self.entry[reg] {
                            return Err("wrote a register the calling convention preserves");
                        }
                    }
                    if self.sp != self.stack.len() {
                        return Err("returned without restoring the stack");
                    }
                    return Ok((self.r[0] as i64, steps));
                }
                _ => return Err("an instruction the emulator does not decode"),
            }
            pc = at;
        }
        Err("ran past its bound")
    }

    /// The address a `ModRM` byte and what follows it name, and where the next
    /// instruction begins.
    fn address(
        &self,
        modrm: u8,
        at: usize,
        index_high: u8,
        base_high: u8,
    ) -> Result<(u64, usize), Fault> {
        let mode = modrm >> 6;
        let rm = modrm & 7;
        let mut at = at;
        // A position-relative operand, whose base the caller adds.
        if mode == 0 && rm == 5 {
            let offset = self.signed32(at)?;
            return Ok((offset as u64, at + 4));
        }
        let (base, index) = if rm == 4 {
            let sib = self.byte(at)?;
            at += 1;
            if sib >> 6 != 0 {
                return Err("a scaled index the emulator does not decode");
            }
            let index = ((sib >> 3) & 7) as usize | (index_high as usize) << 3;
            let base = (sib & 7) as usize | (base_high as usize) << 3;
            // Index four with no high bit names no register at all.
            (base, (index != 4).then_some(index))
        } else {
            ((rm as usize) | (base_high as usize) << 3, None)
        };
        let displacement = match mode {
            0 => 0,
            1 => {
                let value = self.byte(at)? as i8;
                at += 1;
                i64::from(value)
            }
            2 => {
                let value = self.signed32(at)?;
                at += 4;
                value
            }
            _ => return Err("a register operand where memory was expected"),
        };
        let address = self.r[base]
            .wrapping_add(index.map_or(0, |index| self.r[index]))
            .wrapping_add(displacement as u64);
        Ok((address, at))
    }
}
