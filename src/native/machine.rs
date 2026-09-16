//! What the code generator asks of an instruction encoder.
//!
//! The generator in [`super::emit`] selects instructions once, for every
//! target. What differs between targets is the encoding and the register file,
//! so both live behind this trait: the generator names abstract [`Slot`]s and
//! operations, and a backend maps them to its own registers and bytes.
//!
//! The operations are the ones the generated search needs and no more. Where
//! two targets reach a result differently — a range test, a byte read at a
//! distance before a position, the last start with room for a match — the
//! operation says what is wanted rather than how, so that neither backend has
//! to spend a register emulating the other's instruction set.
#![allow(dead_code)]

/// A value the generated code keeps while a search runs. A backend maps each
/// to one of its registers, or to storage of its own when it has fewer
/// registers than this file has slots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Slot(pub u8);

/// The subject's first byte, its length, the start being tried, the caller's
/// register array, and the backward branches left: the five arguments.
pub(crate) const SUBJECT: Slot = Slot(0);
pub(crate) const LENGTH: Slot = Slot(1);
pub(crate) const FROM: Slot = Slot(2);
pub(crate) const REGISTERS: Slot = Slot(3);
pub(crate) const BUDGET: Slot = Slot(4);
/// The membership table a repeated multi-range class is tested through, held
/// across that repeat's scan.
pub(crate) const TABLE: Slot = Slot(5);
/// The position inside the current attempt.
pub(crate) const AT: Slot = Slot(6);
/// The byte just read from the subject.
pub(crate) const BYTE: Slot = Slot(7);
/// Scratch within one step of the generated code.
pub(crate) const TMP: Slot = Slot(8);
/// Open repeats keep two slots each: where the run currently ends, and the
/// earliest end its minimum allows.
///
/// Two is what both targets hold in registers. x86-64 has fifteen usable
/// registers, two of which an encoder keeps for itself, and this file is the
/// thirteen that leaves; a third repeat would have to live in memory, in the
/// generated code and in what verifies it, to admit patterns of three
/// sequential open repeats. Those fall back to the interpreter instead.
pub(crate) const MAX_REPEATS: usize = 2;

pub(crate) fn repeat_end(depth: usize) -> Slot {
    Slot(9 + depth as u8 * 2)
}
pub(crate) fn repeat_floor(depth: usize) -> Slot {
    Slot(10 + depth as u8 * 2)
}
/// Slots a backend has to place, which is every one of the above.
pub(crate) const SLOTS: usize = 9 + MAX_REPEATS * 2;

/// A comparison's outcome, as the generator means it rather than as any target
/// spells it. Every comparison the generated code makes is unsigned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Cond {
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

/// A position in the emitted code that a branch can be pointed at.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Label(pub usize);

/// What an unbound reference will become once its target is known. A backend
/// reads this to know which field of which instruction to fill in.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Kind {
    #[default]
    CondBranch,
    Branch,
    /// A position-relative address rather than a jump.
    Address,
}

/// A reference that has been emitted but whose target is not known yet. The
/// default is one that was never emitted, which `bind` ignores, so a generator
/// can carry a fixed array of them.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Patch {
    pub at: usize,
    pub kind: Kind,
    pub live: bool,
}

/// Why an encoding could not be produced. Every one of these is a bug in the
/// code generator rather than anything a pattern can cause, but an encoder
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
    /// The target has no register or storage for a slot the generator used.
    Slots,
}

/// The instructions a generated search is built from.
///
/// Positions and lengths are whole registers; a byte read from the subject is
/// compared as a 32-bit value, which is exact because it was zero-extended
/// from eight bits. Every comparison is unsigned, and every branch is to a
/// [`Label`] behind or a [`Patch`] ahead.
pub(crate) trait Machine {
    /// Where the next instruction will go, as a branch target.
    fn here(&self) -> Label;
    /// Bytes emitted so far.
    fn position(&self) -> usize;
    /// The bytes written, or the first error that stopped them being written.
    fn done(&mut self) -> Result<usize, EncodeError>;

    /// Save whatever the calling convention expects a call to preserve. Emitted
    /// once, before anything else.
    fn enter(&mut self);

    /// The code a call may return: a start position, or one of the two codes.
    /// Each restores what `enter` saved.
    fn return_start(&mut self);
    fn return_code(&mut self, code: i32);

    /// `dst = src`.
    fn copy(&mut self, dst: Slot, src: Slot);
    /// `dst = src + offset`, where both are positions.
    fn ahead(&mut self, dst: Slot, src: Slot, offset: u32);
    /// `dst = src - offset`, where both are positions.
    fn behind(&mut self, dst: Slot, src: Slot, offset: u32);

    /// Compare two positions, for a branch that follows.
    fn compare(&mut self, left: Slot, right: Slot);
    /// Compare a position with a constant.
    fn compare_position(&mut self, left: Slot, right: u32);
    /// Compare a byte read from the subject with a constant.
    fn compare_byte(&mut self, left: Slot, right: u32);
    /// Branch when a byte is inside `lo..=hi`, or when `inside` is false, when
    /// it is outside.
    fn byte_within(&mut self, byte: Slot, lo: u32, hi: u32, inside: bool) -> Patch;

    /// `dst = subject[index]`, zero-extended.
    fn load_byte(&mut self, dst: Slot, base: Slot, index: Slot);
    /// Establish the window of `distance` bytes ending at `position`, which
    /// the caller has already checked the subject holds, so that its bytes can
    /// be read one at a time.
    fn window_before(&mut self, position: Slot, distance: u32);
    /// `dst` is byte `offset` of that window, counting from its earliest,
    /// zero-extended.
    fn load_window_byte(&mut self, dst: Slot, offset: u32);
    /// `registers[index] = value`, where `value` is a position.
    fn store_position(&mut self, value: Slot, index: u32);

    /// Fix the last start that has room for a match of `least` bytes, which is
    /// the whole search's, and branch when the subject is shorter than that.
    fn fix_room(&mut self, least: u32) -> Patch;
    /// Branch when the start being tried is past that last start, which is
    /// also what makes a start past the subject's end no match.
    fn start_without_room(&mut self) -> Patch;

    /// `bound = min(bound, from + offset)`, both positions.
    fn limit_to(&mut self, bound: Slot, from: Slot, offset: u32);

    /// Branch on the last comparison.
    fn branch_if(&mut self, cond: Cond) -> Patch;
    /// Branch unconditionally, forwards.
    fn branch(&mut self) -> Patch;
    /// Branch backwards, spending one unit of budget, and branch out when
    /// there is none left. This is the only backward branch a backend may
    /// emit, and the shape of it is what bounds how long a call runs.
    fn branch_back(&mut self, to: Label) -> Patch;
    /// Point an emitted reference at where the next instruction will go.
    fn bind(&mut self, patch: Patch);

    /// Take the address of data that follows the code, for a table read.
    fn data_address(&mut self, dst: Slot) -> Patch;
    /// Write bytes after the code that reads them.
    fn data(&mut self, bytes: &[u8]);
}
