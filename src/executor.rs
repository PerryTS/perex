//! One ordered bytecode evaluator, with host-owned bounded scratch.
use crate::{
    Budget, casefold,
    input::{Cursor, Input, Mark},
    program::*,
    span::Span,
};
const UNSET: usize = usize::MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecError {
    WorkLimit,
    Registers,
    Frames,
    Undo,
    Captures,
    InvalidProgram,
}

/// Caller-owned backtracking/assertion storage. Fields contain offsets only.
#[derive(Clone, Copy, Debug, Default)]
pub struct Frame {
    pc: usize,
    position: Mark,
    undo: usize,
    assertion: usize,
    kind: u8,
    reverse: bool,
}
/// Caller-owned reversible register updates. Contains no subject/program pointer.
#[derive(Clone, Copy, Debug, Default)]
pub struct Undo {
    slot: usize,
    value: usize,
}

/// Scratch may be stack storage, an accounted arena or ordinary host buffers.
/// The evaluator never grows it, allocates, retains a borrow, or calls the host.
pub struct Scratch<'a> {
    pub registers: &'a mut [usize],
    pub frames: &'a mut [Frame],
    pub undo: &'a mut [Undo],
}
struct Vm<'a, 'p, 's, 'b> {
    program: Program<'p>,
    input: Input<'a>,
    cursor: Cursor<'a>,
    scratch: Scratch<'s>,
    budget: &'b mut Budget,
    pc: usize,
    frames: usize,
    undo: usize,
    assertion: usize,
    reverse: bool,
}
impl Vm<'_, '_, '_, '_> {
    fn charge(&mut self, n: usize) -> Result<(), ExecError> {
        self.budget.charge(n).map_err(|_| ExecError::WorkLimit)
    }
    fn store(&mut self, slot: usize, value: usize) -> Result<(), ExecError> {
        let old = *self
            .scratch
            .registers
            .get(slot)
            .ok_or(ExecError::InvalidProgram)?;
        if old == value {
            return Ok(());
        }
        if self.frames > 0 {
            *self
                .scratch
                .undo
                .get_mut(self.undo)
                .ok_or(ExecError::Undo)? = Undo { slot, value: old };
            self.undo += 1;
        }
        self.scratch.registers[slot] = value;
        Ok(())
    }
    fn rollback(&mut self, until: usize) -> Result<(), ExecError> {
        while self.undo > until {
            self.charge(1)?;
            self.undo -= 1;
            let old = self.scratch.undo[self.undo];
            self.scratch.registers[old.slot] = old.value;
        }
        Ok(())
    }
    fn push(&mut self, pc: usize, kind: u8) -> Result<(), ExecError> {
        *self
            .scratch
            .frames
            .get_mut(self.frames)
            .ok_or(ExecError::Frames)? = Frame {
            pc,
            position: self.cursor.mark(),
            undo: self.undo,
            assertion: self.assertion,
            kind,
            reverse: self.reverse,
        };
        self.frames += 1;
        Ok(())
    }
    fn fail(&mut self) -> Result<bool, ExecError> {
        while self.frames > 0 {
            self.charge(1)?;
            self.frames -= 1;
            let frame = self.scratch.frames[self.frames];
            self.rollback(frame.undo)?;
            self.cursor.restore(frame.position);
            self.reverse = frame.reverse;
            self.assertion = frame.assertion;
            if self.frames == 0 {
                self.undo = 0;
            }
            if frame.kind != 1 {
                self.pc = frame.pc;
                return Ok(true);
            }
            // Positive assertion failure propagates; negative assertion failure
            // succeeds. Work/memory errors never enter this failure path.
        }
        Ok(false)
    }
    fn read(&mut self) -> Option<u32> {
        match (self.program.unicode(), self.reverse) {
            (true, false) => self.cursor.next_point(),
            (true, true) => self.cursor.previous_point(),
            (false, false) => self.cursor.next_unit().map(u32::from),
            (false, true) => self.cursor.previous_unit().map(u32::from),
        }
    }
    fn equal(&self, left: u32, right: u32) -> bool {
        left == right
            || (self.program.words[2] & I != 0
                && casefold::equal(left, right, self.program.unicode()))
    }
    fn trial(&mut self) -> Result<bool, ExecError> {
        loop {
            self.charge(1)?;
            if self.pc >= self.program.instructions() {
                return Err(ExecError::InvalidProgram);
            }
            let [op, a, b] = self.program.instruction(self.pc);
            self.pc += 1;
            let mut success = true;
            match op {
                MATCH => {
                    if self.assertion != UNSET {
                        return Err(ExecError::InvalidProgram);
                    }
                    return Ok(true);
                }
                CHAR => success = self.read().is_some_and(|c| self.equal(c, a)),
                ANY => {
                    success = self
                        .read()
                        .is_some_and(|c| self.program.words[2] & S != 0 || !line_terminator(c))
                }
                CLASS => {
                    if let Some(c) = self.read() {
                        let values = if self.program.words[2] & I != 0 {
                            casefold::equivalents(c, self.program.unicode())
                        } else {
                            [c; 4]
                        };
                        let mut found = false;
                        for i in a..a + (b & !NEGATED) {
                            self.charge(1)?;
                            let [lo, hi] = self.program.range(i as usize);
                            if values.iter().any(|&value| value >= lo && value <= hi) {
                                found = true;
                                break;
                            }
                        }
                        success = found != (b & NEGATED != 0);
                    } else {
                        success = false;
                    }
                }
                SAVE => self.store(a as usize, self.cursor.position())?,
                SPLIT => {
                    self.push(b as usize, 0)?;
                    self.pc = a as usize;
                }
                JUMP => self.pc = a as usize,
                START => {
                    let mut previous = self.cursor;
                    success = self.cursor.position() == 0
                        || (self.program.words[2] & M != 0
                            && previous
                                .previous_unit()
                                .is_some_and(|u| line_terminator(u32::from(u))));
                }
                END => {
                    let mut next = self.cursor;
                    success = self.cursor.position() == self.input.len_utf16()
                        || (self.program.words[2] & M != 0
                            && next
                                .next_unit()
                                .is_some_and(|u| line_terminator(u32::from(u))));
                }
                WORD => {
                    let mut before = self.cursor;
                    let mut after = self.cursor;
                    let ui = self.program.words[2] & (U | I) == U | I;
                    let left = if self.program.unicode() {
                        before.previous_point()
                    } else {
                        before.previous_unit().map(u32::from)
                    };
                    let right = if self.program.unicode() {
                        after.next_point()
                    } else {
                        after.next_unit().map(u32::from)
                    };
                    success = (left.is_some_and(|c| casefold::word(c, ui))
                        != right.is_some_and(|c| casefold::word(c, ui)))
                        != (a != 0);
                }
                BACKREF => {
                    let start = self.scratch.registers[a as usize * 2];
                    let end = self.scratch.registers[a as usize * 2 + 1];
                    if start != UNSET && end != UNSET {
                        if start > end || end > self.input.len_utf16() {
                            return Err(ExecError::InvalidProgram);
                        }
                        let at = if self.reverse { end } else { start };
                        self.charge(self.input.seek_work(at))?;
                        let mut captured =
                            self.input.cursor_at(at).ok_or(ExecError::InvalidProgram)?;
                        let until = if self.reverse { start } else { end };
                        while captured.position() != until {
                            self.charge(1)?;
                            let left = match (self.program.unicode(), self.reverse) {
                                (true, false) => captured.next_point(),
                                (true, true) => captured.previous_point(),
                                (false, false) => captured.next_unit().map(u32::from),
                                (false, true) => captured.previous_unit().map(u32::from),
                            };
                            if captured.position() < start || captured.position() > end {
                                return Err(ExecError::InvalidProgram);
                            }
                            let left = left.ok_or(ExecError::InvalidProgram)?;
                            if !self.read().is_some_and(|right| self.equal(left, right)) {
                                success = false;
                                break;
                            }
                        }
                    }
                }
                ASSERT => {
                    self.push(a as usize, if b & 1 == 0 { 1 } else { 2 })?;
                    self.assertion = self.frames - 1;
                    self.reverse = b & 2 != 0;
                }
                ASSERT_END => {
                    if self.assertion == UNSET || self.assertion >= self.frames {
                        return Err(ExecError::InvalidProgram);
                    }
                    let frame = self.scratch.frames[self.assertion];
                    self.frames = self.assertion;
                    self.cursor.restore(frame.position);
                    self.reverse = frame.reverse;
                    self.assertion = frame.assertion;
                    if frame.kind == 2 {
                        self.rollback(frame.undo)?;
                        success = false;
                    } else {
                        self.pc = frame.pc;
                    }
                    if self.frames == 0 {
                        self.undo = 0;
                    }
                }
                REPEAT_INIT => {
                    let slot = self.program.capture_count() * 2 + a as usize * 2;
                    self.store(slot, 0)?;
                    self.store(slot + 1, UNSET)?;
                }
                REPEAT_CHOICE => {
                    let r = self.program.repeat(a as usize);
                    let slot = self.program.capture_count() * 2 + a as usize * 2;
                    let count = self.scratch.registers[slot];
                    if r[2] & 1 == 0 && count >= r[1] as usize {
                        self.pc = r[4] as usize;
                    } else if count >= r[0] as usize {
                        if r[2] & 2 == 0 {
                            self.push(r[4] as usize, 0)?;
                        } else {
                            self.push(r[3] as usize, 0)?;
                            self.pc = r[4] as usize;
                        }
                    }
                }
                REPEAT_BODY => {
                    let r = self.program.repeat(a as usize);
                    let slot = self.program.capture_count() * 2 + a as usize * 2;
                    for capture in r[5] as usize * 2..r[6] as usize * 2 {
                        self.charge(1)?;
                        self.store(capture, UNSET)?;
                    }
                    self.store(slot + 1, self.cursor.position())?;
                }
                REPEAT_NEXT => {
                    let r = self.program.repeat(a as usize);
                    let slot = self.program.capture_count() * 2 + a as usize * 2;
                    let count = self.scratch.registers[slot];
                    if self.cursor.position() == self.scratch.registers[slot + 1]
                        && count >= r[0] as usize
                    {
                        success = false;
                    } else {
                        self.store(slot, count.checked_add(1).ok_or(ExecError::WorkLimit)?)?;
                        self.pc = r[3] as usize - 1;
                    }
                }
                _ => return Err(ExecError::InvalidProgram),
            }
            if !success && !self.fail()? {
                return Ok(false);
            }
        }
    }
}
fn line_terminator(c: u32) -> bool {
    matches!(c, 10 | 13 | 0x2028 | 0x2029)
}

/// Find the first ordered match at/after an explicit UTF-16 position. A `y`
/// program is sticky. The host owns lastIndex coercion/update and global loops.
/// Output is untouched on no-match/error and filled only after a complete match. Work and
/// scratch exhaustion are errors, never no-match. This initial synchronous API
/// holds scoped borrows for at most the supplied work allowance; it cannot yet
/// suspend/resume at a moving safepoint during one operation.
pub fn find(
    program: Program<'_>,
    input: Input<'_>,
    start_utf16: usize,
    scratch: Scratch<'_>,
    captures: &mut [Option<Span>],
    budget: &mut Budget,
) -> Result<bool, ExecError> {
    if captures.len() < program.capture_count() {
        return Err(ExecError::Captures);
    }
    if scratch.registers.len() < program.register_count() {
        return Err(ExecError::Registers);
    }
    if start_utf16 > input.len_utf16() {
        return Ok(false);
    }
    budget
        .charge(input.seek_work(start_utf16))
        .map_err(|_| ExecError::WorkLimit)?;
    let mut start = input.cursor_at(start_utf16).unwrap();
    if program.unicode() {
        start.normalize_unicode_start();
    }
    let mut vm = Vm {
        program,
        input,
        cursor: start,
        scratch,
        budget,
        pc: 0,
        frames: 0,
        undo: 0,
        assertion: UNSET,
        reverse: false,
    };
    loop {
        vm.charge(program.register_count())?;
        vm.scratch.registers[..program.register_count()].fill(UNSET);
        vm.pc = 0;
        vm.frames = 0;
        vm.undo = 0;
        vm.assertion = UNSET;
        vm.reverse = false;
        vm.cursor = start;
        if vm.trial()? {
            // Validate every capture before exposing any output.
            vm.charge(program.capture_count() * 2)?;
            if vm.scratch.registers[0] == UNSET || vm.scratch.registers[1] == UNSET {
                return Err(ExecError::InvalidProgram);
            }
            for slots in vm.scratch.registers[..program.capture_count() * 2]
                .as_chunks::<2>()
                .0
            {
                if slots[0] != UNSET
                    && slots[1] != UNSET
                    && (slots[0] > slots[1] || slots[1] > input.len_utf16())
                {
                    return Err(ExecError::InvalidProgram);
                }
            }
            for (i, target) in captures[..program.capture_count()].iter_mut().enumerate() {
                let lo = vm.scratch.registers[2 * i];
                let hi = vm.scratch.registers[2 * i + 1];
                *target = if lo == UNSET || hi == UNSET {
                    None
                } else {
                    Span::new(lo, hi)
                };
            }
            return Ok(true);
        }
        if program.words[2] & Y != 0 {
            return Ok(false);
        }
        vm.charge(1)?;
        let next = if program.unicode() {
            start.next_point()
        } else {
            start.next_unit().map(u32::from)
        };
        if next.is_none() {
            return Ok(false);
        }
    }
}
