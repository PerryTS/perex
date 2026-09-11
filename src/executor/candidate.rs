//! Skip impossible ASCII start positions in original byte storage. The descriptor
//! is a necessary first-character range, never a substitute for capture logic.
use super::*;

impl Vm<'_, '_, '_, '_> {
    pub(super) fn start_candidate(&mut self) {
        self.state.phase = if self.program.words[7] != 0 && self.input.original_bytes().is_some() {
            Phase::Candidate
        } else {
            Phase::Initialize(0)
        };
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
        let (found, inspected) =
            first_in_range::<false, false>(&bytes[start..start + count], lo, hi);
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
