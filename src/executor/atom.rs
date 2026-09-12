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
    /// Whether this atom can be decided without the resumable class phase.
    fn inline_atom(&self, op: u32, b: u32) -> bool {
        matches!(op, CHAR | ANY | ANY_S) || (op == CLASS && (b & !NEGATED) <= ATOM_CLASS_RANGES)
    }

    /// Membership for an atom `inline_atom` accepted.
    fn atom_holds(&mut self, op: u32, a: u32, b: u32, c: u32) -> Result<bool, ExecError> {
        Ok(match op {
            CHAR => equal(self.program, c, a, false),
            ANY | ANY_S => op == ANY_S || !line_terminator(c),
            _ => {
                let mut found = false;
                for index in a..a + (b & !NEGATED) {
                    self.charge(1)?;
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
        })
    }

    /// The charge one character of this atom always costs, when that is the
    /// same whatever the character is. `None` when it is data-dependent, which
    /// a byte scan could not reproduce and so must not replace.
    fn fixed_atom_charge(&self, op: u32, a: u32, b: u32) -> Option<(usize, u8, u8)> {
        match op {
            // One unit for the character; `equal` charges nothing.
            CHAR if a < 128 => Some((1, a as u8, a as u8)),
            // One unit for the character and one for the single range examined,
            // which is the same whether or not it matches.
            CLASS if b == 1 => {
                let [lo, hi] = self.program.range(a as usize);
                (lo & PROPERTY == 0 && hi < 128).then_some((2, lo as u8, hi as u8))
            }
            _ => None,
        }
    }

    /// Walk a run of a fixed-charge atom directly over original bytes, which
    /// costs a fraction of decoding each character through the cursor. Returns
    /// how many characters the run holds from `start`.
    fn byte_run(bytes: &[u8], start: usize, lo: u8, hi: u8, limit: usize) -> usize {
        const HIGH: u64 = 0x8080_8080_8080_8080;
        const ONES: u64 = 0x0101_0101_0101_0101;
        let end = bytes.len().min(start + limit);
        let mut i = start;
        let (lower, upper) = (u64::from(lo) * ONES, u64::from(hi) * ONES);
        while i + 8 <= end {
            let word = u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
            // High bit set for each lane inside the range, as in the start scan.
            let inside =
                ((word | HIGH).wrapping_sub(lower)) & ((upper | HIGH).wrapping_sub(word)) & HIGH;
            if inside != HIGH {
                return i - start + (!inside & HIGH).trailing_zeros() as usize / 8;
            }
            i += 8;
        }
        while i < end && bytes[i] >= lo && bytes[i] <= hi {
            i += 1;
        }
        i - start
    }

    pub(super) fn atom_scan(&mut self, extend: bool, available: usize) -> Result<(), ExecError> {
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
        let [op, a, b] = self.program.instruction(self.state.pc + 3);
        // Once a greedy unbounded repeat has met its minimum, consuming another
        // character changes no counter: it only advances the cursor. Walking
        // that run here instead of one character per phase round trip avoids
        // re-reading the repeat record and rewriting the work state twice each
        // time. Every decision still belongs to the code below, which re-reads
        // the character the run stopped at.
        if !extend && r[2] & 2 == 0 && infinite && atom.needed == 0 && self.inline_atom(op, b) {
            // The most one character can charge: its own unit plus a range
            // walk. Keeping that much quantum and budget in hand means no
            // charge inside the loop can fail, so the cursor never stops
            // between a character's read and its decision.
            // A fixed-charge atom over ASCII storage is walked as bytes. The
            // charge is identical to deciding each character separately, so a
            // pause still reaches the same total, and the cursor moves once.
            if let Some((per, lo, hi)) = self.fixed_atom_charge(op, a, b)
                && let Some(bytes) = self.input.ascii_bytes()
                && !self.state.reverse
            {
                let start = self.cursor.position();
                let limit = (available / per).min(self.budget.remaining() / per);
                let run = Self::byte_run(bytes, start, lo, hi, limit);
                if run != 0 {
                    self.charge(run * per)?;
                    atom.before = self
                        .input
                        .cursor_at(start + run - 1)
                        .ok_or(ExecError::InvalidProgram)?
                        .mark();
                    self.cursor = self
                        .input
                        .cursor_at(start + run)
                        .ok_or(ExecError::InvalidProgram)?;
                    self.state.work = Work::Atom(atom);
                }
                // The stopping character is left to the ordinary step below,
                // which charges and decides it exactly as it always has.
                if run != 0 && run == limit {
                    return Ok(());
                }
            }
            let cost = 1 + if op == CLASS { b & !NEGATED } else { 0 } as usize;
            let mut used = 0;
            let mut ended = false;
            while used + cost <= available && self.budget.remaining() > cost {
                let before = self.cursor.mark();
                // Charged in the same order as the ordinary step: the character
                // first, then whatever deciding it costs. A pause must not
                // change the total, so the stopping character pays here too.
                self.charge(1)?;
                let Some(c) = self.read() else {
                    self.restore(before);
                    ended = true;
                    break;
                };
                if !self.atom_holds(op, a, b, c)? {
                    self.restore(before);
                    ended = true;
                    break;
                }
                atom.before = before;
                used += cost;
            }
            self.state.work = Work::Atom(atom);
            if ended {
                // The run stops here. This is the same conclusion the ordinary
                // step reaches, taken without charging the stopping character
                // twice, and the run's end is recorded for the next start.
                if self.program.run_skip() == Some(self.state.pc) {
                    self.state.run_end = self.cursor.position();
                }
                self.state.phase = Phase::AtomCommit;
                return Ok(());
            }
            if used != 0 {
                // The quantum ran out mid-run; resume this same phase.
                return Ok(());
            }
        }
        self.charge(1)?;
        atom.before = self.cursor.mark();
        self.state.work = Work::Atom(atom);
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
