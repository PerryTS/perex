//! Skip impossible ASCII start positions in original byte storage. The descriptor
//! is a necessary first-character range, never a substitute for capture logic.
use super::*;

impl Vm<'_, '_, '_, '_> {
    pub(super) fn end_candidate(&mut self) -> Result<bool, ExecError> {
        // The upper byte is the length bound, not part of this descriptor.
        let descriptor = self.program.words[8] & 0xff_ffff;
        if descriptor == 0 {
            return Ok(true);
        }
        self.charge(1)?;
        // The endpoint is O(1) in every representation. Decode only the last
        // original unit; retain neither a new subject nor an interior pointer.
        let mut end = self
            .input
            .cursor_at(self.input.len_utf16())
            .ok_or(ExecError::InvalidProgram)?;
        let Some(unit) = end.previous_unit() else {
            return Ok(false);
        };
        // Non-ASCII, including either surrogate half, always reaches the VM.
        Ok(unit >= 128
            || (descriptor != 1
                && u32::from(unit) >= (descriptor >> 8) & 255
                && u32::from(unit) <= (descriptor >> 16) & 255))
    }

    pub(super) fn start_candidate(&mut self) {
        self.state.phase = if self.program.words[7] != 0 && self.input.original_bytes().is_some() {
            Phase::Candidate
        } else {
            Phase::Initialize(0)
        };
    }
    /// The first byte of every branch of the entry alternation. The derivation
    /// proved each branch begins with at least two ASCII characters, so each
    /// contributes exactly one byte, and there are at most `LEADING_BRANCHES`.
    fn branch_bytes(&self) -> ([u8; LEADING_BRANCHES], usize) {
        let mut first = [0; LEADING_BRANCHES];
        let mut count = 0;
        let mut pending = [0usize; LEADING_BRANCHES];
        let mut depth = 0;
        let mut pc = self.program.leading_pc();
        loop {
            let [op, a, b] = self.program.instruction(pc);
            if op == SPLIT {
                if depth == pending.len() {
                    return (first, 0);
                }
                pending[depth] = b as usize;
                depth += 1;
                pc = a as usize;
                continue;
            }
            if count == first.len() {
                return (first, 0);
            }
            first[count] = self.program.instruction(pc)[1] as u8;
            count += 1;
            if depth == 0 {
                return (first, count);
            }
            depth -= 1;
            pc = pending[depth];
        }
    }

    /// Compare the entry alternation's branches against original bytes at `at`,
    /// succeeding as soon as one matches. The branches are read from the
    /// instructions, so this needs no program storage and no match state.
    fn alternation_matches(&mut self, bytes: &[u8], at: usize) -> Result<bool, ExecError> {
        let mut pending = [0usize; LEADING_BRANCHES];
        let mut depth = 0;
        let mut pc = self.program.leading_pc();
        loop {
            self.charge(1)?;
            let [op, a, b] = self.program.instruction(pc);
            if op == SPLIT {
                // The derivation proved the branch count fits.
                if depth == pending.len() {
                    return Ok(true);
                }
                pending[depth] = b as usize;
                depth += 1;
                pc = a as usize;
                continue;
            }
            let mut matched = true;
            let mut next = pc;
            while self.program.instruction(next)[0] == CHAR {
                self.charge(1)?;
                let want = self.program.instruction(next)[1];
                match bytes.get(at + (next - pc)) {
                    Some(&byte) if u32::from(byte) == want => next += 1,
                    _ => {
                        matched = false;
                        break;
                    }
                }
            }
            if matched {
                return Ok(true);
            }
            if depth == 0 {
                return Ok(false);
            }
            depth -= 1;
            pc = pending[depth];
        }
    }

    /// Compare the program's leading literal against original bytes at `at`.
    /// `None` reports that the literal cannot fit before the subject's end.
    /// Every compared position is charged, so this cannot outrun its budget.
    fn leading_matches(
        &mut self,
        bytes: &[u8],
        at: usize,
        leading: usize,
    ) -> Result<Option<bool>, ExecError> {
        if leading == LEADING_ALTERNATION as usize {
            return self.alternation_matches(bytes, at).map(Some);
        }
        let Some(window) = bytes.get(at..at + leading) else {
            self.charge(1)?;
            return Ok(None);
        };
        self.charge(leading)?;
        let pc = self.program.leading_pc();
        // `ascii_bytes` is `Some` only for wholly ASCII storage, so an
        // ASCII-insensitive comparison is exact for a folded run there.
        let fold = self.program.leading_fold();
        for (i, &byte) in window.iter().enumerate() {
            let want = self.program.instruction(pc + i)[1];
            let same = if fold {
                byte.eq_ignore_ascii_case(&(want as u8))
            } else {
                u32::from(byte) == want
            };
            if !same {
                return Ok(Some(false));
            }
        }
        Ok(Some(true))
    }

    pub(super) fn candidate_step(&mut self, available: usize) -> Result<(), ExecError> {
        let Some(bytes) = self.input.ascii_bytes() else {
            return self.mixed_candidate_step(available);
        };
        let start = self.cursor.position();
        let descriptor = self.program.words[7];
        if descriptor == 1 || start == bytes.len() {
            self.charge(1)?;
            self.state.phase = Phase::Finished(false);
            return Ok(());
        }
        let lo = (descriptor >> 8) as u8;
        let hi = (descriptor >> 16) as u8;
        let limit = if self.program.words[2] & Y != 0 {
            1
        } else {
            256
        };
        let mut count = (bytes.len() - start)
            .min(available.min(limit))
            .min(self.budget.remaining());
        if count == 0 {
            self.charge(1)?;
            return Err(ExecError::InvalidProgram);
        }
        // Every successful match consumes the admission condition at or after
        // its own start, so a start with no later occurrence cannot match, and
        // neither can any start after it. Keep the scan inside the last known
        // occurrence; past it, resume the condition search before continuing.
        // That search only moves forward, so the whole bound costs one pass.
        if self.program.admission_forward() && self.state.required_at != UNSET {
            if start > self.state.required_at {
                self.charge(1)?;
                self.state.phase = Phase::BoundPrepare {
                    from: self.state.required_from,
                };
                return Ok(());
            }
            count = count.min(self.state.required_at + 1 - start);
        }
        // Bound every memory read by the available operation work. Charge logical
        // positions through the first candidate before publishing its mark;
        // speculative word lanes do not change work across different quanta.
        //
        // A leading literal lets an impossible start be rejected here instead of
        // by an initialized trial, which costs registers, a frame and the
        // instruction loop. Only positions the descriptor already admits are
        // examined, so this removes work without reaching a new one.
        // The ASCII path already proves the storage is wholly ASCII, which is
        // what a folded comparison needs.
        let leading = self.program.leading();
        // An alternation scans the exact set of branch starts rather than the
        // widened interval the descriptor holds, which on ordinary text stops
        // at a small fraction of the positions the interval does.
        let (set, members) = if leading == LEADING_ALTERNATION as usize {
            self.branch_bytes()
        } else {
            ([0; LEADING_BRANCHES], 0)
        };
        let mut scanned = 0;
        let found = loop {
            let window = &bytes[start + scanned..start + count];
            let (found, inspected) = if members != 0 {
                first_in_set(window, &set[..members])
            } else {
                first_in_range::<false, false>(window, lo, hi)
            };
            self.charge(inspected)?;
            let Some(index) = found else { break None };
            let at = start + scanned + index;
            if leading == 0 {
                break Some(at);
            }
            match self.leading_matches(bytes, at, leading)? {
                Some(true) => break Some(at),
                // The literal cannot fit before the subject's end, so it cannot
                // fit at any later start either.
                None => {
                    self.state.phase = Phase::Finished(false);
                    return Ok(());
                }
                Some(false) => {}
            }
            scanned += index + 1;
            if scanned >= count {
                break None;
            }
        };
        if let Some(at) = found {
            if self.program.words[2] & Y != 0 && at != self.state.requested_start {
                self.state.phase = Phase::Finished(false);
            } else {
                self.cursor = self.input.cursor_at(at).ok_or(ExecError::InvalidProgram)?;
                self.state.start = self.cursor.mark();
                self.state.phase = Phase::Initialize(0);
            }
        } else if self.program.words[2] & Y != 0 || start + count == bytes.len() {
            self.state.phase = Phase::Finished(false);
        } else {
            self.cursor = self
                .input
                .cursor_at(start + count)
                .ok_or(ExecError::InvalidProgram)?;
        }
        Ok(())
    }

    // Keep mixed decoding/scanning out of the common instruction loop's body.
    #[inline(never)]
    fn mixed_candidate_step(&mut self, available: usize) -> Result<(), ExecError> {
        let Some(bytes) = self.cursor.byte_tail() else {
            // Half-pair positions are ordinary potential starts. No byte scan
            // is allowed to round the position past either surrogate unit.
            self.charge(1)?;
            self.state.phase = Phase::Initialize(0);
            return Ok(());
        };
        if bytes.is_empty() {
            self.charge(1)?;
            self.state.phase = Phase::Finished(false);
            return Ok(());
        }
        let descriptor = self.program.words[7];
        let (lo, hi) = if descriptor == 1 {
            // No ASCII member is possible, but every non-ASCII lead byte must
            // still go through the ordinary matcher.
            (128, 127)
        } else {
            ((descriptor >> 8) as u8, (descriptor >> 16) as u8)
        };
        let sticky = self.program.words[2] & Y != 0;
        let count = bytes
            .len()
            .min(available.min(if sticky { 1 } else { 256 }))
            .min(self.budget.remaining());
        if count == 0 {
            self.charge(1)?;
            return Err(ExecError::InvalidProgram);
        }
        let (found, inspected) = first_in_range::<true, true>(&bytes[..count], lo, hi);
        self.charge(inspected)?;
        if let Some(index) = found {
            if !self.cursor.skip_ascii(index) {
                return Err(ExecError::InvalidProgram);
            }
            self.state.start = self.cursor.mark();
            self.state.phase = Phase::Initialize(0);
        } else if sticky || count == bytes.len() {
            self.state.phase = Phase::Finished(false);
        } else if !self.cursor.skip_ascii(count) {
            return Err(ExecError::InvalidProgram);
        }
        Ok(())
    }
}

/// The first position holding any byte of `set`, and the positions inspected.
///
/// A union of first characters is widened into one interval for the word 7
/// descriptor, so an alternation's range admits far more than its branches do.
/// Scanning the exact set instead stops only where a branch can begin. The set
/// is small, so one word-parallel pass per member still costs a fraction of a
/// comparison per byte.
pub(super) fn first_in_set(bytes: &[u8], set: &[u8]) -> (Option<usize>, usize) {
    const HIGH: u64 = 0x8080_8080_8080_8080;
    const ONES: u64 = 0x0101_0101_0101_0101;
    if set.iter().any(|&b| b == bytes[0]) {
        return (Some(0), 1);
    }
    let mut i = 1;
    while i + 8 <= bytes.len() {
        let word = u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
        let mut mask = 0;
        for &b in set {
            // A lane equal to `b` is the only one whose difference can borrow
            // into its high bit while that bit stays clear in the operand.
            let x = word ^ (u64::from(b) * ONES);
            mask |= x.wrapping_sub(ONES) & !x & HIGH;
        }
        if mask != 0 {
            let at = i + mask.trailing_zeros() as usize / 8;
            return (Some(at), at + 1);
        }
        i += 8;
    }
    while i < bytes.len() {
        if set.iter().any(|&b| b == bytes[i]) {
            return (Some(i), i + 1);
        }
        i += 1;
    }
    (None, bytes.len())
}

pub(super) fn first_in_range<const MIXED: bool, const STOP_NON_ASCII: bool>(
    bytes: &[u8],
    lo: u8,
    hi: u8,
) -> (Option<usize>, usize) {
    if (bytes[0] >= lo && bytes[0] <= hi) || (STOP_NON_ASCII && bytes[0] >= 128) {
        return (Some(0), 1);
    }
    let mut i = 1;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    const ONES: u64 = 0x0101_0101_0101_0101;
    let lower = u64::from(lo) * ONES;
    let upper = u64::from(hi) * ONES;
    while i + 8 <= bytes.len() {
        let word = u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
        let lanes = if MIXED { word & !HIGH } else { word };
        // ASCII lanes are <=127. Setting the minuend's high bit prevents
        // inter-lane borrows in both subtractions, so every lane is exact.
        let range =
            ((lanes | HIGH).wrapping_sub(lower)) & ((upper | HIGH).wrapping_sub(lanes)) & HIGH;
        let mask = if STOP_NON_ASCII {
            range | (word & HIGH)
        } else if MIXED {
            // Admission seeks an ASCII member, so UTF-8/WTF-8 high-byte lanes
            // cannot qualify. Masking them before subtraction prevents borrows
            // from changing an adjacent lane's comparison.
            range & !word
        } else {
            range
        };
        if mask != 0 {
            let at = i + mask.trailing_zeros() as usize / 8;
            return (Some(at), at + 1);
        }
        i += 8;
    }
    while i < bytes.len() {
        if (bytes[i] >= lo && bytes[i] <= hi) || (STOP_NON_ASCII && bytes[i] >= 128) {
            return (Some(i), i + 1);
        }
        i += 1;
    }
    (None, bytes.len())
}

#[cfg(test)]
mod tests {
    use super::first_in_range;
    #[test]
    fn mixed_word_scan_stops_at_every_non_ascii_lane() {
        for (lo, hi) in [
            (0, 0),
            (0, 127),
            (65, 90),
            (110, 110),
            (127, 127),
            (128, 127),
        ] {
            for byte in 0..=255u8 {
                for position in 0..33 {
                    let mut bytes = [b'x'; 33];
                    bytes[position] = byte;
                    let want = bytes.iter().position(|&b| b >= 128 || (b >= lo && b <= hi));
                    let (got, inspected) = first_in_range::<true, true>(&bytes, lo, hi);
                    assert_eq!(got, want, "{lo}..{hi}, byte {byte}, position {position}");
                    assert_eq!(inspected, want.map_or(bytes.len(), |at| at + 1));
                }
            }
        }
    }
    #[test]
    fn mixed_range_membership_rejects_high_bytes_without_cross_lane_borrows() {
        for (lo, hi) in [(0, 0), (0, 127), (65, 90), (97, 102), (127, 127)] {
            for byte in 0..=255u8 {
                for position in 0..33 {
                    let mut bytes = [0u8; 33];
                    for (i, b) in bytes.iter_mut().enumerate() {
                        *b = (i * 73 + usize::from(byte) * 19) as u8;
                    }
                    bytes[position] = byte;
                    let want = bytes.iter().position(|&b| b >= lo && b <= hi);
                    let (got, inspected) = first_in_range::<true, false>(&bytes, lo, hi);
                    assert_eq!(got, want, "{lo}..{hi}, byte {byte}, position {position}");
                    assert_eq!(inspected, want.map_or(bytes.len(), |at| at + 1));
                }
            }
        }
    }
    #[test]
    fn ascii_range_words_match_scalar_lanes() {
        for lo in 0..128u8 {
            for hi in lo..128 {
                for position in 0..17 {
                    let mut bytes = [0u8; 17];
                    for (i, b) in bytes.iter_mut().enumerate() {
                        *b = ((i * 37 + position * 13) % 128) as u8;
                    }
                    let want = bytes.iter().position(|&b| b >= lo && b <= hi);
                    let (got, inspected) = first_in_range::<false, false>(&bytes, lo, hi);
                    assert_eq!(got, want, "range {lo}..{hi}, rotation {position}");
                    assert!((1..=bytes.len()).contains(&inspected));
                    if let Some(at) = got {
                        assert!(at < inspected);
                    }
                }
            }
        }
    }
}
