//! One retry record for a repeated, noncapturing consuming atom. All scanning
//! and retreat uses the current original input view; pauses retain offsets only.
use super::*;

/// What a fixed-charge atom accepts, as a predicate over original ASCII bytes.
#[derive(Clone, Copy)]
enum AtomBytes {
    /// Inside an inclusive range, as a plain single-range class.
    Inside(u8, u8),
    /// Outside one, as that class negated.
    Outside(u8, u8),
    /// Anything but a line terminator, as `.` without the `s` flag. On ASCII
    /// storage only the two single-byte terminators can occur.
    Unlined,
    /// Anything, as `.` with the `s` flag.
    Every,
    /// Inside any of several inclusive ranges, or outside all of them when
    /// negated. Only ranges an ASCII subject can reach are kept.
    Ranges([(u8, u8); ATOM_CLASS_RANGES as usize], u8, bool),
}

impl AtomBytes {
    fn holds(self, byte: u8) -> bool {
        match self {
            Self::Inside(lo, hi) => byte >= lo && byte <= hi,
            Self::Outside(lo, hi) => byte < lo || byte > hi,
            Self::Unlined => byte != b'\n' && byte != b'\r',
            Self::Every => true,
            Self::Ranges(ranges, count, negated) => {
                let inside = ranges[..count as usize]
                    .iter()
                    .any(|&(lo, hi)| byte >= lo && byte <= hi);
                inside != negated
            }
        }
    }
}

/// Bytes a span must hold before block comparisons pay for their setup.
const BLOCK_MIN: usize = 256;

/// The high bit and the low bit of every lane of a word, for comparing eight
/// bytes at once without a borrow crossing between them.
const HIGH: u64 = 0x8080_8080_8080_8080;
const ONES: u64 = 0x0101_0101_0101_0101;

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
                // Charged for the whole class rather than per range examined.
                // A byte scan cannot know which range matched, and a paused
                // search must reach the same total as an unpaused one, so the
                // cost of a repeated class is its size either way.
                self.charge((b & !NEGATED) as usize)?;
                let mut found = false;
                for index in a..a + (b & !NEGATED) {
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
    /// The charge one character of this atom always costs, when that is the
    /// same whatever the character is, with the predicate to walk it by. `None`
    /// when the charge is data-dependent, which a byte scan cannot reproduce.
    fn fixed_atom_charge(&self, op: u32, a: u32, b: u32) -> Option<(usize, AtomBytes)> {
        match op {
            // One unit for the character; `equal` charges nothing.
            CHAR if a < 128 => Some((1, AtomBytes::Inside(a as u8, a as u8))),
            ANY => Some((1, AtomBytes::Unlined)),
            ANY_S => Some((1, AtomBytes::Every)),
            // One unit for the character and one for the single range examined,
            // which is the same whether or not it matches.
            CLASS if b & !NEGATED <= ATOM_CLASS_RANGES && b & !NEGATED != 0 => {
                let count = b & !NEGATED;
                let mut ranges = [(0, 0); ATOM_CLASS_RANGES as usize];
                let mut kept = 0;
                for index in a..a + count {
                    let [lo, hi] = self.program.range(index as usize);
                    if lo & PROPERTY != 0 || lo > hi {
                        return None;
                    }
                    // A range beyond ASCII can never match this storage, so it
                    // is dropped rather than making the class ineligible.
                    if lo < 128 {
                        ranges[kept] = (lo as u8, hi.min(127) as u8);
                        kept += 1;
                    }
                }
                let negated = b & NEGATED != 0;
                // One range keeps its own predicate: the general form walks a
                // slice per word, which costs more than the single comparison
                // the common case needs.
                let accepts = match (kept, negated) {
                    (1, false) => AtomBytes::Inside(ranges[0].0, ranges[0].1),
                    (1, true) => AtomBytes::Outside(ranges[0].0, ranges[0].1),
                    _ => AtomBytes::Ranges(ranges, kept as u8, negated),
                };
                Some((1 + count as usize, accepts))
            }
            _ => None,
        }
    }

    /// Walk a run of a fixed-charge atom directly over original bytes, which
    /// costs a fraction of decoding each character through the cursor. Returns
    /// how many characters the run holds from `start`.
    /// Walk a run of a fixed-charge atom directly over original bytes, which
    /// costs a fraction of decoding each character through the cursor. Returns
    /// how many characters the run holds from `start`.
    ///
    /// Each predicate gets its own loop. Deciding which one applies per word,
    /// and rebuilding its comparison words there, costs more than the
    /// comparison itself; decided once, each loop is a fixed sequence over a
    /// word the compiler can widen further where the target allows.
    fn byte_run(bytes: &[u8], start: usize, accepts: AtomBytes, limit: usize) -> usize {
        let end = bytes.len().min(start + limit);
        match accepts {
            AtomBytes::Every => end - start,
            AtomBytes::Inside(lo, hi) => Self::run_range(bytes, start, end, lo, hi, false),
            AtomBytes::Outside(lo, hi) => Self::run_range(bytes, start, end, lo, hi, true),
            AtomBytes::Unlined => Self::run_unlined(bytes, start, end),
            AtomBytes::Ranges(ranges, count, negated) => {
                Self::run_ranges(bytes, start, end, &ranges[..count as usize], negated)
            }
        }
    }

    /// Lanes of a word that hold a byte inside `[lower, upper]`, marked by
    /// their high bit. Both bounds arrive already spread over every lane.
    fn inside_lanes(word: u64, lower: u64, upper: u64) -> u64 {
        (word | HIGH).wrapping_sub(lower) & (upper | HIGH).wrapping_sub(word) & HIGH
    }

    /// Characters from `start` inside one inclusive range, or outside it.
    fn run_range(bytes: &[u8], start: usize, end: usize, lo: u8, hi: u8, outside: bool) -> usize {
        let (lower, upper) = (u64::from(lo) * ONES, u64::from(hi) * ONES);
        let mut i = start;
        while i + 8 <= end {
            let word = u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
            let inside = Self::inside_lanes(word, lower, upper);
            let rejected = if outside { inside } else { !inside & HIGH };
            if rejected != 0 {
                return i - start + rejected.trailing_zeros() as usize / 8;
            }
            i += 8;
        }
        while i < end && ((bytes[i] >= lo && bytes[i] <= hi) != outside) {
            i += 1;
        }
        i - start
    }

    /// Characters from `start` that are not line terminators. On ASCII storage
    /// only the two single-byte terminators can occur.
    fn run_unlined(bytes: &[u8], start: usize, end: usize) -> usize {
        let (nl, cr) = (u64::from(b'\n') * ONES, u64::from(b'\r') * ONES);
        let mut i = start;
        while i + 8 <= end {
            let word = u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
            let (a, b) = (word ^ nl, word ^ cr);
            let rejected = (a.wrapping_sub(ONES) & !a & HIGH) | (b.wrapping_sub(ONES) & !b & HIGH);
            if rejected != 0 {
                return i - start + rejected.trailing_zeros() as usize / 8;
            }
            i += 8;
        }
        while i < end && bytes[i] != b'\n' && bytes[i] != b'\r' {
            i += 1;
        }
        i - start
    }

    /// Characters from `start` inside any of several inclusive ranges, or
    /// outside all of them.
    fn run_ranges(
        bytes: &[u8],
        start: usize,
        end: usize,
        ranges: &[(u8, u8)],
        negated: bool,
    ) -> usize {
        // Past a few blocks, testing each range against every lane of a block
        // widens into vector comparisons where the target has them. Below that
        // the block's own setup costs more than the lane arithmetic it
        // replaces, so short runs keep the word loop.
        let mut base = start;
        if end.saturating_sub(start) >= BLOCK_MIN {
            const LANES: usize = 32;
            while base + LANES <= end {
                let block: [u8; LANES] = bytes[base..base + LANES].try_into().unwrap();
                let mut inside = [0u8; LANES];
                for &(lo, hi) in ranges {
                    for lane in 0..LANES {
                        inside[lane] |= u8::from(block[lane] >= lo && block[lane] <= hi);
                    }
                }
                // Find the lane the run stops at through the word it lands
                // in rather than by a scalar pass over every lane, which on a
                // short run costs more than the comparisons it follows.
                for (word, lanes) in inside.chunks_exact(8).enumerate() {
                    let lanes = u64::from_le_bytes(lanes.try_into().unwrap());
                    let stop = if negated { lanes } else { !lanes & ONES };
                    if stop != 0 {
                        let lane = word * 8 + stop.trailing_zeros() as usize / 8;
                        return base + lane - start;
                    }
                }
                base += LANES;
            }
        }
        // Each bound spread over a word's lanes, built once for the whole run
        // rather than once per word: the multiplies cost more than the
        // comparisons they serve.
        let mut bounds = [(0u64, 0u64); ATOM_CLASS_RANGES as usize];
        for (bound, &(lo, hi)) in bounds.iter_mut().zip(ranges) {
            *bound = (u64::from(lo) * ONES, u64::from(hi) * ONES);
        }
        let bounds = &bounds[..ranges.len()];
        while base + 8 <= end {
            let word = u64::from_le_bytes(bytes[base..base + 8].try_into().unwrap());
            let mut inside = 0;
            for &(lower, upper) in bounds {
                inside |= Self::inside_lanes(word, lower, upper);
            }
            let rejected = if negated { inside } else { !inside & HIGH };
            if rejected != 0 {
                return base - start + rejected.trailing_zeros() as usize / 8;
            }
            base += 8;
        }
        while base < end
            && (ranges
                .iter()
                .any(|&(lo, hi)| bytes[base] >= lo && bytes[base] <= hi)
                != negated)
        {
            base += 1;
        }
        base - start
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
            if let Some((per, accepts)) = self.fixed_atom_charge(op, a, b)
                && let Some(bytes) = self.input.ascii_bytes()
                && !self.state.reverse
            {
                let start = self.cursor.position();
                // A character of most atoms charges exactly one, where the
                // division below is the identity. Naming that case keeps two
                // divisions out of the entry cost of every run.
                let limit = if per == 1 {
                    available.min(self.budget.remaining())
                } else {
                    (available / per).min(self.budget.remaining() / per)
                };
                let run = Self::byte_run(bytes, start, accepts, limit);
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
                // The same test the run and byte scans use, so every path
                // through a repeated class charges it identically.
                CLASS if (b & !NEGATED) <= ATOM_CLASS_RANGES => {
                    match self.atom_holds(op, a, b, c) {
                        Ok(held) => held,
                        Err(error) => {
                            // Resume through the phase, which re-tests the class
                            // with the character already read.
                            if matches!(error, ExecError::WorkLimit) {
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
                            }
                            return Err(error);
                        }
                    }
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
        let [next, value, third] = self.program.instruction(r[4] as usize);
        if matches!(next, CHAR | CHAR_I) {
            return self.retreat_literal(atom.minimum_end, value, next == CHAR_I, available);
        }
        // The continuation's own first consumed character is just as much a
        // necessary condition as a literal's, so an endpoint it cannot start at
        // needs no retry frame either. A repeat that can match nothing consumes
        // no character and so states no condition.
        let probe = if next == ATOM_REPEAT {
            let inner = self.program.repeat(value as usize);
            let body = r[4] as usize + 3;
            (inner[0] >= 1 && body < self.program.instructions())
                .then(|| self.program.instruction(body))
        } else {
            Some([next, value, third])
        };
        if let Some([op, a, b]) = probe
            && self.inline_atom(op, b)
        {
            return self.retreat_probe(atom.minimum_end, op, a, b, available);
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

    /// The same retreat as `retreat_literal`, for a continuation whose first
    /// consumed character is a class or `.` rather than a literal.
    #[inline(never)]
    fn retreat_probe(
        &mut self,
        minimum_end: usize,
        op: u32,
        a: u32,
        b: u32,
        available: usize,
    ) -> Result<(), ExecError> {
        let initial = self.budget.remaining();
        let limit = available.min(256);
        // Over ASCII storage the endpoints this walks back through are bytes,
        // so the continuation's own condition is a byte test. Decoding each one
        // through the cursor costs several times that, and this walk is what a
        // failed greedy repeat spends most of its time in.
        let direct = (!self.state.reverse)
            .then(|| self.fixed_atom_charge(op, a, b))
            .flatten()
            .zip(self.input.ascii_bytes());
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
            let possible = if let Some(((per, accepts), bytes)) = direct {
                self.charge(per - 1)?;
                bytes.get(position).is_some_and(|&byte| accepts.holds(byte))
            } else {
                let mut ahead = self.cursor;
                match read(&mut ahead, self.program.unicode(), self.state.reverse) {
                    Some(c) => self.atom_holds(op, a, b, c)?,
                    None => false,
                }
            };
            if possible || position == minimum_end {
                self.state.phase = Phase::AtomCommit;
                return Ok(());
            }
            if initial - self.budget.remaining() >= limit {
                return Ok(());
            }
        }
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
