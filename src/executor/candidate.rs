//! Skip impossible start positions in original ASCII storage. The descriptor
//! is a necessary first-character range, never a substitute for capture logic.
use super::*;

impl Vm<'_, '_, '_, '_> {
    pub(super) fn start_candidate(&mut self) {
        self.state.phase = if self.program.words[7] != 0 && self.input.ascii_bytes().is_some() {
            Phase::Candidate
        } else {
            Phase::Initialize(0)
        };
    }
    pub(super) fn candidate_step(&mut self, available: usize) -> Result<(), ExecError> {
        let bytes = self.input.ascii_bytes().ok_or(ExecError::InvalidProgram)?;
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
        let count = (bytes.len() - start)
            .min(available.min(limit))
            .min(self.budget.remaining());
        if count == 0 {
            self.charge(1)?;
            return Err(ExecError::InvalidProgram);
        }
        // Bound every memory read by the available operation work. Charge logical
        // positions through the first candidate before publishing its mark;
        // speculative word lanes do not change work across different quanta.
        let (found, inspected) = first_in_range(&bytes[start..start + count], lo, hi);
        self.charge(inspected)?;
        if let Some(index) = found {
            let at = start + index;
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
}

fn first_in_range(bytes: &[u8], lo: u8, hi: u8) -> (Option<usize>, usize) {
    if bytes[0] >= lo && bytes[0] <= hi {
        return (Some(0), 1);
    }
    let mut i = 1;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    const ONES: u64 = 0x0101_0101_0101_0101;
    let lower = u64::from(lo) * ONES;
    let upper = u64::from(hi) * ONES;
    while i + 8 <= bytes.len() {
        let word = u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
        // ASCII lanes are <=127. Setting the minuend's high bit prevents
        // inter-lane borrows in both subtractions, so every lane is exact.
        let mask = ((word | HIGH).wrapping_sub(lower)) & ((upper | HIGH).wrapping_sub(word)) & HIGH;
        if mask != 0 {
            let at = i + mask.trailing_zeros() as usize / 8;
            return (Some(at), at + 1);
        }
        i += 8;
    }
    while i < bytes.len() {
        if bytes[i] >= lo && bytes[i] <= hi {
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
    fn ascii_range_words_match_scalar_lanes() {
        for lo in 0..128u8 {
            for hi in lo..128 {
                for position in 0..17 {
                    let mut bytes = [0u8; 17];
                    for (i, b) in bytes.iter_mut().enumerate() {
                        *b = ((i * 37 + position * 13) % 128) as u8;
                    }
                    let want = bytes.iter().position(|&b| b >= lo && b <= hi);
                    let (got, inspected) = first_in_range(&bytes, lo, hi);
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
