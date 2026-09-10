//! Select one condition necessary for every successful match. Emission is done,
//! so AST slots can be reused without growing caller scratch or the program.
use super::*;
use core::cmp::Ordering;

impl Parser<'_, '_> {
    fn compare_admission(
        &mut self,
        p: Program<'_>,
        a: u32,
        b: u32,
    ) -> Result<Ordering, CompileError> {
        let (left, right) = (self.nodes[a as usize - 1], self.nodes[b as usize - 1]);
        let score = left.end.cmp(&right.end);
        if score != Ordering::Equal {
            return Ok(score);
        }
        let (lpc, rpc) = (left.start as usize, right.start as usize);
        let (li, ri) = (p.instruction(lpc), p.instruction(rpc));
        let kind = li[0].cmp(&ri[0]);
        if kind != Ordering::Equal {
            return Ok(kind);
        }
        if li[0] == CHAR {
            let len = ADMISSION_MAX - left.end as usize;
            for i in 0..len {
                self.step()?;
                let l = p.instruction(lpc + if left.reverse { len - 1 - i } else { i })[1];
                let r = p.instruction(rpc + if right.reverse { len - 1 - i } else { i })[1];
                let order = l.cmp(&r);
                if order != Ordering::Equal {
                    return Ok(order);
                }
            }
        } else {
            let shape = li[2].cmp(&ri[2]);
            if shape != Ordering::Equal {
                return Ok(shape);
            }
            for i in 0..li[2] as usize {
                self.step()?;
                let order = p
                    .range(li[1] as usize + i)
                    .cmp(&p.range(ri[1] as usize + i));
                if order != Ordering::Equal {
                    return Ok(order);
                }
            }
        }
        Ok(Ordering::Equal)
    }

    pub(super) fn admission(&mut self, p: Program<'_>, root: u32) -> Result<u32, CompileError> {
        let (mut literal_start, mut literal_end) = (0, 0);
        for i in 0..self.used {
            self.step()?;
            let n = self.nodes[i];
            let candidate = match n.kind {
                CHAR => {
                    let pc = n.start as usize;
                    if pc < literal_start || pc >= literal_end {
                        // Cache the entire instruction run, including reverse
                        // bodies. Rescanning up to 32 successors for every
                        // literal made long-pattern compilation unnecessarily
                        // expensive. AST literal leaves visit each run together.
                        literal_start = pc;
                        literal_end = pc + 1;
                        while literal_start > 0 && p.instruction(literal_start - 1)[0] == CHAR {
                            self.step()?;
                            literal_start -= 1;
                        }
                        while literal_end < p.instructions()
                            && p.instruction(literal_end)[0] == CHAR
                        {
                            self.step()?;
                            literal_end += 1;
                        }
                    }
                    let len = (literal_end - pc).min(ADMISSION_MAX);
                    self.nodes[i].end = (ADMISSION_MAX - len) as u32;
                    i as u32 + 1
                }
                CLASS if n.b & NEGATED == 0 => {
                    let mut cardinality = 0u32;
                    for r in n.a..n.a + n.b {
                        self.step()?;
                        let [lo, hi] = p.range(r as usize);
                        if lo & PROPERTY != 0 {
                            cardinality = 257;
                            break;
                        }
                        cardinality = cardinality.saturating_add(hi - lo + 1);
                        if cardinality > 256 {
                            break;
                        }
                    }
                    if cardinality <= 256 {
                        // A heuristic rank only; actual membership reuses VM
                        // semantics. Overlapping ranges need not be normalized.
                        self.nodes[i].end = ADMISSION_MAX as u32 + cardinality;
                        i as u32 + 1
                    } else {
                        0
                    }
                }
                GROUP | WRAP => self.nodes[n.a as usize].c,
                REPEAT if n.b > 0 => self.nodes[n.a as usize].c,
                ASSERT if n.flags & 1 == 0 => self.nodes[n.a as usize].c,
                SEQ | ALT => {
                    let (a, b) = (self.nodes[n.a as usize].c, self.nodes[n.b as usize].c);
                    if n.kind == ALT {
                        if a != 0 && b != 0 && self.compare_admission(p, a, b)? == Ordering::Equal {
                            a
                        } else {
                            0
                        }
                    } else if a == 0 {
                        b
                    } else if b == 0 {
                        a
                    } else {
                        let (left, right) =
                            (self.nodes[a as usize - 1], self.nodes[b as usize - 1]);
                        // A sequence does not need lexical equality: either
                        // condition is necessary. Prefer the later literal on
                        // a rank tie (a+z should check z). Keep canonical class
                        // ranking so reordered alternatives share conditions.
                        let order = if left.kind == CHAR && right.kind == CHAR {
                            left.end.cmp(&right.end).then(Ordering::Greater)
                        } else {
                            self.compare_admission(p, a, b)?
                        };
                        if order == Ordering::Greater { b } else { a }
                    }
                }
                _ => 0,
            };
            // c/end no longer hold repeat maxima/capture ends after emission.
            self.nodes[i].c = candidate;
        }
        let id = self.nodes[root as usize].c;
        if id == 0 {
            return Ok(0);
        }
        let n = self.nodes[id as usize - 1];
        if n.start >= 1 << 24 {
            return Ok(0);
        } // Omit the optimization, not the pattern.
        Ok(ADMISSION | (n.start << 8) | if n.reverse { ADMISSION_REVERSE } else { 0 })
    }
}
