//! Conservative first-consumed ASCII range. Zero-width operations contribute
//! no characters; nullable prefixes union the following child's possibilities.
//! Reuse the emitted AST's capture bounds/direction, never allocate a new arena.
use super::*;
use crate::casefold;

#[derive(Clone, Copy)]
struct First {
    lo: u32,
    hi: u32,
    nullable: bool,
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
                                found = values.iter().any(|&v| {
                                    if r.lo & PROPERTY != 0 {
                                        properties::contains(r.lo & !PROPERTY, v) != (r.hi != 0)
                                    } else {
                                        v >= r.lo && v <= r.hi
                                    }
                                });
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
            self.nodes[i].first = first.lo;
            self.nodes[i].end = first.hi;
            self.nodes[i].reverse = first.nullable;
        }
        Ok(())
    }
    pub(super) fn candidate_descriptor(&self, root: u32) -> u32 {
        let first = self.first(root);
        if first.nullable || (first.lo == 0 && first.hi == 127) {
            0
        } else if first.lo > first.hi {
            1
        } else {
            2 | (first.lo << 8) | (first.hi << 16)
        }
    }
}
