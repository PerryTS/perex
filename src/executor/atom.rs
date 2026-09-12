//! One retry record for a repeated, noncapturing consuming atom. All scanning
//! and retreat uses the current original input view; pauses retain offsets only.
use super::*;

/// Ranges a repeated class may hold and still be tested in one step. The step
/// stays bounded work, like the sorted-class search, while covering the small
/// classes ordinary patterns repeat.
const ATOM_CLASS_RANGES: u32 = 8;

impl Vm<'_, '_, '_, '_> {
    fn atom_state(&self) -> Result<AtomState, ExecError> {
        match self.state.work {
            Work::Atom(state) => Ok(state),
            _ => Err(ExecError::InvalidProgram),
        }
    }
    fn atom_record(&self) -> Result<[u32; 8], ExecError> {
        if self.state.pc >= self.program.instructions() {
            return Err(ExecError::InvalidProgram);
        }
        let [op, id, _] = self.program.instruction(self.state.pc);
        if op != ATOM_REPEAT {
            return Err(ExecError::InvalidProgram);
        }
        Ok(self.program.repeat(id as usize))
    }
    pub(super) fn atom_scan(&mut self, extend: bool) -> Result<(), ExecError> {
        let mut atom = self.atom_state()?;
        let r = self.atom_record()?;
        let infinite = r[2] & 1 != 0;
        if !extend && atom.needed == 0 && (r[2] & 2 != 0 || (!infinite && atom.remaining == 0)) {
            self.state.phase = Phase::AtomCommit;
            return Ok(());
        }
        if extend && (atom.needed != 0 || (!infinite && atom.remaining == 0)) {
            return Err(ExecError::InvalidProgram);
        }
        self.charge(1)?;
        atom.before = self.cursor.mark();
        self.state.work = Work::Atom(atom);
        let [op, a, b] = self.program.instruction(self.state.pc + 3);
        let matched = if let Some(c) = self.read() {
            match op {
                CHAR | CHAR_I => equal(self.program, c, a, op == CHAR_I),
                ANY | ANY_S => op == ANY_S || !line_terminator(c),
                // A repeated plain class is tested here rather than through the
                // resumable class phase. That phase exists for folding, sorted
                // search and classes too large for one step; building and taking
                // it apart again per character costs several times the
                // membership test itself, and this is the engine's hottest loop.
                CLASS if (b & !NEGATED) <= ATOM_CLASS_RANGES => {
                    let mut found = false;
                    for index in a..a + (b & !NEGATED) {
                        if self.charge(1).is_err() {
                            // Resume through the phase, which re-tests the class
                            // from its first range with the character already read.
                            self.begin_class(
                                a,
                                b,
                                c,
                                if extend {
                                    ClassUse::AtomExtend
                                } else {
                                    ClassUse::AtomScan
                                },
                                false,
                                false,
                            );
                            return Err(ExecError::WorkLimit);
                        }
                        let [lo, hi] = self.program.range(index as usize);
                        found = if lo & PROPERTY != 0 {
                            if hi == 2 {
                                !properties::contains(lo & !PROPERTY, c)
                            } else {
                                properties::contains(lo & !PROPERTY, c) != (hi != 0)
                            }
                        } else {
                            c >= lo && c <= hi
                        };
                        if found {
                            break;
                        }
                    }
                    found != (b & NEGATED != 0)
                }
                CLASS | CLASS_I | CLASS_SORTED | CLASS_SORTED_I => {
                    self.begin_class(
                        a,
                        b,
                        c,
                        if extend {
                            ClassUse::AtomExtend
                        } else {
                            ClassUse::AtomScan
                        },
                        matches!(op, CLASS_I | CLASS_SORTED_I),
                        matches!(op, CLASS_SORTED | CLASS_SORTED_I),
                    );
                    return Ok(());
                }
                _ => return Err(ExecError::InvalidProgram),
            }
        } else {
            false
        };
        self.atom_result(matched, extend)
    }

    pub(super) fn atom_result(&mut self, matched: bool, extend: bool) -> Result<(), ExecError> {
        let mut atom = self.atom_state()?;
        let r = self.atom_record()?;
        if matched {
            if atom.needed > 0 {
                atom.needed -= 1;
                atom.minimum_end = self.cursor.position();
            } else if r[2] & 1 == 0 {
                atom.remaining = atom
                    .remaining
                    .checked_sub(1)
                    .ok_or(ExecError::InvalidProgram)?;
            }
            self.state.work = Work::Atom(atom);
            self.state.phase = if extend {
                Phase::AtomCommit
            } else {
                Phase::AtomScan
            };
        } else {
            self.restore(atom.before);
            // The run this scan just walked ends here. When it is the pattern's
            // leading repeat, every later start inside it fails with this one.
            if self.program.run_skip() == Some(self.state.pc) {
                self.state.run_end = self.cursor.position();
            }
            self.state.phase = if extend || atom.needed != 0 {
                Phase::Fail
            } else {
                Phase::AtomCommit
            };
        }
        Ok(())
    }

    pub(super) fn atom_commit(&mut self) -> Result<(), ExecError> {
        let atom = self.atom_state()?;
        let r = self.atom_record()?;
        if atom.needed != 0 {
            return Err(ExecError::InvalidProgram);
        }
        let lazy = r[2] & 2 != 0;
        let retry = if lazy {
            r[2] & 1 != 0 || atom.remaining != 0
        } else {
            self.cursor.position() != atom.minimum_end
        };
        if retry {
            // A capacity failure leaves this phase and all paid scan/retreat
            // state intact. Replacement scratch can resume just this store.
            self.push(self.state.pc, if lazy { 4 } else { 3 })?;
            self.scratch.frames[self.state.frames - 1].limit = if lazy {
                atom.remaining as usize
            } else {
                atom.minimum_end
            };
        }
        self.state.pc = r[4] as usize;
        self.state.work = Work::Idle;
        self.state.phase = Phase::Trial;
        Ok(())
    }

    pub(super) fn atom_retreat(&mut self, available: usize) -> Result<(), ExecError> {
        let atom = self.atom_state()?;
        let r = self.atom_record()?;
        let position = self.cursor.position();
        if r[2] & 2 != 0
            || if self.state.reverse {
                position >= atom.minimum_end
            } else {
                position <= atom.minimum_end
            }
        {
            return Err(ExecError::InvalidProgram);
        }
        let [next, value, _] = self.program.instruction(r[4] as usize);
        if matches!(next, CHAR | CHAR_I) {
            return self.retreat_literal(atom.minimum_end, value, next == CHAR_I, available);
        }
        self.charge(1)?;
        // Every position between this endpoint and the minimum was already
        // accepted by this same atom on this immutable input. No membership
        // recheck, subject copy or saved per-character record is necessary.
        read(
            &mut self.cursor,
            self.program.unicode(),
            !self.state.reverse,
        )
        .ok_or(ExecError::InvalidProgram)?;
        if if self.state.reverse {
            self.cursor.position() > atom.minimum_end
        } else {
            self.cursor.position() < atom.minimum_end
        } {
            return Err(ExecError::InvalidProgram);
        }
        self.state.phase = Phase::AtomCommit;
        Ok(())
    }

    // The immediate continuation must consume this literal before it can change
    // captures or choose another branch. Impossible endpoints need no retry
    // frame or capture rollback. Leave possible endpoints to the same VM.
    #[inline(never)]
    fn retreat_literal(
        &mut self,
        minimum_end: usize,
        value: u32,
        fold: bool,
        available: usize,
    ) -> Result<(), ExecError> {
        let initial = self.budget.remaining();
        let limit = available.min(256);
        loop {
            self.charge(1)?;
            read(
                &mut self.cursor,
                self.program.unicode(),
                !self.state.reverse,
            )
            .ok_or(ExecError::InvalidProgram)?;
            let position = self.cursor.position();
            if if self.state.reverse {
                position > minimum_end
            } else {
                position < minimum_end
            } {
                return Err(ExecError::InvalidProgram);
            }
            self.charge(1)?;
            let mut probe = self.cursor;
            let possible = read(&mut probe, self.program.unicode(), self.state.reverse)
                .is_some_and(|c| equal(self.program, c, value, fold));
            if possible || position == minimum_end {
                self.state.phase = Phase::AtomCommit;
                return Ok(());
            }
            // Pauses retain only the existing endpoint and atom metadata. The
            // probe never escapes this view; the operation budget is continuous.
            if initial - self.budget.remaining() >= limit {
                return Ok(());
            }
        }
    }
}
