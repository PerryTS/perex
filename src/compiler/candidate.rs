//! Conservative first/last-consumed ASCII ranges. The last range is useful only
//! when every successful path consumes input and ends at the subject boundary.
//! Reuse emitted AST slots; no extra arena or second matching program is needed.
use super::*;
use crate::casefold;

#[derive(Clone, Copy)]
struct First {
    lo: u32,
    hi: u32,
    nullable: bool,
}

#[derive(Clone, Copy)]
struct Last {
    range: First,
    consumes: bool,
    anchored: bool,
}
impl First {
    const EMPTY: Self = Self {
        lo: 128,
        hi: 0,
        nullable: false,
    };
    fn union(self, other: Self) -> Self {
        Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
            nullable: self.nullable || other.nullable,
        }
    }
    fn include(&mut self, lo: u32, hi: u32) {
        if lo <= hi && lo < 128 {
            self.lo = self.lo.min(lo);
            self.hi = self.hi.max(hi.min(127));
        }
    }
}
impl Prepared<'_> {
    fn first(&self, id: u32) -> First {
        let n = self.nodes[id as usize];
        First {
            lo: n.first,
            hi: n.end,
            nullable: n.reverse,
        }
    }
    /// The packed maximum match length of a node, in UTF-16 units. `None` is
    /// unbounded or not derived; the stored value is one more than the length
    /// so that zero can mean unbounded.
    fn max_units(&self, id: u32) -> Option<u32> {
        match self.nodes[id as usize].len >> 2 {
            0 => None,
            packed => Some(packed - 1),
        }
    }

    fn last(&self, id: u32) -> Last {
        let n = self.nodes[id as usize];
        Last {
            range: First {
                lo: n.c,
                hi: n.start,
                nullable: n.reverse,
            },
            consumes: n.len & 1 != 0,
            anchored: n.len & 2 != 0,
        }
    }
    pub(super) fn candidate(&mut self) -> Result<(), CompileError> {
        for i in 0..self.used {
            self.step()?;
            let n = self.nodes[i];
            let mut first = First::EMPTY;
            match n.kind {
                CHAR => {
                    if n.flags & I != 0 {
                        if let Some((lo, hi)) = casefold::ascii_bounds(n.a, n.flags & U != 0) {
                            first.include(lo, hi);
                        }
                    } else {
                        first.include(n.a, n.a);
                    }
                }
                ANY => first.include(0, 127),
                CLASS => {
                    let end = n.a + (n.b & !NEGATED);
                    if n.flags & (I | U) != I | U {
                        let mut members = 0u128;
                        for index in n.a..end {
                            self.step()?;
                            let r = self.ranges[index as usize];
                            if r.lo & PROPERTY == 0 {
                                if r.lo < 128 {
                                    members |=
                                        (u128::MAX << r.lo) & (u128::MAX >> (127 - r.hi.min(127)));
                                }
                            } else {
                                let property = properties::ascii(r.lo & !PROPERTY);
                                members |= if r.hi != 0 { !property } else { property };
                            }
                        }
                        if n.flags & I != 0 {
                            // Legacy ASCII equivalence is only A-Z <-> a-z;
                            // Unicode's non-ASCII equivalents use the path below.
                            const UPPER: u128 = ((1u128 << 26) - 1) << 65;
                            members |= ((members & UPPER) << 32) | ((members >> 32) & UPPER);
                        }
                        if n.b & NEGATED != 0 {
                            members = !members;
                        }
                        if members != 0 {
                            first.include(members.trailing_zeros(), 127 - members.leading_zeros());
                        }
                    } else {
                        for c in 0..128 {
                            let values = if n.flags & I != 0 {
                                casefold::equivalents(c, n.flags & U != 0)
                            } else {
                                [c; 4]
                            };
                            let mut found = false;
                            for index in n.a..end {
                                self.step()?;
                                let r = self.ranges[index as usize];
                                // A `v` complement negates membership of the
                                // whole closure, matching the evaluator.
                                found = if r.lo & PROPERTY != 0 && r.hi == 2 {
                                    !values
                                        .iter()
                                        .any(|&v| properties::contains(r.lo & !PROPERTY, v))
                                } else {
                                    values.iter().any(|&v| {
                                        if r.lo & PROPERTY != 0 {
                                            properties::contains(r.lo & !PROPERTY, v) != (r.hi != 0)
                                        } else {
                                            v >= r.lo && v <= r.hi
                                        }
                                    })
                                };
                                if found {
                                    break;
                                }
                            }
                            if found != (n.b & NEGATED != 0) {
                                first.include(c, c);
                            }
                        }
                    }
                }
                GROUP | WRAP => first = self.first(n.a),
                REPEAT => {
                    first = self.first(n.a);
                    first.nullable |= n.b == 0;
                }
                ALT => first = self.first(n.a).union(self.first(n.b)),
                SEQ => {
                    first = self.first(n.a);
                    if first.nullable {
                        let right = self.first(n.b);
                        first = first.union(right);
                        first.nullable = right.nullable;
                    }
                }
                BACKREF | NAMED_BACKREF => {
                    first.include(0, 127);
                    first.nullable = true;
                }
                _ => first.nullable = true,
            }
            let mut last = Last {
                range: first,
                consumes: matches!(n.kind, CHAR | ANY | CLASS | BACKREF | NAMED_BACKREF),
                anchored: n.kind == END && n.flags & M == 0,
            };
            match n.kind {
                GROUP | WRAP => last = self.last(n.a),
                REPEAT => {
                    last = self.last(n.a);
                    last.anchored &= n.b != 0;
                }
                ALT => {
                    let (left, right) = (self.last(n.a), self.last(n.b));
                    last.range = left.range.union(right.range);
                    last.consumes = left.consumes || right.consumes;
                    last.anchored = left.anchored && right.anchored;
                }
                SEQ => {
                    let (left, right) = (self.last(n.a), self.last(n.b));
                    last.range = right.range;
                    if right.range.nullable {
                        last.range = last.range.union(left.range);
                    }
                    last.consumes = left.consumes || right.consumes;
                    // A nullable suffix can still consume input. Only a suffix
                    // that cannot consume preserves an earlier end assertion.
                    last.anchored = right.anchored || (left.anchored && !right.consumes);
                }
                _ => {}
            }
            self.nodes[i].first = first.lo;
            self.nodes[i].end = first.hi;
            self.nodes[i].reverse = first.nullable;
            // All instruction/table emission and required-text admission have
            // finished. Repeat maxima, starts and lengths are now dead fields.
            // The longest a node can match, in UTF-16 units. A repetition or a
            // backreference has no bound this pass can establish, so it
            // disables the claim rather than guessing one.
            let max = match n.kind {
                CHAR => Some(if n.a >= 0x10000 { 2 } else { 1 }),
                ANY | CLASS => Some(2),
                GROUP | WRAP => self.max_units(n.a),
                SEQ => self
                    .max_units(n.a)
                    .zip(self.max_units(n.b))
                    .map(|(l, r)| l + r),
                ALT => self
                    .max_units(n.a)
                    .zip(self.max_units(n.b))
                    .map(|(l, r)| l.max(r)),
                // An assertion consumes nothing, whatever it examines.
                ASSERT | START | END | WORD | EMPTY => Some(0),
                _ => None,
            };
            self.nodes[i].c = last.range.lo;
            self.nodes[i].start = last.range.hi;
            self.nodes[i].len = u32::from(last.consumes)
                | (u32::from(last.anchored) << 1)
                | match max {
                    Some(units) if units < MAX_END_UNITS => (units + 1) << 2,
                    _ => 0,
                };
        }
        Ok(())
    }
    pub(super) fn candidate_descriptor(&self, root: u32) -> u32 {
        Self::range_descriptor(self.first(root))
    }
    pub(super) fn end_candidate_descriptor(&self, root: u32) -> u32 {
        let last = self.last(root);
        if !last.anchored {
            return 0;
        }
        // Every successful match ends at the subject's end and consumes at most
        // this many units, so no match can begin before that far from the end.
        let bound = match self.max_units(root) {
            Some(units) if units < MAX_END_UNITS => (units + 1) << 24,
            _ => 0,
        };
        Self::range_descriptor(last.range) | bound
    }
    fn range_descriptor(first: First) -> u32 {
        if first.nullable || (first.lo == 0 && first.hi == 127) {
            0
        } else if first.lo > first.hi {
            1
        } else {
            2 | (first.lo << 8) | (first.hi << 16)
        }
    }
}
