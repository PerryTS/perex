//! Name interning in caller scratch; no source pointers survive compilation.
use super::*;

impl Parser<'_, '_> {
    pub(super) fn has_named_mode(&mut self) -> Result<bool, CompileError> {
        if self.named_mode.is_none() {
            self.scan_groups()?;
        }
        Ok(self.named_mode.unwrap())
    }

    fn name_point(&self, start: u32, index: u32) -> u32 {
        self.ranges[self.ranges.len() - 1 - start as usize - index as usize].lo
    }

    pub(super) fn name(&mut self) -> Result<u32, CompileError> {
        let start = u32::try_from(self.name_chars).map_err(|_| CompileError::SizeLimit)?;
        let mut count = 0u32;
        let mut units = 0u32;
        while !self.eat(b'>')? {
            let point = if self.eat(b'\\')? {
                if !self.eat(b'u')? {
                    return Err(self.error());
                }
                // Group names use Unicode escapes/identifier rules even when
                // the pattern otherwise consumes individual UTF-16 units.
                let flags = self.flags;
                self.flags |= U;
                let result = self.escaped_point(117, false);
                self.flags = flags;
                result?
            } else {
                self.step()?;
                self.cursor.next_point().ok_or_else(|| self.error())?
            };
            if !properties::identifier(point, count == 0) {
                return Err(self.error());
            }
            if self.range_used == self.ranges.len() - self.name_chars {
                return Err(CompileError::Ranges);
            }
            self.ranges[self.ranges.len() - 1 - self.name_chars] = Range { lo: point, hi: 0 };
            self.name_chars += 1;
            count = count.checked_add(1).ok_or(CompileError::SizeLimit)?;
            units = units
                .checked_add(if point > 0xffff { 2 } else { 1 })
                .ok_or(CompileError::SizeLimit)?;
        }
        if count == 0 {
            return Err(self.error());
        }
        let mut next = self.name_head;
        while next != 0 {
            self.step()?;
            let old = self.nodes[next as usize - 1];
            let mut equal = old.b == count;
            if equal {
                for i in 0..count {
                    self.step()?;
                    if self.name_point(start, i) != self.name_point(old.a, i) {
                        equal = false;
                        break;
                    }
                }
            }
            if equal {
                self.name_chars = start as usize;
                return Ok(next - 1);
            }
            next = old.start;
        }
        let id = self.add(Node {
            kind: NAME_META,
            a: start,
            b: count,
            c: u32::MAX,
            len: units,
            start: self.name_head,
            ..Node::default()
        })?;
        self.name_head = id.checked_add(1).ok_or(CompileError::SizeLimit)?;
        Ok(id)
    }

    pub(super) fn declare_name(&mut self, name: u32, capture: u32) -> Result<u32, CompileError> {
        let previous = self.nodes[name as usize].end;
        let id = self.add(Node {
            kind: NAME_DECL,
            a: name,
            b: capture,
            ..Node::default()
        })?;
        let link = id.checked_add(1).ok_or(CompileError::SizeLimit)?;
        if previous != 0 {
            self.nodes[previous as usize - 1].flags = link;
        }
        let meta = &mut self.nodes[name as usize];
        if meta.first == 0 {
            meta.c = self.name_count;
            self.name_count = self
                .name_count
                .checked_add(1)
                .ok_or(CompileError::SizeLimit)?;
            meta.flags = link;
        }
        meta.first = meta.first.checked_add(1).ok_or(CompileError::SizeLimit)?;
        meta.end = link;
        Ok(id)
    }

    pub(super) fn check_names(&mut self) -> Result<(), CompileError> {
        if self.name_head == 0 {
            return Ok(());
        }
        // Temporarily use AST emission positions for parent links. Parents have
        // larger arena indices than children, so LCA needs no extra stack.
        for i in 0..self.used {
            self.step()?;
            let n = self.nodes[i];
            if matches!(n.kind, SEQ | ALT | GROUP | WRAP | ASSERT | REPEAT) {
                self.nodes[n.a as usize].start = i as u32 + 1;
                if matches!(n.kind, SEQ | ALT) {
                    self.nodes[n.b as usize].start = i as u32 + 1;
                }
            }
        }
        let mut next = self.name_head;
        while next != 0 {
            self.step()?;
            let name = self.nodes[next as usize - 1];
            if name.first == 0 {
                return Err(self.error());
            }
            let mut left = name.flags;
            while left != 0 {
                self.step()?;
                let declaration = self.nodes[left as usize - 1];
                let mut right = declaration.flags;
                while right != 0 {
                    self.step()?;
                    let other = self.nodes[right as usize - 1];
                    let (mut a, mut b) = (declaration.c, other.c);
                    while a != b {
                        self.step()?;
                        if a < b {
                            a = self.nodes[a as usize].start - 1;
                        } else {
                            b = self.nodes[b as usize].start - 1;
                        }
                    }
                    // The enclosing disjunction must put these declarations
                    // in different alternatives; being merely optional is not enough.
                    if self.nodes[a as usize].kind != ALT {
                        return Err(self.error());
                    }
                    right = other.flags;
                }
                left = declaration.flags;
            }
            next = name.start;
        }
        for i in 0..self.used {
            self.step()?;
            let n = self.nodes[i];
            if n.kind == NAMED_BACKREF {
                let name = self.nodes[n.a as usize];
                if name.first == 1 {
                    self.nodes[i].kind = BACKREF;
                    self.nodes[i].a = self.nodes[name.flags as usize - 1].b;
                } else {
                    self.nodes[i].a = name.c;
                }
            }
        }
        Ok(())
    }

    pub(super) fn names_words(&mut self) -> Result<usize, CompileError> {
        if self.name_count == 0 {
            return Ok(0);
        }
        let mut words = 1usize;
        let mut next = self.name_head;
        while next != 0 {
            self.step()?;
            let n = self.nodes[next as usize - 1];
            words = words
                .checked_add(3)
                .and_then(|v| v.checked_add((n.len as usize).div_ceil(2)))
                .and_then(|v| v.checked_add(n.first as usize))
                .ok_or(CompileError::SizeLimit)?;
            next = n.start;
        }
        Ok(words)
    }

    pub(super) fn write_names(
        &mut self,
        output: &mut [u32],
        base: usize,
    ) -> Result<(), CompileError> {
        if self.name_count == 0 {
            return Ok(());
        }
        output[base] = self.name_count;
        let mut at = base + 1 + self.name_count as usize * 3;
        // Declarations are allocated before parsing their children. This emits
        // first-declaration order even if a name was interned by a forward reference.
        for i in 0..self.used {
            self.step()?;
            let d = self.nodes[i];
            if d.kind != NAME_DECL {
                continue;
            }
            let n = self.nodes[d.a as usize];
            if n.flags as usize != i + 1 {
                continue;
            }
            let row = base + 1 + n.c as usize * 3;
            output[row..row + 3].copy_from_slice(&[n.len, n.first, at as u32]);
            let mut unit_index = 0;
            for j in 0..n.b {
                self.step()?;
                let c = self.name_point(n.a, j);
                let mut units = [0; 2];
                for &unit in char::from_u32(c).unwrap().encode_utf16(&mut units).iter() {
                    let word = &mut output[at + unit_index / 2];
                    if unit_index % 2 == 0 {
                        *word = u32::from(unit);
                    } else {
                        *word |= u32::from(unit) << 16;
                    }
                    unit_index += 1;
                }
            }
            at += (n.len as usize).div_ceil(2);
            let mut declaration = n.flags;
            while declaration != 0 {
                self.step()?;
                let d = self.nodes[declaration as usize - 1];
                output[at] = d.b;
                at += 1;
                declaration = d.flags;
            }
        }
        Ok(())
    }
}
