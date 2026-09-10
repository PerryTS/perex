//! Legacy numeric ambiguity is resolved before quantifying an atom. A suffix
//! left after an octal escape remains ordinary pattern syntax, not a backref.
use super::*;

impl Parser<'_, '_> {
    // This lossless, allocation-free scan is lazy and shared by forward decimal
    // references and legacy \k. Escaped parens and class contents do not count.
    pub(super) fn scan_groups(&mut self) -> Result<(), CompileError> {
        let mut cursor = self.pattern.cursor();
        let mut class = false;
        let mut named = false;
        let mut captures = 1u32;
        while let Some(c) = cursor.next_unit() {
            self.step()?;
            match c {
                92 => {
                    cursor.next_unit();
                }
                91 if !class => class = true,
                93 if class => class = false,
                40 if !class => {
                    let mut look = cursor;
                    let ordinary = look.next_unit() != Some(63);
                    let name = !ordinary
                        && look.next_unit() == Some(60)
                        && !matches!(look.next_unit(), Some(61 | 33));
                    if ordinary || name {
                        captures = captures.checked_add(1).ok_or(CompileError::SizeLimit)?;
                    }
                    named |= name;
                }
                _ => {}
            }
        }
        self.capture_total = Some(captures);
        self.named_mode = Some(named || self.flags & U != 0);
        Ok(())
    }

    pub(super) fn numeric_character(&mut self, first: u16) -> Result<u32, CompileError> {
        if self.flags & U != 0 {
            return if first == 48 && !matches!(self.peek(), Some(48..=57)) {
                Ok(0)
            } else {
                Err(self.error())
            };
        }
        if first >= 56 {
            return Ok(u32::from(first));
        }
        let mut value = u32::from(first - 48);
        // A leading 0..3 permits three octal digits; 4..7 permits only two.
        for _ in 0..if first <= 51 { 2 } else { 1 } {
            let Some(c @ 48..=55) = self.peek() else {
                break;
            };
            self.take()?;
            value = value * 8 + u32::from(c - 48);
        }
        Ok(value)
    }

    pub(super) fn numeric_node(&mut self, first: u16) -> Result<u32, CompileError> {
        let after_first = self.cursor;
        let mut value = Some(u32::from(first - 48));
        while let Some(c @ 48..=57) = self.peek() {
            self.take()?;
            value = value.and_then(|v| v.checked_mul(10)?.checked_add(u32::from(c - 48)));
        }
        if let Some(number) = value {
            // Already-declared captures (including an open self-reference) do
            // not require the group scan. Otherwise forward captures can decide
            // whether the whole decimal escape is a reference.
            if number >= self.captures && self.capture_total.is_none() {
                self.scan_groups()?;
            }
            if number < self.captures || number < self.capture_total.unwrap_or(0) {
                return self.leaf(BACKREF, number, 0);
            }
        }
        if self.flags & U != 0 {
            return Err(self.error());
        }
        self.cursor = after_first;
        let point = self.numeric_character(first)?;
        self.leaf(CHAR, point, 0)
    }
}
