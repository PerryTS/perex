//! Normalize large literal-only classes in caller scratch. Properties retain
//! their existing representation and membership rules. No extra arena is used.
use super::*;

fn charge(budget: &mut Budget) -> Result<(), CompileError> {
    budget.charge(1).map_err(|_| CompileError::WorkLimit)
}

fn sift(
    ranges: &mut [Range],
    mut root: usize,
    end: usize,
    budget: &mut Budget,
) -> Result<(), CompileError> {
    // A heap node has children only in this first half; the guard also bounds
    // the multiplication on targets whose usize is no wider than u32.
    while root < end / 2 {
        let mut child = root * 2 + 1;
        if child + 1 < end {
            charge(budget)?;
            if (ranges[child].lo, ranges[child].hi) < (ranges[child + 1].lo, ranges[child + 1].hi) {
                child += 1;
            }
        }
        charge(budget)?;
        if (ranges[root].lo, ranges[root].hi) >= (ranges[child].lo, ranges[child].hi) {
            break;
        }
        ranges.swap(root, child);
        root = child;
    }
    Ok(())
}

impl Parser<'_, '_> {
    pub(super) fn normalize_class(
        &mut self,
        start: usize,
        count: usize,
    ) -> Result<Option<usize>, CompileError> {
        if count < 16 {
            return Ok(None);
        }
        // Class members are appended together. Name characters occupy the
        // opposite end of the arena and are outside this live range slice.
        if start.checked_add(count) != Some(self.range_used) {
            return Err(CompileError::InvalidProgram);
        }
        let ranges = &mut self.ranges[start..self.range_used];
        let mut sorted = true;
        let mut previous = None;
        for range in ranges.iter() {
            charge(self.budget)?;
            if range.lo & PROPERTY != 0 {
                return Ok(None);
            }
            let key = (range.lo, range.hi);
            sorted &= previous.is_none_or(|old| old <= key);
            previous = Some(key);
        }
        // In-place heapsort has bounded O(n log n) comparisons and no native
        // recursion. Every comparison and merge pays compilation work.
        if !sorted {
            for root in (0..count / 2).rev() {
                sift(ranges, root, count, self.budget)?;
            }
            for end in (1..count).rev() {
                ranges.swap(0, end);
                sift(ranges, 0, end, self.budget)?;
            }
        }
        let mut used = 0;
        for i in 0..count {
            charge(self.budget)?;
            let next = ranges[i];
            if used != 0 && next.lo <= ranges[used - 1].hi.saturating_add(1) {
                ranges[used - 1].hi = ranges[used - 1].hi.max(next.hi);
            } else {
                ranges[used] = next;
                used += 1;
            }
        }
        self.range_used = start + used;
        Ok(Some(used))
    }
}
