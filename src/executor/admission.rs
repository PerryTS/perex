//! Necessary-condition search, using the original subject and the same character
//! semantics as the VM. A true result admits normal execution; only a proved
//! absence can reject. The full subject includes text needed by lookbehind.
use super::*;

fn charge(budget: &mut Budget, work: usize) -> Result<(), ExecError> {
    budget.charge(work).map_err(|_| ExecError::WorkLimit)
}
fn point(cursor: &mut Cursor<'_>, unicode: bool) -> Option<u32> {
    if unicode {
        cursor.next_point()
    } else {
        cursor.next_unit().map(u32::from)
    }
}
fn literal(p: Program<'_>, pc: usize, len: usize, reverse: bool, index: usize) -> u32 {
    p.instruction(pc + if reverse { len - 1 - index } else { index })[1]
}

pub(super) fn admits(
    p: Program<'_>,
    input: Input<'_>,
    budget: &mut Budget,
) -> Result<bool, ExecError> {
    let Some((pc, reverse)) = p.admission() else {
        return Ok(true);
    };
    // find gates short subjects and programs without a hint before calling.
    let [op, a, b] = p.instruction(pc);
    if op == CLASS {
        if b == 1 && p.words[2] & I == 0 {
            let [lo, hi] = p.range(a as usize);
            if hi < 128
                && lo <= hi
                && let Some(bytes) = input.original_bytes()
            {
                // ASCII bytes cannot occur inside a UTF-8/WTF-8 encoding.
                for chunk in bytes.chunks(256) {
                    charge(budget, chunk.len())?;
                    if chunk
                        .iter()
                        .any(|&byte| byte >= lo as u8 && byte <= hi as u8)
                    {
                        return Ok(true);
                    }
                }
                return Ok(false);
            }
        }
        let mut cursor = input.cursor();
        while let Some(c) = point(&mut cursor, p.unicode()) {
            charge(budget, 1)?;
            if class_matches(p, a, b, c, budget)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    let len = p.literal_run(pc);
    charge(budget, len)?;
    // This small buffer contains pattern bytes only. No subject bytes or units
    // are copied. Non-ASCII needles use the original instruction records.
    let mut needle = [0; ADMISSION_MAX];
    let mut ascii = true;
    for (i, byte) in needle[..len].iter_mut().enumerate() {
        let c = literal(p, pc, len, reverse, i);
        if c >= 128 {
            ascii = false;
            break;
        }
        *byte = c as u8;
    }
    let fold = p.words[2] & I != 0;
    let bytes = if fold {
        input.ascii_bytes()
    } else {
        input.original_bytes()
    };
    if ascii && let Some(bytes) = bytes {
        let needle = &needle[..len];
        for (chunk_index, chunk) in bytes.chunks(256).enumerate() {
            // Charge before scanning each bounded chunk. A low budget cannot
            // trigger a whole large scan before being checked.
            charge(budget, chunk.len())?;
            let mut offset = 0;
            while let Some(hit) = chunk[offset..].iter().position(|&byte| {
                if fold {
                    byte.eq_ignore_ascii_case(&needle[0])
                } else {
                    byte == needle[0]
                }
            }) {
                offset += hit;
                charge(budget, len)?;
                let start = chunk_index * 256 + offset;
                if let Some(window) = bytes.get(start..start + len)
                    && (if fold {
                        window.eq_ignore_ascii_case(needle)
                    } else {
                        window == needle
                    })
                {
                    return Ok(true);
                }
                offset += 1;
            }
        }
        return Ok(false);
    }
    // A required literal at the subject's end can admit immediately. Starting
    // at that boundary is constant-time in every supported storage, so Unicode
    // suffix hits do not decode the entire subject before the VM scans it again.
    // Failure of this bounded probe still performs the complete absence check.
    let mut tail = input.cursor_at(input.len_utf16()).unwrap();
    let mut suffix = true;
    for i in (0..len).rev() {
        charge(budget, 1)?;
        let c = if p.unicode() {
            tail.previous_point()
        } else {
            tail.previous_unit().map(u32::from)
        };
        if !c.is_some_and(|c| equal(p, c, literal(p, pc, len, reverse, i))) {
            suffix = false;
            break;
        }
    }
    if suffix {
        return Ok(true);
    }
    let first = literal(p, pc, len, reverse, 0);
    let mut cursor = input.cursor();
    while let Some(c) = point(&mut cursor, p.unicode()) {
        charge(budget, 1)?;
        if !equal(p, c, first) {
            continue;
        }
        let mut probe = cursor;
        let mut matched = true;
        for i in 1..len {
            charge(budget, 1)?;
            if !point(&mut probe, p.unicode())
                .is_some_and(|next| equal(p, next, literal(p, pc, len, reverse, i)))
            {
                matched = false;
                break;
            }
        }
        if matched {
            return Ok(true);
        }
    }
    Ok(false)
}
