//! Remove duplicate partitions of a single consuming atom. This preserves the
//! order of distinct end positions, not just the accepted language.
use super::*;

impl Parser<'_, '_> {
    fn unwrapped(&mut self, mut id: u32) -> Result<u32, CompileError> {
        while self.nodes[id as usize].kind == WRAP {
            self.step()?;
            id = self.nodes[id as usize].a;
        }
        Ok(id)
    }

    pub(super) fn merge_repetition(
        &mut self,
        child: u32,
        min: u32,
        max: u32,
        infinite: bool,
    ) -> Result<bool, CompileError> {
        let id = self.unwrapped(child)?;
        let n = self.nodes[id as usize];
        let (body, inner_min, inner_max, inner_infinite) = match n.kind {
            REPEAT if n.flags & 2 == 0 && n.b <= 1 => (n.a, n.b, n.c, n.flags & 1 != 0),
            ALT => {
                let empty = self.unwrapped(n.b)?;
                if self.nodes[empty as usize].kind != EMPTY {
                    return Ok(false);
                }
                (n.a, 0, 1, false)
            }
            _ => return Ok(false),
        };
        let atom = self.unwrapped(body)?;
        if !matches!(self.nodes[atom as usize].kind, CHAR | CLASS | ANY) {
            return Ok(false);
        }
        // For a greedy single-atom interval whose minimum is zero or one,
        // greedy nesting visits each distinct attainable end in descending
        // order. Alternative partitions revisit the same end without changing
        // captures. Larger inner minima can leave gaps AND change that order;
        // captures, assertions, compound bodies and lazy choices are excluded.
        let empty = (!infinite && max == 0) || (!inner_infinite && inner_max == 0);
        let combined_infinite = !empty && (infinite || inner_infinite);
        let combined_max = if empty || combined_infinite {
            0
        } else if let Some(bound) = max.checked_mul(inner_max) {
            bound
        } else {
            return Ok(false); // Preserve the original bounded representation.
        };
        let combined_min = min * inner_min; // inner_min <= 1
        if n.kind == ALT {
            self.repeats = self.repeats.checked_add(1).ok_or(CompileError::SizeLimit)?;
        }
        let len = self.nodes[body as usize]
            .len
            .checked_add(4)
            .ok_or(CompileError::SizeLimit)?;
        self.nodes[id as usize] = Node {
            kind: REPEAT,
            a: body,
            b: combined_min,
            c: combined_max,
            flags: u32::from(combined_infinite),
            len,
            ..n
        };
        // Wrappers remain in the original tree for syntax/name ancestry. No
        // new node, repeat record, register pair or native recursion is needed.
        let mut wrapper = child;
        while wrapper != id {
            self.step()?;
            self.nodes[wrapper as usize].len = len;
            wrapper = self.nodes[wrapper as usize].a;
        }
        Ok(true)
    }
}
