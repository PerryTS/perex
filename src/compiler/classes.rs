//! Normalize large literal-only classes in caller scratch. Properties retain
//! their existing representation and membership rules. No extra arena is used.
use super::*;
use crate::casefold;

/// Members a folded class may have and still be replaced by its case closure
/// at compile time. Each member costs one equivalence lookup here, once, in
/// place of one per subject character per test at match time; past this the
/// class keeps folding at match time as before.
const FOLD_CLOSURE_MEMBERS: u32 = 256;

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
    /// Replace a folded class by its case closure: every character some
    /// member is case-equivalent to. The evaluator folds a subject character
    /// `c` into `equivalents(c)` and accepts when any of them is a member.
    /// Case equivalence is symmetric (`casefold::equivalents` walks one cycle
    /// of an equivalence family from any of its members), so that holds
    /// exactly when `c` is in the closure, and the unfolded class over the
    /// closure answers every character identically. Negation applies to the
    /// membership either way, so it is kept as it is.
    ///
    /// Returns the closed class's range count, or `None` when the class keeps
    /// its folded form: it holds a property, or more than
    /// `FOLD_CLOSURE_MEMBERS` characters.
    pub(super) fn close_fold(
        &mut self,
        start: usize,
        count: usize,
    ) -> Result<Option<usize>, CompileError> {
        if count == 0 || start.checked_add(count) != Some(self.range_used) {
            return Ok(None);
        }
        let mut members = 0u32;
        for range in &self.ranges[start..self.range_used] {
            charge(self.budget)?;
            if range.lo & PROPERTY != 0 || range.lo > range.hi {
                return Ok(None);
            }
            members = members.saturating_add(range.hi - range.lo + 1);
            if members > FOLD_CLOSURE_MEMBERS {
                return Ok(None);
            }
        }
        let unicode = self.flags & U != 0;
        let original = start + count;
        for index in start..original {
            let Range { lo, hi } = self.ranges[index];
            for c in lo..=hi {
                charge(self.budget)?;
                for value in casefold::equivalents(c, unicode) {
                    if value == c
                        || self.ranges[start..original]
                            .iter()
                            .any(|r| value >= r.lo && value <= r.hi)
                    {
                        continue;
                    }
                    // Consecutive members fold to consecutive partners, as a
                    // letter range does, so extend the last addition where
                    // that continues it.
                    let last = self.range_used - 1;
                    if self.range_used > original
                        && self.ranges[last].hi.checked_add(1) == Some(value)
                    {
                        self.ranges[last].hi = value;
                    } else {
                        self.range(value, value)?;
                    }
                }
            }
        }
        // Sort by lower bound and merge, so the closure is as few ranges as
        // it can be: `[0-9a-f]` closes to three, not twenty-two.
        let ranges = &mut self.ranges[start..self.range_used];
        for i in 1..ranges.len() {
            let mut j = i;
            while j > 0 && (ranges[j - 1].lo, ranges[j - 1].hi) > (ranges[j].lo, ranges[j].hi) {
                charge(self.budget)?;
                ranges.swap(j - 1, j);
                j -= 1;
            }
        }
        let mut used = 0;
        for i in 0..ranges.len() {
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
