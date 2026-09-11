//! ECMAScript case equivalence without constructing a folded subject.
#[path = "casefold_data.rs"]
mod data;

/// Bounds of just the ASCII members of an equivalence family. Candidate-start
/// inference needs these bounds, not a traversal of the non-ASCII case tables.
pub(crate) fn ascii_bounds(c: u32, unicode: bool) -> Option<(u32, u32)> {
    match c {
        65..=90 => Some((c, c + 32)),
        97..=122 => Some((c - 32, c)),
        0..=127 => Some((c, c)),
        0x17f if unicode => Some((83, 115)),
        0x212a if unicode => Some((75, 107)),
        _ => None,
    }
}

/// At most four scalar values, independent of subject or pattern length.
/// The generator exhaustively verifies the cycle bound against pinned UCD.
pub(crate) fn equivalents(c: u32, unicode: bool) -> [u32; 4] {
    let mut result = [c; 4];
    if c < 128 && (!unicode || !matches!(c, 75 | 83 | 107 | 115)) {
        if matches!(c, 65..=90 | 97..=122) {
            result[1] = c ^ 32;
        }
        return result;
    }
    let mut at = c;
    for value in &mut result[1..] {
        at = next(at, unicode);
        if at == c {
            break;
        }
        *value = at;
    }
    result
}

fn next(c: u32, unicode: bool) -> u32 {
    let table = if unicode { data::UNICODE } else { data::LEGACY };
    let index = table.partition_point(|r| r[0] as u32 <= c);
    if index == 0 {
        return c;
    }
    let row = table[index - 1];
    if c > row[1] as u32 {
        return c;
    }
    (c as i32 + row[2 + ((c - row[0] as u32) & 1) as usize]) as u32
}

pub(crate) fn equal(left: u32, right: u32, unicode: bool) -> bool {
    if left == right {
        return true;
    }
    if left < 128 && right < 128 {
        return matches!(left, 65..=90 | 97..=122) && left ^ 32 == right;
    }
    equivalents(left, unicode).contains(&right)
}

pub(crate) fn word(c: u32, unicode_ignore_case: bool) -> bool {
    matches!(c, 48..=57 | 65..=90 | 95 | 97..=122)
        || (unicode_ignore_case && matches!(c, 0x17f | 0x212a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_inference_agrees_with_every_pinned_case_family() {
        for unicode in [false, true] {
            for c in 0..=0x10ffff {
                let family = equivalents(c, unicode);
                let mut ascii = family.into_iter().filter(|&member| member < 128);
                let expected = ascii.next().map(|first| {
                    ascii.fold((first, first), |(lo, hi), member| {
                        (lo.min(member), hi.max(member))
                    })
                });
                assert_eq!(
                    ascii_bounds(c, unicode),
                    expected,
                    "{c:x}, unicode={unicode}"
                );
            }
        }
    }
}
