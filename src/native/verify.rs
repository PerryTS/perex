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
//! What it assumes is the host's half of the call: `X0` addresses `X1` readable
//! bytes, and `X1` is a slice length and so at most `isize::MAX`; `X3` addresses
//! `register_count` writable 64-bit slots; the code is mapped as exactly the
//! bytes verified; and it is entered at its first byte with a return address in
//! the link register. `X2` and `X4` may hold anything.
//!
//! The analysis is abstract interpretation over caller-owned [`Facts`], one per
//! instruction, so it allocates nothing and its work is bounded by the code's
//! length.

use core::cmp::{max, min};

/// Registers the analysis follows: `X0` through `X17`, which are the ones a
/// leaf function may overwrite. Writing any other is refused.
const TRACKED: usize = 18;
/// The budget register.
const BUDGET: u8 = 4;
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct State {
    x: [Value; TRACKED],
    flags: Flags,
    /// A lower bound on the subject's length.
    length_low: u32,
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
}

/// The instructions accepted, decoded. Register 31, which is the stack pointer
/// or the zero register depending on the instruction, is refused as every
/// operand.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Insn {
    Ret,
    Movz { rd: u8, imm: u16 },
    Movn { rd: u8, imm: u16 },
    Mov { rd: u8, rm: u8 },
    AddImm { rd: u8, rn: u8, imm: u32 },
    SubImm { rd: u8, rn: u8, imm: u32 },
    SubImm32 { rd: u8, rn: u8, imm: u32 },
    AddReg { rd: u8, rn: u8, rm: u8 },
    CmpImm { rn: u8, imm: u32 },
    CmpImm32 { rn: u8, imm: u32 },
    CmpReg { rn: u8, rm: u8 },
    LdrbReg { rt: u8, rn: u8, rm: u8 },
    LdrbImm { rt: u8, rn: u8, imm: u32 },
    StrIndex { rt: u8, rn: u8, index: u32 },
    Adr { rd: u8, offset: i32 },
    BCond { cond: u8, offset: i32 },
    B { offset: i32 },
    Cbz { rt: u8, offset: i32 },
}

fn sign_extend(value: u32, bits: u32) -> i32 {
    ((value << (32 - bits)) as i32) >> (32 - bits)
}

/// Decode one word, exactly: every bit outside an instruction's operand fields
/// must be the value this accepts, so a word that would execute as something
/// else is not decoded as this.
fn decode(w: u32) -> Option<Insn> {
    let rd = (w & 31) as u8;
    let rn = ((w >> 5) & 31) as u8;
    let rm = ((w >> 16) & 31) as u8;
    let imm12 = (w >> 10) & 0xfff;
    let insn = if w == 0xd65f_03c0 {
        Insn::Ret
    } else if w & 0xffe0_0000 == 0xd280_0000 {
        Insn::Movz {
            rd,
            imm: (w >> 5) as u16,
        }
    } else if w & 0xffe0_0000 == 0x9280_0000 {
        Insn::Movn {
            rd,
            imm: (w >> 5) as u16,
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
        Insn::LdrbReg { rt: rd, rn, rm }
    } else if w & 0xffc0_0000 == 0x3940_0000 {
        Insn::LdrbImm {
            rt: rd,
            rn,
            imm: imm12,
        }
    } else if w & 0xffc0_0000 == 0xf900_0000 {
        Insn::StrIndex {
            rt: rd,
            rn,
            index: imm12,
        }
    } else if w & 0x9f00_0000 == 0x1000_0000 {
        let value = (((w >> 5) & 0x7_ffff) << 2) | ((w >> 29) & 3);
        Insn::Adr {
            rd,
            offset: sign_extend(value, 21),
        }
    } else if w & 0xff00_0010 == 0x5400_0000 {
        let cond = (w & 15) as u8;
        if !matches!(cond, 0 | 1 | 2 | 3 | 8 | 9) {
            return None;
        }
        Insn::BCond {
            cond,
            offset: sign_extend((w >> 5) & 0x7_ffff, 19),
        }
    } else if w & 0xfc00_0000 == 0x1400_0000 {
        Insn::B {
            offset: sign_extend(w & 0x3ff_ffff, 26),
        }
    } else if w & 0xff00_0000 == 0xb400_0000 {
        Insn::Cbz {
            rt: rd,
            offset: sign_extend((w >> 5) & 0x7_ffff, 19),
        }
    } else {
        return None;
    };
    let registers: [u8; 3] = match insn {
        Insn::Ret | Insn::BCond { .. } | Insn::B { .. } => [0; 3],
        Insn::Movz { rd, .. } | Insn::Movn { rd, .. } | Insn::Adr { rd, .. } => [rd, 0, 0],
        Insn::Cbz { rt, .. } => [rt, 0, 0],
        Insn::Mov { rd, rm } => [rd, rm, 0],
        Insn::AddImm { rd, rn, .. } | Insn::SubImm { rd, rn, .. } => [rd, rn, 0],
        Insn::SubImm32 { rd, rn, .. } => [rd, rn, 0],
        Insn::CmpImm { rn, .. } | Insn::CmpImm32 { rn, .. } => [rn, 0, 0],
        Insn::CmpReg { rn, rm } => [rn, rm, 0],
        Insn::AddReg { rd, rn, rm } => [rd, rn, rm],
        Insn::LdrbReg { rt, rn, rm } => [rt, rn, rm],
        Insn::LdrbImm { rt, rn, .. } | Insn::StrIndex { rt, rn, .. } => [rt, rn, 0],
    };
    (!registers.contains(&31)).then_some(insn)
}

impl Insn {
    /// The register this writes, if any.
    fn written(self) -> Option<u8> {
        match self {
            Insn::Movz { rd, .. }
            | Insn::Movn { rd, .. }
            | Insn::Mov { rd, .. }
            | Insn::AddImm { rd, .. }
            | Insn::SubImm { rd, .. }
            | Insn::SubImm32 { rd, .. }
            | Insn::AddReg { rd, .. }
            | Insn::Adr { rd, .. } => Some(rd),
            Insn::LdrbReg { rt, .. } | Insn::LdrbImm { rt, .. } => Some(rt),
            _ => None,
        }
    }

    /// Where a branch goes, as an instruction index, if it is inside the code.
    /// `Err` for a branch that leaves it; `Ok(None)` for anything else.
    fn target(self, index: usize, count: usize) -> Result<Option<usize>, ()> {
        let offset = match self {
            Insn::BCond { offset, .. } | Insn::B { offset } | Insn::Cbz { offset, .. } => offset,
            _ => return Ok(None),
        };
        let target = index as i64 + i64::from(offset);
        if (0..count as i64).contains(&target) {
            Ok(Some(target as usize))
        } else {
            Err(())
        }
    }
}

fn word(code: &[u8], index: usize) -> u32 {
    let at = index * 4;
    u32::from_le_bytes([code[at], code[at + 1], code[at + 2], code[at + 3]])
}

fn narrow(value: i64) -> Option<i32> {
    i32::try_from(value).ok()
}

impl State {
    fn entry() -> Self {
        let mut state = State::default();
        state.x[0] = Value::Subject;
        state.x[1] = Value::Length;
        state.x[3] = Value::Registers;
        state
    }

    fn get(&self, r: u8) -> Value {
        self.x.get(r as usize).copied().unwrap_or(Value::Top)
    }

    fn set(&mut self, r: u8, value: Value) {
        if let Some(slot) = self.x.get_mut(r as usize) {
            *slot = value;
        }
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
    fn refine(&mut self, cond: u8) {
        match self.flags {
            Flags::Registers(a, b) => match cond {
                0 => {
                    self.below(a, b, 0);
                    self.below(b, a, 0);
                }
                2 => self.below(b, a, 0),
                8 => self.below(b, a, 1),
                3 => self.below(a, b, 1),
                9 => self.below(a, b, 0),
                _ => {}
            },
            Flags::Immediate(a, imm) => match cond {
                0 | 2 => self.at_least(a, u64::from(imm)),
                8 => self.at_least(a, u64::from(imm) + 1),
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

/// Whether one byte at this address is inside the subject or the code.
fn readable(value: Value, code_length: usize) -> bool {
    match value {
        Value::SubjectAt { slack, .. } => slack >= 1,
        Value::Code(at) => (at as usize) < code_length,
        Value::CodeSpan { at, width } => at as usize + width as usize <= code_length,
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
fn transfer(insn: Insn, index: usize, state: &State) -> (State, State) {
    let mut next = *state;
    let mut taken = *state;
    match insn {
        Insn::Ret | Insn::StrIndex { .. } | Insn::B { .. } | Insn::Cbz { .. } => {}
        Insn::Movz { rd, imm } => next.set(rd, Value::Small(u32::from(imm))),
        Insn::Movn { rd, imm } => next.set(rd, Value::Negative(imm)),
        Insn::Mov { rd, rm } => next.set(rd, state.get(rm)),
        Insn::AddImm { rd, rn, imm } => next.set(rd, add_const(state.get(rn), u64::from(imm))),
        Insn::SubImm { rd, rn, imm } => {
            next.set(rd, sub_const(state, state.get(rn), u64::from(imm)))
        }
        Insn::SubImm32 { rd, .. } => next.set(rd, Value::Fits32),
        Insn::AddReg { rd, rn, rm } => next.set(rd, add_values(state.get(rn), state.get(rm))),
        Insn::CmpImm { rn, imm } => next.flags = Flags::Immediate(rn, imm as u16),
        Insn::CmpImm32 { .. } => next.flags = Flags::Unknown,
        Insn::CmpReg { rn, rm } => next.flags = Flags::Registers(rn, rm),
        Insn::LdrbReg { rt, .. } | Insn::LdrbImm { rt, .. } => next.set(rt, Value::Byte),
        Insn::Adr { rd, offset } => {
            let at = (index * 4) as i64 + i64::from(offset);
            let value = u32::try_from(at).map_or(Value::Top, Value::Code);
            next.set(rd, value);
        }
        Insn::BCond { cond, .. } => {
            taken.refine(cond);
            // Every condition accepted has its inverse one bit away.
            next.refine(cond ^ 1);
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
    index: usize,
    state: &State,
    code_length: usize,
    register_count: usize,
) -> Result<(), Reason> {
    let count = code_length / 4;
    let require = |holds: bool, reason| if holds { Ok(()) } else { Err(reason) };
    if let Some(rd) = insn.written() {
        require((rd as usize) < TRACKED, Reason::Written)?;
    }
    require(insn.target(index, count).is_ok(), Reason::Branch)?;
    let terminal = matches!(insn, Insn::Ret | Insn::B { .. });
    require(terminal || index + 1 < count, Reason::FallsOffEnd)?;
    match insn {
        Insn::Ret => {
            let answer = state.get(0);
            require(
                is_position(answer) || matches!(answer, Value::Negative(0 | 1)),
                Reason::Return,
            )
        }
        Insn::LdrbReg { rn, rm, .. } => require(
            readable(add_values(state.get(rn), state.get(rm)), code_length),
            Reason::Load,
        ),
        Insn::LdrbImm { rn, imm, .. } => require(
            readable(add_const(state.get(rn), u64::from(imm)), code_length),
            Reason::Load,
        ),
        Insn::StrIndex { rt, rn, index } => {
            require(
                state.get(rn) == Value::Registers && (index as usize) < register_count,
                Reason::Store,
            )?;
            require(is_position(state.get(rt)), Reason::Stored)
        }
        Insn::CmpImm32 { rn, .. } => require(fits32(state.get(rn)), Reason::Compare),
        _ => Ok(()),
    }
}

/// Check `code` as described in the module documentation, using one of
/// `facts` per four bytes of it.
pub fn verify(code: &[u8], register_count: usize, facts: &mut [Facts]) -> Result<(), VerifyError> {
    let count = code.len() / 4;
    let fail = |index: usize, reason| {
        Err(VerifyError {
            at: index * 4,
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
    merge(facts, 0, &State::entry());

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
            let Some(insn) = decode(word(code, index)) else {
                continue;
            };
            let state = facts[index].state;
            let (next, taken) = transfer(insn, index, &state);
            if !matches!(insn, Insn::Ret | Insn::B { .. }) && index + 1 < count {
                merge(facts, index + 1, &next);
            }
            if let Ok(Some(target)) = insn.target(index, count) {
                merge(facts, target, &taken);
            }
        }
    }

    // What every reached instruction must satisfy, against everything that can
    // reach it.
    for (index, fact) in facts.iter().enumerate() {
        if fact.reached {
            let insn = decode(word(code, index)).ok_or(Reason::Undecodable);
            insn.and_then(|insn| obligations(insn, index, &fact.state, code.len(), register_count))
                .or_else(|reason| fail(index, reason))?;
        }
    }

    // The budget. A backward branch must be the last of
    //     cbz x4, <forward>; sub x4, x4, #1; b <back>
    // with nothing branching to its second or third instruction, and nothing
    // else may write x4. Then x4 falls by exactly one before each backward
    // branch and never wraps, and between two backward branches execution only
    // moves forward, which is the bound in the module documentation.
    for index in 0..count {
        if !facts[index].reached {
            continue;
        }
        let insn = decode(word(code, index)).expect("decoded above");
        let Ok(Some(target)) = insn.target(index, count) else {
            continue;
        };
        if target > index {
            continue;
        }
        let guarded = matches!(insn, Insn::B { .. })
            && index >= 2
            && decode(word(code, index - 1))
                == Some(Insn::SubImm {
                    rd: BUDGET,
                    rn: BUDGET,
                    imm: 1,
                })
            && matches!(
                decode(word(code, index - 2)),
                Some(Insn::Cbz { rt: BUDGET, .. })
            );
        if !guarded {
            return fail(index, Reason::Budget);
        }
        facts[index - 1].guard = true;
        facts[index].guard = true;
    }
    for index in 0..count {
        if !facts[index].reached {
            continue;
        }
        let insn = decode(word(code, index)).expect("decoded above");
        if let Ok(Some(target)) = insn.target(index, count)
            && facts[target].guard
        {
            return fail(index, Reason::Budget);
        }
        if insn.written() == Some(BUDGET) && !facts[index].guard {
            return fail(index, Reason::Budget);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::a64::{Assembler, Cond, Reg, X0, X1, X2, X3, X4, X5};

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
        verify(&code[..length], 2, &mut facts).map_err(|error| error.reason)
    }

    fn raw(asm: &mut Assembler<'_>, word: u32) {
        asm.data(&word.to_le_bytes());
    }

    #[test]
    fn decodes_what_the_assembler_encodes() {
        // The same words the encoder's tests take from the system assembler.
        assert_eq!(decode(0xd65f_03c0), Some(Insn::Ret));
        assert_eq!(
            decode(0x3862_6824),
            Some(Insn::LdrbReg {
                rt: 4,
                rn: 1,
                rm: 2
            })
        );
        assert_eq!(decode(0x7101_849f), Some(Insn::CmpImm32 { rn: 4, imm: 97 }));
        assert_eq!(decode(0xf100_183f), Some(Insn::CmpImm { rn: 1, imm: 6 }));
        assert_eq!(decode(0xeb10_005f), Some(Insn::CmpReg { rn: 2, rm: 16 }));
        assert_eq!(
            decode(0x9100_2020),
            Some(Insn::AddImm {
                rd: 0,
                rn: 1,
                imm: 8
            })
        );
        assert_eq!(
            decode(0xd100_0484),
            Some(Insn::SubImm {
                rd: 4,
                rn: 4,
                imm: 1
            })
        );
        assert_eq!(
            decode(0x5101_84e7),
            Some(Insn::SubImm32 {
                rd: 7,
                rn: 7,
                imm: 97
            })
        );
        assert_eq!(
            decode(0xf900_0862),
            Some(Insn::StrIndex {
                rt: 2,
                rn: 3,
                index: 2
            })
        );
        assert_eq!(decode(0xaa08_03e7), Some(Insn::Mov { rd: 7, rm: 8 }));
        assert_eq!(
            decode(0x3940_1ce7),
            Some(Insn::LdrbImm {
                rt: 7,
                rn: 7,
                imm: 7
            })
        );
        assert_eq!(
            decode(0x8b08_000d),
            Some(Insn::AddReg {
                rd: 13,
                rn: 0,
                rm: 8
            })
        );
        assert_eq!(decode(0x9280_0020), Some(Insn::Movn { rd: 0, imm: 1 }));
        assert_eq!(decode(0xd29f_ffe3), Some(Insn::Movz { rd: 3, imm: 65535 }));
        assert_eq!(decode(0x1000_0089), Some(Insn::Adr { rd: 9, offset: 16 }));
        assert_eq!(
            decode(0x54ff_ffe1),
            Some(Insn::BCond {
                cond: 1,
                offset: -1
            })
        );
        assert_eq!(decode(0x17ff_ffff), Some(Insn::B { offset: -1 }));
        assert_eq!(decode(0xb4ff_ffe4), Some(Insn::Cbz { rt: 4, offset: -1 }));
        assert_eq!(
            decode(0x54ff_ffc2),
            Some(Insn::BCond {
                cond: 2,
                offset: -2
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
            assert_eq!(decode(word), None, "{word:08x}");
        }
    }

    #[test]
    fn accepts_a_load_after_the_comparison_that_bounds_it() {
        assert_eq!(
            check(|a| {
                a.cmp(X2, X1);
                let none = a.b_cond_forward(Cond::Hs);
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
                let none = a.b_cond_forward(Cond::Hi);
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
                let load = a.b_cond_forward(Cond::Hs);
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
                let none = a.b_cond_forward(Cond::Hi);
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
                let none = a.b_cond_forward(Cond::Lo);
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
                let none = a.b_cond_forward(Cond::Hs);
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
                    none = Some(a.b_cond_forward(Cond::Hi));
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
                let none = a.b_cond_forward(Cond::Hi);
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
                a.b_cond_back(Cond::Ne, top);
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
                let skip = a.b_cond_forward(Cond::Eq);
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
            verify(&code[..length], 0, &mut facts).map_err(|error| error.reason),
            Err(Reason::Scratch)
        );
    }
}
