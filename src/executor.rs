//! One ordered evaluator with caller-owned scratch and resumable offset state.
use crate::{
    Budget, casefold,
    input::{Cursor, Input, Mark},
    program::*,
    properties,
    span::Span,
};
mod admission;
mod state;
use state::*;
const UNSET: usize = usize::MAX;

fn equal(program: Program<'_>, left: u32, right: u32) -> bool {
    left == right || (program.words[2] & I != 0 && casefold::equal(left, right, program.unicode()))
}
fn line_terminator(c: u32) -> bool {
    matches!(c, 10 | 13 | 0x2028 | 0x2029)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecError {
    WorkLimit,
    Registers,
    Frames,
    Undo,
    Captures,
    InvalidProgram,
    ChangedResources,
    Cancelled,
    NotMatched,
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

/// Scratch is exclusively borrowed by an active or paused search. It never
/// grows implicitly and must not contain untraced host object references.
pub struct Scratch<'a> {
    pub registers: &'a mut [usize],
    pub frames: &'a mut [Frame],
    pub undo: &'a mut [Undo],
}

/// Owner of initialized scratch buffers. Views must preserve all live entries
/// between calls. Acquiring a view must not allocate, collect, call the host or
/// change live contents. Allocate replacement owners between advances and use
/// `Search::rebuffer` to transfer state. The engine owns this value exclusively.
pub trait ScratchOwner {
    fn scratch(&mut self) -> Scratch<'_>;
}
impl ScratchOwner for Scratch<'_> {
    fn scratch(&mut self) -> Scratch<'_> {
        Scratch {
            registers: self.registers,
            frames: self.frames,
            undo: self.undo,
        }
    }
}

/// A rooted, immutable program/subject pair whose backing allocations may move.
///
/// Every callback must observe the SAME program words and subject representation
/// for this resource object's entire search lifetime. Relocation may change
/// addresses, never contents or encoding. Implementors own roots, sharing rules
/// and borrow guards. Views must be released before collection, callbacks or
/// allocation that can move them. Length checks cannot prove immutable identity.
/// Violating this semantic contract may yield incorrect answers or panic; it
/// never authorizes unsafe code in the evaluator.
///
/// The callback cannot return a view borrowed from its arguments. This keeps
/// subject/program borrows inside one advance. Existing safe constructors still
/// validate/count when acquiring views; avoiding repeated validation in a GC
/// adapter requires a separately established owner invariant.
/// An acquisition error must occur before invoking the callback. After invoking
/// it, return its result successfully so retrying cannot replay completed work.
///
/// ```compile_fail
/// use perex::{executor::Resources, input::Input};
/// fn escape<R: Resources>(owner: &R) -> Input<'_> {
///     owner.with_views(|_, input| input).ok().unwrap()
/// }
/// ```
pub trait Resources {
    type Error;
    fn with_views<T>(
        &self,
        use_views: impl FnOnce(Program<'_>, Input<'_>) -> T,
    ) -> Result<T, Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Progress {
    Pending,
    Matched,
    NoMatch,
}
#[derive(Debug)]
pub enum SearchError<E> {
    Resource(E),
    Execution(ExecError),
}

/// Minimum capacities that preserve live state and satisfy the current request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScratchRequirements {
    pub registers: usize,
    pub frames: usize,
    pub undo: usize,
}

/// Failed rebinding returns the unchanged operation and all supplied buffers.
pub struct RebufferError<S, B> {
    pub error: ExecError,
    pub search: S,
    pub buffers: B,
}

/// One operation retaining roots, offset-only state and exclusive scratch.
/// No subject/program view survives `advance`. The operation-wide budget is
/// never replenished by a pause. Frame/undo requests retain the pending update;
/// caller-owned replacement buffers can resume it through `rebuffer`.
pub struct Search<'r, R: Resources, B: ScratchOwner> {
    resources: &'r R,
    buffers: B,
    shape: Shape,
    state: State,
    budget: Budget,
}
impl<'r, R: Resources, B: ScratchOwner> Search<'r, R, B> {
    pub fn new(
        resources: &'r R,
        start_utf16: usize,
        mut buffers: B,
        budget: Budget,
    ) -> Result<Self, SearchError<R::Error>> {
        let shape = resources
            .with_views(Shape::new)
            .map_err(SearchError::Resource)?;
        if buffers.scratch().registers.len() < shape.registers() {
            return Err(SearchError::Execution(ExecError::Registers));
        }
        Ok(Self {
            resources,
            buffers,
            shape,
            state: State::new(start_utf16, shape.input.2),
            budget,
        })
    }

    pub fn remaining_work(&self) -> usize {
        self.budget.remaining()
    }

    /// Run toward a requested work quantum, then release every view. Zero
    /// performs no work. Atomic bounded steps may exceed the quantum: at most
    /// one 256-byte admission chunk plus 32 comparisons per candidate (8,448
    /// work units). Long seeks, register operations, classes and backreferences
    /// are incremental. View acquisition/validation belongs to `Resources` and
    /// is outside this quantum; it is not yet an efficient host reborrow API.
    pub fn advance(&mut self, quantum: usize) -> Result<Progress, SearchError<R::Error>> {
        if let Some(error) = self.state.blocked {
            return Err(SearchError::Execution(error));
        }
        if let Some(outcome) = self.state.phase.outcome() {
            return outcome.map_err(SearchError::Execution);
        }
        if quantum == 0 {
            return Ok(Progress::Pending);
        }
        let result = self
            .resources
            .with_views(|program, input| {
                if Shape::new(program, input) != self.shape {
                    return Err(ExecError::ChangedResources);
                }
                let cursor = input
                    .resume_cursor(self.state.current)
                    .ok_or(ExecError::ChangedResources)?;
                let mut scratch = self.buffers.scratch();
                if scratch.registers.len() < self.shape.registers()
                    || scratch.frames.len() < self.state.frames
                    || scratch.undo.len() < self.state.undo
                {
                    return Err(ExecError::ChangedResources);
                }
                let mut vm = Vm {
                    program,
                    input,
                    cursor,
                    scratch: &mut scratch,
                    state: self.state,
                    budget: self.budget,
                };
                let result = vm.run(quantum);
                vm.state.current = vm.cursor.mark();
                self.state = vm.state;
                self.budget = vm.budget;
                result
            })
            .map_err(SearchError::Resource)?;
        if let Err(error) = result {
            if matches!(error, ExecError::Frames | ExecError::Undo) {
                self.state.blocked = Some(error);
            } else {
                self.state.phase = Phase::Failed(error);
            }
        }
        result.map_err(SearchError::Execution)
    }

    pub fn cancel(&mut self) {
        if self.state.phase.outcome().is_none() {
            self.state.blocked = None;
            self.state.phase = Phase::Failed(ExecError::Cancelled);
        }
    }

    pub fn capture_count(&self) -> usize {
        self.shape.header[3] as usize
    }

    /// Read one validated capture without allocating or borrowing a subject.
    pub fn capture(&mut self, index: usize) -> Result<Option<Span>, ExecError> {
        self.require_match()?;
        if index >= self.capture_count() {
            return Err(ExecError::Captures);
        }
        let scratch = self.buffers.scratch();
        let lo = *scratch
            .registers
            .get(index * 2)
            .ok_or(ExecError::ChangedResources)?;
        let hi = *scratch
            .registers
            .get(index * 2 + 1)
            .ok_or(ExecError::ChangedResources)?;
        Ok(if lo == UNSET || hi == UNSET {
            None
        } else {
            Span::new(lo, hi)
        })
    }

    fn require_match(&self) -> Result<(), ExecError> {
        if let Some(error) = self.state.blocked {
            return Err(error);
        }
        match self.state.phase {
            Phase::Finished(true) => {}
            Phase::Failed(error) => return Err(error),
            _ => return Err(ExecError::NotMatched),
        }
        Ok(())
    }

    /// Copy spans only after a completed match. Output stays untouched for
    /// pending/no-match/error and insufficient output capacity.
    pub fn copy_captures(&mut self, output: &mut [Option<Span>]) -> Result<(), ExecError> {
        if output.len() < self.capture_count() {
            return Err(ExecError::Captures);
        }
        self.require_match()?;
        let count = self.capture_count();
        let scratch = self.buffers.scratch();
        copy_match_registers(scratch.registers, output, count)
    }

    pub fn required_scratch(&self) -> ScratchRequirements {
        ScratchRequirements {
            registers: self.shape.registers(),
            frames: self.state.frames + usize::from(self.state.blocked == Some(ExecError::Frames)),
            undo: self.state.undo + usize::from(self.state.blocked == Some(ExecError::Undo)),
        }
    }

    /// End this operation and return its scratch owner for reuse or release.
    /// Pending matching state is discarded; this is not a restart or a resume.
    /// Save `remaining_work` and copy completed captures first if needed.
    /// No resource view is acquired, no allocation occurs, and the returned
    /// owner has only the lifetimes of its own type. Contents remain opaque
    /// scratch; a subsequent search initializes its own live state.
    pub fn into_buffers(self) -> B {
        self.buffers
    }

    /// Move live scratch metadata to caller-owned replacement buffers between
    /// advances. This consumes the old borrow, so the caller can free the old
    /// allocations while the returned search continues. No subject is copied.
    /// A frame/undo request resumes at its pending update without replaying work.
    /// Capacity checks precede writes; failure returns both owners unchanged.
    // Returning the original search on failure is deliberate: boxing it would
    // introduce an allocation into the core's failure/ownership boundary.
    #[allow(clippy::result_large_err)]
    pub fn rebuffer<N: ScratchOwner>(
        mut self,
        mut buffers: N,
    ) -> Result<Search<'r, R, N>, RebufferError<Self, N>> {
        let need = self.required_scratch();
        let target = buffers.scratch();
        let error = if target.registers.len() < need.registers {
            Some(ExecError::Registers)
        } else if target.frames.len() < need.frames {
            Some(ExecError::Frames)
        } else if target.undo.len() < need.undo {
            Some(ExecError::Undo)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(RebufferError {
                error,
                search: self,
                buffers,
            });
        }
        let source = self.buffers.scratch();
        if source.registers.len() < need.registers
            || source.frames.len() < self.state.frames
            || source.undo.len() < self.state.undo
        {
            return Err(RebufferError {
                error: ExecError::ChangedResources,
                search: self,
                buffers,
            });
        }
        target.registers[..need.registers].copy_from_slice(&source.registers[..need.registers]);
        target.frames[..self.state.frames].copy_from_slice(&source.frames[..self.state.frames]);
        target.undo[..self.state.undo].copy_from_slice(&source.undo[..self.state.undo]);
        let mut state = self.state;
        state.blocked = None;
        Ok(Search {
            resources: self.resources,
            buffers,
            shape: self.shape,
            state,
            budget: self.budget,
        })
    }
}

struct Vm<'a, 'p, 's, 'w> {
    program: Program<'p>,
    input: Input<'a>,
    cursor: Cursor<'a>,
    scratch: &'w mut Scratch<'s>,
    state: State,
    budget: Budget,
}
enum Step {
    Next,
    Fail,
    Phase,
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
        if old != value {
            if self.state.frames > 0 {
                *self
                    .scratch
                    .undo
                    .get_mut(self.state.undo)
                    .ok_or(ExecError::Undo)? = Undo { slot, value: old };
                self.state.undo += 1;
            }
            self.scratch.registers[slot] = value;
        }
        Ok(())
    }
    fn push(&mut self, pc: usize, kind: u8) -> Result<(), ExecError> {
        *self
            .scratch
            .frames
            .get_mut(self.state.frames)
            .ok_or(ExecError::Frames)? = Frame {
            pc,
            position: self.cursor.mark(),
            undo: self.state.undo,
            assertion: self.state.assertion,
            kind,
            reverse: self.state.reverse,
        };
        self.state.frames += 1;
        Ok(())
    }
    fn read(&mut self) -> Option<u32> {
        read(&mut self.cursor, self.program.unicode(), self.state.reverse)
    }
    fn success(&mut self, success: bool) {
        self.state.phase = if success { Phase::Trial } else { Phase::Fail };
    }
    #[inline]
    fn restore(&mut self, mark: Mark) {
        // All VM marks originate from cursors on this immutable resource.
        // Search::advance checks the fresh view and saved current mark at the
        // borrow boundary. Within it, restoring another private mark changes
        // only offsets; it need not reconstruct/recheck the same input view.
        self.cursor.restore(mark);
    }
    fn seek(&mut self, target: usize, after: AfterSeek, available: usize) -> Result<(), ExecError> {
        let work = self.input.seek_work(target);
        if work == 1 || work <= available {
            self.charge(work)?;
            self.cursor = self
                .input
                .cursor_at(target)
                .ok_or(ExecError::InvalidProgram)?;
            self.sought(after)?;
        } else {
            self.charge(1)?;
            self.cursor = self
                .input
                .cursor_at(if target <= self.input.len_utf16() / 2 {
                    0
                } else {
                    self.input.len_utf16()
                })
                .unwrap();
            self.state.phase = Phase::Seek { target, after };
        }
        Ok(())
    }
    fn sought(&mut self, after: AfterSeek) -> Result<(), ExecError> {
        match after {
            AfterSeek::Start => {
                if self.program.unicode() {
                    self.cursor.normalize_unicode_start();
                }
                self.state.start = self.cursor.mark();
                self.state.phase = Phase::Initialize(0);
            }
            AfterSeek::Backref {
                start,
                end,
                matched,
            } => {
                let captured = self.cursor.mark();
                self.restore(matched);
                self.state.phase = Phase::Backref {
                    start,
                    end,
                    captured,
                };
            }
        }
        Ok(())
    }
    fn backref(&mut self, index: usize, available: usize) -> Result<(), ExecError> {
        let start = self.scratch.registers[index * 2];
        let end = self.scratch.registers[index * 2 + 1];
        if start == UNSET || end == UNSET {
            self.state.phase = Phase::Trial;
        } else {
            if start > end || end > self.input.len_utf16() {
                return Err(ExecError::InvalidProgram);
            }
            let after = AfterSeek::Backref {
                start,
                end,
                matched: self.cursor.mark(),
            };
            self.seek(
                if self.state.reverse { end } else { start },
                after,
                available,
            )?;
        }
        Ok(())
    }
    fn begin_class(&mut self, a: u32, b: u32, c: u32, admission: bool) {
        let values = if self.program.words[2] & I != 0 {
            casefold::equivalents(c, self.program.unicode())
        } else {
            [c; 4]
        };
        self.state.phase = Phase::Class {
            index: a,
            end: a + (b & !NEGATED),
            negated: b & NEGATED != 0,
            values,
            admission,
        };
    }
    fn class_result(&mut self, found: bool, negated: bool, admission: bool) {
        let success = found != negated;
        if admission {
            self.state.phase = if success {
                Phase::Start
            } else {
                Phase::AdmitClass
            };
        } else {
            self.success(success);
        }
    }
    fn begin_trial(&mut self) {
        self.state.pc = 0;
        self.state.frames = 0;
        self.state.undo = 0;
        self.state.assertion = UNSET;
        self.state.reverse = false;
        self.restore(self.state.start);
        self.state.phase = Phase::Trial;
    }

    // Ordinary opcodes stay in one dispatch loop. A long sub-operation, pause,
    // failure or capacity request returns to the outer resumable phase loop.
    // This is the same instruction implementation for both entry points.
    fn trial(&mut self, quantum: usize) -> Result<(), ExecError> {
        let initial = self.budget.remaining();
        let mut paid = match self.state.phase {
            Phase::Execute { op, a, b } => Some([op, a, b]),
            _ => None,
        };
        self.state.phase = Phase::Trial;
        loop {
            if initial - self.budget.remaining() >= quantum {
                return Ok(());
            }
            let [op, a, b] = if let Some(opcode) = paid.take() {
                opcode
            } else {
                self.charge(1)?;
                if self.state.pc >= self.program.instructions() {
                    return Err(ExecError::InvalidProgram);
                }
                let opcode = self.program.instruction(self.state.pc);
                self.state.pc += 1;
                opcode
            };
            let available = quantum.saturating_sub(initial - self.budget.remaining());
            match self.instruction(op, a, b, available) {
                Ok(Step::Next) => {}
                Ok(Step::Fail) => {
                    self.state.phase = Phase::Fail;
                    return Ok(());
                }
                Ok(Step::Phase) => return Ok(()),
                Err(error) => {
                    if matches!(error, ExecError::Frames | ExecError::Undo) {
                        self.state.phase = Phase::Execute { op, a, b };
                    }
                    return Err(error);
                }
            }
        }
    }
    fn run(&mut self, quantum: usize) -> Result<Progress, ExecError> {
        let initial = self.budget.remaining();
        loop {
            if let Some(result) = self.state.phase.outcome() {
                return result;
            }
            let used = initial - self.budget.remaining();
            if used >= quantum {
                return Ok(Progress::Pending);
            }
            let available = quantum - used;
            match self.state.phase {
                Phase::Admission
                | Phase::AdmitBytes { .. }
                | Phase::AdmitByteClass { .. }
                | Phase::AdmitClass
                | Phase::AdmitSuffix(_)
                | Phase::AdmitScan
                | Phase::AdmitProbe { .. } => self.admit_step()?,
                Phase::Start => {
                    self.seek(self.state.requested_start, AfterSeek::Start, available)?
                }
                Phase::Seek { target, after } => {
                    if self.cursor.position() == target {
                        self.sought(after)?;
                    } else {
                        self.charge(1)?;
                        let unit = if self.cursor.position() < target {
                            self.cursor.next_unit()
                        } else {
                            self.cursor.previous_unit()
                        };
                        if unit.is_none() {
                            return Err(ExecError::InvalidProgram);
                        }
                    }
                }
                Phase::Initialize(index) => {
                    if index == self.program.register_count() {
                        self.begin_trial();
                    } else {
                        let end = self
                            .program
                            .register_count()
                            .min(index + available.min(256));
                        self.charge(end - index)?;
                        self.scratch.registers[index..end].fill(UNSET);
                        if end == self.program.register_count() {
                            self.begin_trial();
                        } else {
                            self.state.phase = Phase::Initialize(end);
                        }
                    }
                }
                Phase::Trial | Phase::Execute { .. } => self.trial(available)?,
                Phase::Class {
                    index,
                    end,
                    negated,
                    values,
                    admission,
                } => {
                    if index == end {
                        self.class_result(false, negated, admission);
                    } else {
                        self.charge(1)?;
                        let [lo, hi] = self.program.range(index as usize);
                        let found = values.iter().any(|&value| {
                            if lo & PROPERTY != 0 {
                                properties::contains(lo & !PROPERTY, value) != (hi != 0)
                            } else {
                                value >= lo && value <= hi
                            }
                        });
                        if found {
                            self.class_result(true, negated, admission);
                        } else {
                            self.state.phase = Phase::Class {
                                index: index + 1,
                                end,
                                negated,
                                values,
                                admission,
                            };
                        }
                    }
                }
                Phase::Named {
                    group,
                    next,
                    selected,
                } => {
                    let captures = self
                        .program
                        .named_group(group)
                        .ok_or(ExecError::InvalidProgram)?
                        .capture_indices();
                    if next == captures.len() {
                        if selected == UNSET {
                            self.state.phase = Phase::Trial;
                        } else {
                            self.backref(selected, available)?;
                        }
                    } else {
                        self.charge(1)?;
                        let index = captures[next] as usize;
                        let complete = self.scratch.registers[index * 2] != UNSET
                            && self.scratch.registers[index * 2 + 1] != UNSET;
                        if complete && selected != UNSET {
                            return Err(ExecError::InvalidProgram);
                        }
                        self.state.phase = Phase::Named {
                            group,
                            next: next + 1,
                            selected: if complete { index } else { selected },
                        };
                    }
                }
                Phase::Backref {
                    start,
                    end,
                    captured,
                } => {
                    let mut left = self.cursor;
                    left.restore(captured);
                    let until = if self.state.reverse { start } else { end };
                    if left.position() == until {
                        self.state.phase = Phase::Trial;
                    } else {
                        self.charge(1)?;
                        let point = read(&mut left, self.program.unicode(), self.state.reverse)
                            .ok_or(ExecError::InvalidProgram)?;
                        if left.position() < start || left.position() > end {
                            return Err(ExecError::InvalidProgram);
                        }
                        if self
                            .read()
                            .is_some_and(|right| equal(self.program, point, right))
                        {
                            self.state.phase = Phase::Backref {
                                start,
                                end,
                                captured: left.mark(),
                            };
                        } else {
                            self.state.phase = Phase::Fail;
                        }
                    }
                }
                Phase::Clear { next, end, slot } => {
                    if next == end {
                        self.store(slot, self.cursor.position())?;
                        self.state.phase = Phase::Trial;
                    } else {
                        self.charge(1)?;
                        self.state.phase = Phase::ClearStore { next, end, slot };
                        self.store(next, UNSET)?;
                        self.state.phase = Phase::Clear {
                            next: next + 1,
                            end,
                            slot,
                        };
                    }
                }
                Phase::ClearStore { next, end, slot } => {
                    self.store(next, UNSET)?;
                    self.state.phase = Phase::Clear {
                        next: next + 1,
                        end,
                        slot,
                    };
                }
                Phase::Fail => {
                    if self.state.frames == 0 {
                        self.state.phase = Phase::NextStart;
                    } else {
                        self.charge(1)?;
                        self.state.frames -= 1;
                        let frame = self.scratch.frames[self.state.frames];
                        self.restore(frame.position);
                        self.state.reverse = frame.reverse;
                        self.state.assertion = frame.assertion;
                        self.state.pc = frame.pc;
                        self.state.phase = Phase::Rollback {
                            until: frame.undo,
                            fail: frame.kind == 1,
                        };
                    }
                }
                Phase::Rollback { until, fail } => {
                    if self.state.undo > until {
                        self.charge(1)?;
                        self.state.undo -= 1;
                        let old = self.scratch.undo[self.state.undo];
                        self.scratch.registers[old.slot] = old.value;
                    } else {
                        if self.state.undo < until {
                            return Err(ExecError::InvalidProgram);
                        }
                        if self.state.frames == 0 {
                            self.state.undo = 0;
                        }
                        self.success(!fail);
                    }
                }
                Phase::NextStart => {
                    if self.program.words[2] & Y != 0 {
                        self.state.phase = Phase::Finished(false);
                    } else {
                        self.charge(1)?;
                        self.restore(self.state.start);
                        if read(&mut self.cursor, self.program.unicode(), false).is_none() {
                            self.state.phase = Phase::Finished(false);
                        } else {
                            self.state.start = self.cursor.mark();
                            self.state.phase = Phase::Initialize(0);
                        }
                    }
                }
                Phase::Validate(index) => {
                    let end = self
                        .program
                        .capture_count()
                        .min(index + (available / 2).clamp(1, 128));
                    for index in index..end {
                        self.charge(2)?;
                        let lo = self.scratch.registers[index * 2];
                        let hi = self.scratch.registers[index * 2 + 1];
                        if (index == 0 && (lo == UNSET || hi == UNSET))
                            || (lo != UNSET
                                && hi != UNSET
                                && (lo > hi || hi > self.input.len_utf16()))
                        {
                            return Err(ExecError::InvalidProgram);
                        }
                    }
                    self.state.phase = if end == self.program.capture_count() {
                        Phase::Finished(true)
                    } else {
                        Phase::Validate(end)
                    };
                }
                Phase::Finished(_) | Phase::Failed(_) => unreachable!(),
            }
        }
    }
    #[inline(always)]
    fn instruction(
        &mut self,
        op: u32,
        a: u32,
        b: u32,
        available: usize,
    ) -> Result<Step, ExecError> {
        let mut success = true;
        match op {
            MATCH => {
                if self.state.assertion != UNSET {
                    return Err(ExecError::InvalidProgram);
                }
                self.state.phase = Phase::Validate(0);
                return Ok(Step::Phase);
            }
            CHAR => success = self.read().is_some_and(|c| equal(self.program, c, a)),
            ANY => {
                success = self
                    .read()
                    .is_some_and(|c| self.program.words[2] & S != 0 || !line_terminator(c))
            }
            CLASS => {
                if let Some(c) = self.read() {
                    self.begin_class(a, b, c, false);
                    return Ok(Step::Phase);
                }
                success = false;
            }
            SAVE => self.store(a as usize, self.cursor.position())?,
            SPLIT => {
                self.push(b as usize, 0)?;
                self.state.pc = a as usize;
            }
            JUMP => self.state.pc = a as usize,
            START => {
                let mut before = self.cursor;
                success = self.cursor.position() == 0
                    || (self.program.words[2] & M != 0
                        && before
                            .previous_unit()
                            .is_some_and(|u| line_terminator(u32::from(u))));
            }
            END => {
                let mut after = self.cursor;
                success = self.cursor.position() == self.input.len_utf16()
                    || (self.program.words[2] & M != 0
                        && after
                            .next_unit()
                            .is_some_and(|u| line_terminator(u32::from(u))));
            }
            WORD => {
                let mut before = self.cursor;
                let mut after = self.cursor;
                let ui = self.program.words[2] & (U | I) == U | I;
                let left = read(&mut before, self.program.unicode(), true);
                let right = read(&mut after, self.program.unicode(), false);
                success = (left.is_some_and(|c| casefold::word(c, ui))
                    != right.is_some_and(|c| casefold::word(c, ui)))
                    != (a != 0);
            }
            BACKREF => {
                self.backref(a as usize, available)?;
                return Ok(Step::Phase);
            }
            NAMED_BACKREF => {
                self.state.phase = Phase::Named {
                    group: a as usize,
                    next: 0,
                    selected: UNSET,
                };
                return Ok(Step::Phase);
            }
            ASSERT => {
                self.push(a as usize, if b & 1 == 0 { 1 } else { 2 })?;
                self.state.assertion = self.state.frames - 1;
                self.state.reverse = b & 2 != 0;
            }
            ASSERT_END => {
                if self.state.assertion == UNSET || self.state.assertion >= self.state.frames {
                    return Err(ExecError::InvalidProgram);
                }
                let frame = self.scratch.frames[self.state.assertion];
                self.state.frames = self.state.assertion;
                self.restore(frame.position);
                self.state.reverse = frame.reverse;
                self.state.assertion = frame.assertion;
                if frame.kind == 2 {
                    self.state.phase = Phase::Rollback {
                        until: frame.undo,
                        fail: true,
                    };
                    return Ok(Step::Phase);
                }
                self.state.pc = frame.pc;
                if self.state.frames == 0 {
                    self.state.undo = 0;
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
                    self.state.pc = r[4] as usize;
                } else if count >= r[0] as usize {
                    if r[2] & 2 == 0 {
                        self.push(r[4] as usize, 0)?;
                    } else {
                        self.push(r[3] as usize, 0)?;
                        self.state.pc = r[4] as usize;
                    }
                }
            }
            REPEAT_BODY => {
                let r = self.program.repeat(a as usize);
                let slot = self.program.capture_count() * 2 + a as usize * 2 + 1;
                if r[5] == r[6] {
                    self.store(slot, self.cursor.position())?;
                } else {
                    self.state.phase = Phase::Clear {
                        next: r[5] as usize * 2,
                        end: r[6] as usize * 2,
                        slot,
                    };
                    return Ok(Step::Phase);
                }
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
                    self.state.pc = r[3] as usize - 1;
                }
            }
            _ => return Err(ExecError::InvalidProgram),
        }
        Ok(if success { Step::Next } else { Step::Fail })
    }
}
fn copy_match_registers(
    registers: &[usize],
    output: &mut [Option<Span>],
    count: usize,
) -> Result<(), ExecError> {
    let registers = registers
        .get(..count * 2)
        .ok_or(ExecError::ChangedResources)?;
    for (i, target) in output[..count].iter_mut().enumerate() {
        let lo = registers[i * 2];
        let hi = registers[i * 2 + 1];
        *target = if lo == UNSET || hi == UNSET {
            None
        } else {
            Span::new(lo, hi)
        };
    }
    Ok(())
}
fn read(cursor: &mut Cursor<'_>, unicode: bool, reverse: bool) -> Option<u32> {
    match (unicode, reverse) {
        (true, false) => cursor.next_point(),
        (true, true) => cursor.previous_point(),
        (false, false) => cursor.next_unit().map(u32::from),
        (false, true) => cursor.previous_unit().map(u32::from),
    }
}

/// Synchronous execution of the same resumable evaluator. Output is untouched
/// on no-match/error. The host owns lastIndex, result objects and global loops.
pub fn find(
    program: Program<'_>,
    input: Input<'_>,
    start_utf16: usize,
    mut scratch: Scratch<'_>,
    captures: &mut [Option<Span>],
    budget: &mut Budget,
) -> Result<bool, ExecError> {
    if captures.len() < program.capture_count() {
        return Err(ExecError::Captures);
    }
    if scratch.registers.len() < program.register_count() {
        return Err(ExecError::Registers);
    }
    // The caller already holds both immutable views. Enter the same evaluator
    // directly; constructing a rooted-owner binding is unnecessary for this
    // single borrow. Search uses this identical VM across multiple borrows.
    let mut vm = Vm {
        program,
        input,
        cursor: input.cursor(),
        scratch: &mut scratch,
        state: State::new(start_utf16, input.len_utf16()),
        budget: *budget,
    };
    let result = vm.run(usize::MAX);
    *budget = vm.budget;
    match result? {
        Progress::Matched => {
            copy_match_registers(vm.scratch.registers, captures, program.capture_count())?;
            Ok(true)
        }
        Progress::NoMatch => Ok(false),
        Progress::Pending => Err(ExecError::InvalidProgram),
    }
}
