//! Shared immutable data for the properties of strings, whose members are
//! sequences of code points rather than single ones. Programs store a set id,
//! never a copy of the members.
//!
//! Members of one code point are not here: `Basic_Emoji`'s are exactly
//! `Emoji_Presentation` without the regional indicators, which the compiler
//! spells with the property references it already has. What remains is, for
//! each set, its members of two or more code points, grouped by length and
//! ordered longest group first — the order the specification's matcher tries
//! them in. Within a group the members are ordered by their first code point,
//! so a subject character selects a run of candidates without scanning.
#[path = "sequence_data.rs"]
mod data;

/// The property references the code point members are spelled with.
pub(crate) const EMOJI_PRESENTATION: u32 = data::EMOJI_PRESENTATION;
pub(crate) const REGIONAL_INDICATOR: u32 = data::REGIONAL_INDICATOR;
/// Code points in the longest member of any set. Nothing longer can be one.
pub(crate) const LONGEST: usize = data::LONGEST;

/// The property of strings with this name, if it is one.
pub(crate) fn resolve(name: &[u8]) -> Option<u32> {
    data::SEQUENCE_NAMES
        .iter()
        .position(|known| known.as_bytes() == name)
        .map(|at| at as u32)
}

pub(crate) fn valid(set: u32) -> bool {
    (set as usize) < data::SEQUENCE_SETS.len()
}

/// Whether this set also has members of one code point.
pub(crate) fn has_points(set: u32) -> bool {
    data::SEQUENCE_SETS[set as usize][4] != 0
}

/// The set's `index`th member, counted from the longest.
pub(crate) fn member(set: u32, index: usize) -> &'static [u32] {
    entry(data::SEQUENCE_SETS[set as usize][2] as usize + index)
}

/// How many groups of equal length the set has.
pub(crate) fn groups(set: u32) -> usize {
    data::SEQUENCE_SETS[set as usize][1] as usize
}

/// How many members of two or more code points the set has. A filter over the
/// whole set is one bit per member, in this order.
pub(crate) fn count(set: u32) -> usize {
    data::SEQUENCE_SETS[set as usize][3] as usize
}

/// Whether any member of the set begins with this code point. A subject
/// character that begins none rejects the set without a search, which is what
/// scanning text that holds no member costs.
pub(crate) fn begins(set: u32, c: u32) -> bool {
    let row = data::SEQUENCE_SETS[set as usize];
    if c < 128 {
        row[5 + c as usize / 32] & (1 << (c % 32)) != 0
    } else {
        row[9] != 0
    }
}

/// The group a member index belongs to, which a resumed instruction needs
/// when it covers every group of its set.
pub(crate) fn group_of(set: u32, index: usize) -> usize {
    for group in 0..groups(set) {
        if index < self::group(set, group).1 {
            return group;
        }
    }
    groups(set).saturating_sub(1)
}

/// One group as member indices within its set: the members of one length, from
/// `start` up to but not including `end`.
pub(crate) fn group(set: u32, group: usize) -> (usize, usize) {
    let row = data::SEQUENCE_GROUPS[data::SEQUENCE_SETS[set as usize][0] as usize + group];
    let base = data::SEQUENCE_SETS[set as usize][2];
    ((row[1] - base) as usize, (row[1] + row[2] - base) as usize)
}

/// The members of one group whose first code point is `first`, as a narrower
/// range of the same member indices. Used only where a comparison is exact;
/// under case folding the caller walks the whole group instead.
pub(crate) fn run(set: u32, group: usize, first: u32) -> (usize, usize) {
    let (start, end) = self::group(set, group);
    let base = data::SEQUENCE_SETS[set as usize][2] as usize;
    let lo = partition(base + start, base + end, first);
    let hi = partition(base + start, base + end, first + 1);
    (lo - base, hi - base)
}

/// The first entry of `[start, end)` whose first code point is not below
/// `first`. The group is ordered by that code point.
fn partition(start: usize, end: usize, first: u32) -> usize {
    let (mut lo, mut hi) = (start, end);
    while lo < hi {
        let middle = lo + (hi - lo) / 2;
        if entry(middle)[0] < first {
            lo = middle + 1;
        } else {
            hi = middle;
        }
    }
    lo
}

/// How many code points the members of one group have.
pub(crate) fn length(set: u32, group: usize) -> usize {
    data::SEQUENCE_GROUPS[data::SEQUENCE_SETS[set as usize][0] as usize + group][0] as usize
}

/// Where `points` sits in `set` as the group holding it and its index within
/// the set, or `None` when it is not a member. Under folding the comparison is
/// by case equivalence, so the ordered first code points cannot narrow the
/// search and the whole group is walked.
pub(crate) fn position(set: u32, points: &[u32], fold: bool) -> Option<(usize, usize)> {
    if points.len() < 2 || points.len() > LONGEST {
        return None;
    }
    for group in 0..groups(set) {
        if length(set, group) != points.len() {
            continue;
        }
        let (start, end) = if fold {
            self::group(set, group)
        } else {
            run(set, group, points[0])
        };
        for index in start..end {
            if same(member(set, index), points, fold) {
                return Some((group, index));
            }
        }
    }
    None
}

/// Whether two members of the same length spell the same sequence.
pub(crate) fn same(left: &[u32], right: &[u32], fold: bool) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(&a, &b)| a == b || (fold && crate::casefold::equal(a, b, true)))
}

/// One member, addressed by its position in the shared index.
fn entry(at: usize) -> &'static [u32] {
    let row = data::SEQUENCE_INDEX[at] as usize;
    let (offset, length) = (row & 0xfffff, row >> 20);
    &data::SEQUENCE_POINTS[offset..offset + length]
}
