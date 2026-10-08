//! A lazy deterministic automaton over host-owned storage: whether a program
//! matches, and the bounds of its match, without backtracking. States are
//! built from the program's instructions when a search first reaches them and
//! kept in a cache the host allocates, sizes and may move. See `docs/dfa.md`.
//!
//! The backtracking evaluator remains the definition of every answer and the
//! only source of captures. Programs the compiler did not mark eligible, and
//! searches the cache cannot hold, are declined and left to it.
use crate::{
    Budget, casefold,
    executor::{
        ExecError,
        candidate::{Prefix, first_in_pairs, prefix, scan_range},
    },
    input::{Cursor, Input},
    program::*,
    properties,
    span::Span,
};

/// Why a search was left to the evaluator. A declined search changed nothing
/// but the cache and the budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decline {
    /// The program uses something an automaton cannot decide exactly.
    Ineligible,
    /// The cache storage is smaller than this program's minimum.
    Storage { required_words: usize },
    /// Clearing repeatedly showed this program's states do not stay cached.
    /// Every call declines until the host resets the cache.
    Disabled,
    /// One transition needed more threads, or the subject more character
    /// classes, than this cache holds.
    Capacity,
}

/// The answer of [`is_match`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tested {
    Match,
    NoMatch,
    Declined(Decline),
}

/// The answer of [`find`]: the span of capture group zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Found {
    Match(Span),
    NoMatch,
    Declined(Decline),
}

/// What a cache holds, for the host's memory accounting.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CacheStats {
    /// Storage the cache's layout covers: the host's allocation.
    pub words: usize,
    /// Words in use now: tables, signatures and live states.
    pub used_words: usize,
    /// The most words ever in use since the cache was built.
    pub peak_words: usize,
    pub states: usize,
    pub classes: usize,
    pub clears: usize,
    pub disabled: bool,
}

const MAGIC: u32 = 0x5058_4446;
const LAYOUT: u32 = 1;
const HEADER_WORDS: usize = 64;

// Header words.
const H_MAGIC: usize = 0;
const H_LAYOUT: usize = 1;
const H_LEN: usize = 2;
/// Program words 2..=10, compared on every call.
const H_PROGRAM: usize = 3;
const H_CAP: usize = 12;
const H_STATUS: usize = 13;
const H_COLUMNS: usize = 14;
const H_CLASSES: usize = 15;
const H_PREDICATES: usize = 16;
const H_SIGW: usize = 17;
const H_NEED: usize = 18;
const H_BUMP: usize = 19;
const H_STATES: usize = 20;
const H_CLEARS: usize = 21;
const H_BAD: usize = 22;
const H_UNITS: usize = 23;
const H_BUILT: usize = 24;
const H_PEAK: usize = 25;
const H_GEN: usize = 26;
const H_MEMO_USED: usize = 27;
const H_THREADS: usize = 28;
const H_PRED: usize = 29;
const H_FLAGS: usize = 30;
const H_REP: usize = 31;
const H_RPS: usize = 32;
const H_RPL: usize = 33;
const H_REPS: usize = 34;
const H_MEMO: usize = 35;
const H_SIGTMP: usize = 36;
const H_TABLE: usize = 37;
const H_TABLE_SLOTS: usize = 38;
const H_STACK: usize = 39;
const H_OUT: usize = 40;
const H_SAVED: usize = 41;
const H_SIG: usize = 42;
const H_SIGCAP: usize = 43;
const H_HASH: usize = 44;
const H_HASH_SLOTS: usize = 45;
const H_REGION: usize = 46;
/// The byte pairs the program's leading claim admits, as the evaluator's
/// candidate scan reads them: their count, then the first bytes and the second
/// bytes, four to a word.
const H_PAIRS: usize = 47;
/// Nonzero when no consuming instruction a search can reach from instruction
/// zero without consuming accepts a non-ASCII character.
const H_ASCII_START: usize = 52;

const ASCII: usize = HEADER_WORDS;
const SLOTS: usize = ASCII + 128;
const FRONT: usize = SLOTS + 48;

/// A transition not yet computed.
const UNKNOWN: u32 = 0;
/// The column of a character whose class is not known yet. No transition is
/// ever stored in it, so it reads as `UNKNOWN` in every state and a lookup
/// needs no separate test for an unclassified character. Classes start at 2;
/// column 0 is the subject's edge.
const UNCLASSIFIED: u32 = 1;
/// No thread remains and no start will be added.
const DEAD: u32 = 1;
/// Forward: the position before this character ends a match. Reverse: it
/// begins one.
const FLAG: u32 = 1 << 31;

/// What a state knows about the character on its consumed side, and a
/// transition about the other. `CX_EDGE` is the subject's start or end.
const CX_EDGE: u32 = 1;
const CX_LT: u32 = 2;
const CX_WORD: u32 = 4;
const CX_WORD_UI: u32 = 8;
/// Signature bit of the first predicate; bits 1..=3 are `CX_LT`, `CX_WORD` and
/// `CX_WORD_UI` of the character itself.
const PRED_BASE: usize = 4;

const S_REVERSE: u32 = 1;
const S_INJECT: u32 = 2;

const MODE_UNANCHORED: usize = 0;
const MODE_ANCHORED: usize = 1;
const MODE_REVERSE: usize = 2;

const MEMO_SLOTS: usize = 512;
/// Storage at or above this many words is declined: a transition holds a
/// state's offset beside the match flag in bit 31.
const MAX_WORDS: usize = 1 << 30;
/// Subject bytes left below which the steady path does not set up the skip
/// to a byte a match can begin with.
const SKIP_MIN: usize = 32;
const FIRST_COLUMNS: usize = 8;
const MAX_CLASSES: usize = 1022;
/// A clear after fewer characters than this per state built is thrashing.
const UNITS_PER_STATE: u32 = 8;
/// Consecutive thrashing clears that disable the cache.
const THRASH_LIMIT: u32 = 3;
/// Visited-table key bit marking an entry of the next state's thread list.
const OUT_KEY: u32 = 1 << 31;
const GEN_SHIFT: u32 = 24;
const PC_MASK: u32 = (1 << GEN_SHIFT) - 1;

/// Whether the compiler marked this program for the automaton.
pub fn eligible(program: Program<'_>) -> bool {
    program.words[7] & DFA != 0
}

/// The least cache storage, in words, that `program` can be searched in, or
/// `None` when it is not eligible. More storage holds more states.
pub fn minimum_words(program: Program<'_>) -> Option<usize> {
    if !eligible(program) {
        return None;
    }
    let (edges, consuming) = census(program);
    let n = program.instructions();
    Some(plan(n, program.words[6] as usize, consuming, edges, 0, true).required)
}

/// The program's epsilon edges and consuming instructions.
fn census(program: Program<'_>) -> (usize, usize) {
    let mut edges = 0;
    let mut count = 0;
    for pc in 0..program.instructions() {
        edges += successors(program, pc).0;
        count += usize::from(consuming(program.instruction(pc)[0]));
    }
    (edges, count)
}

/// Forget everything the cache holds. The next search rebuilds it.
pub fn reset(cache: &mut [u32]) {
    if let Some(first) = cache.first_mut() {
        *first = 0;
    }
}

/// What `cache` holds, or `None` when it holds no built cache.
pub fn stats(cache: &[u32]) -> Option<CacheStats> {
    if cache.len() < HEADER_WORDS || cache[H_MAGIC] != MAGIC || cache[H_LAYOUT] != LAYOUT {
        return None;
    }
    Some(CacheStats {
        words: cache[H_CAP] as usize,
        used_words: cache[H_BUMP] as usize,
        peak_words: cache[H_PEAK] as usize,
        states: cache[H_STATES] as usize,
        classes: cache[H_CLASSES] as usize,
        clears: cache[H_CLEARS] as usize,
        disabled: cache[H_STATUS] != 0,
    })
}

/// Whether `program` matches `input` at or after `start_utf16`, answered by
/// the automaton in `cache`. The cache must belong to this program: the host
/// keeps one per program. A cache built for another program of the same
/// length and header is the host's error and may give wrong answers; one with
/// a different length or header is rebuilt.
pub fn is_match(
    program: Program<'_>,
    input: Input<'_>,
    start_utf16: usize,
    cache: &mut [u32],
    budget: &mut Budget,
) -> Result<Tested, ExecError> {
    Ok(
        match search(program, input, start_utf16, cache, budget, false)? {
            Outcome::Declined(why) => Tested::Declined(why),
            Outcome::NoMatch => Tested::NoMatch,
            Outcome::Matched(_) => Tested::Match,
        },
    )
}

/// The span of the match the evaluator would return for `program` on `input`
/// at or after `start_utf16` — group zero, leftmost first — answered by the
/// automaton in `cache`. Captures come from the evaluator, started at the
/// span's start. The cache contract is [`is_match`]'s.
pub fn find(
    program: Program<'_>,
    input: Input<'_>,
    start_utf16: usize,
    cache: &mut [u32],
    budget: &mut Budget,
) -> Result<Found, ExecError> {
    Ok(
        match search(program, input, start_utf16, cache, budget, true)? {
            Outcome::Declined(why) => Found::Declined(why),
            Outcome::NoMatch => Found::NoMatch,
            Outcome::Matched(span) => Found::Match(span.ok_or(ExecError::InvalidProgram)?),
        },
    )
}

enum Outcome {
    Matched(Option<Span>),
    NoMatch,
    Declined(Decline),
}

/// Why a search stopped before deciding.
enum Stop {
    Declined(Decline),
    Error(ExecError),
}
impl From<ExecError> for Stop {
    fn from(error: ExecError) -> Self {
        Stop::Error(error)
    }
}

#[inline(always)]
fn search(
    program: Program<'_>,
    input: Input<'_>,
    start_utf16: usize,
    cache: &mut [u32],
    budget: &mut Budget,
    bounds: bool,
) -> Result<Outcome, ExecError> {
    // A validated program holds its whole header; reading it as one array
    // checks that once.
    let Some(words) = program.words.first_chunk::<HEADER>() else {
        return general(program, input, start_utf16, cache, budget, bounds);
    };
    let anchored = words[2] & Y != 0 || words[7] & ANCHORED != 0;
    if words[7] & DFA != 0
        && built_for(words, program.words.len(), cache)
        && cache[H_STATUS] == 0
        && start_utf16 <= input.len_utf16()
        && let Some(end) = steady(
            program,
            input,
            start_utf16,
            cache,
            budget,
            !bounds,
            anchored,
        )
    {
        return match end {
            None => Ok(Outcome::NoMatch),
            Some(_) if !bounds => Ok(Outcome::Matched(None)),
            Some(end) if anchored => Ok(Outcome::Matched(Span::new(start_utf16, end))),
            // The steady path starts where it was asked to: ASCII storage
            // needs no Unicode adjustment, and other storage starts at zero.
            Some(end) => match reverse_steady(input, start_utf16, end, cache, budget) {
                Some(begin) => Ok(Outcome::Matched(Span::new(begin, end))),
                None => reverse_only(program, input, start_utf16, end, cache, budget),
            },
        };
    }
    general(program, input, start_utf16, cache, budget, bounds)
}

/// The reverse pass over wholly ASCII storage when every transition it takes
/// is cached: the leftmost start at or after `lower` of the match ending at
/// `end`. `None` as soon as anything is not, before anything changes.
#[inline(always)]
fn reverse_steady(
    input: Input<'_>,
    lower: usize,
    end: usize,
    m: &mut [u32],
    budget: &mut Budget,
) -> Option<usize> {
    let bytes = input.ascii_bytes()?;
    let need = m[H_NEED];
    let ctx = match bytes.get(end) {
        _ if need == 0 => 0,
        None => CX_EDGE & need,
        Some(&byte) => context(u32::from(byte)) & need,
    };
    let mut s = m[SLOTS + MODE_REVERSE * 16 + ctx as usize] as usize;
    if s == 0 {
        return None;
    }
    let table: &[u32; 128] = m[ASCII..ASCII + 128].try_into().ok()?;
    let mut found = None;
    let mut at = end;
    loop {
        let column = match at.checked_sub(1) {
            Some(before) => table[usize::from(*bytes.get(before)? & 127)] as usize,
            None => 0,
        };
        let entry = m[s + column];
        if entry == UNKNOWN {
            return None;
        }
        if entry & FLAG != 0 {
            found = Some(at);
        }
        let next = (entry & !FLAG) as usize;
        if at == 0 || at <= lower || next == DEAD as usize {
            break;
        }
        s = next;
        at -= 1;
    }
    let found = found?;
    budget.charge(end - at + 1).ok()?;
    m[H_UNITS] = m[H_UNITS].saturating_add((end - at + 1).min(u32::MAX as usize) as u32);
    Some(found)
}

/// The reverse pass alone, for a match end the steady path found.
#[inline(never)]
fn reverse_only(
    program: Program<'_>,
    input: Input<'_>,
    start: usize,
    end: usize,
    cache: &mut [u32],
    budget: &mut Budget,
) -> Result<Outcome, ExecError> {
    let mut dfa = Dfa {
        program,
        m: cache,
        budget: *budget,
        progress: 0,
        flushed: 0,
        unicode: program.unicode(),
    };
    let result = dfa.reverse(input, end, start);
    dfa.flush_units();
    *budget = dfa.budget;
    match result {
        Ok(begin) => Ok(Outcome::Matched(Span::new(begin, end))),
        Err(Stop::Declined(why)) => Ok(Outcome::Declined(why)),
        Err(Stop::Error(error)) => Err(error),
    }
}

/// Everything [`search`]'s steady path leaves: building the cache, states
/// and classes not yet cached, other storage, and the reverse pass.
#[inline(never)]
fn general(
    program: Program<'_>,
    input: Input<'_>,
    start_utf16: usize,
    cache: &mut [u32],
    budget: &mut Budget,
    bounds: bool,
) -> Result<Outcome, ExecError> {
    if !eligible(program) {
        return Ok(Outcome::Declined(Decline::Ineligible));
    }
    let built = program
        .words
        .first_chunk::<HEADER>()
        .is_some_and(|words| built_for(words, program.words.len(), cache));
    if !built && let Err(why) = build(program, cache) {
        return Ok(Outcome::Declined(why));
    }
    if cache[H_STATUS] != 0 {
        return Ok(Outcome::Declined(Decline::Disabled));
    }
    if start_utf16 > input.len_utf16() {
        return Ok(Outcome::NoMatch);
    }
    let mut dfa = Dfa {
        program,
        m: cache,
        budget: *budget,
        progress: 0,
        flushed: 0,
        unicode: program.unicode(),
    };
    let result = dfa.run(input, start_utf16, bounds);
    dfa.flush_units();
    *budget = dfa.budget;
    match result {
        Ok(outcome) => Ok(outcome),
        Err(Stop::Declined(why)) => Ok(Outcome::Declined(why)),
        Err(Stop::Error(error)) => Err(error),
    }
}

/// The forward pass over byte storage when everything it meets is already
/// cached: the start state, every character's class (in the ASCII table or
/// the first slot of its memo probe) and every transition. `None` as soon as
/// anything is not, before any state changes; the general path then runs the
/// search from its start.
///
/// Wholly ASCII storage is searched from any start: it holds no surrogate, so
/// a Unicode start needs no adjustment. Other WTF-8 storage is searched from
/// the subject's start only, decoding each scalar once; a high surrogate under
/// `u`, which may pair with a separately encoded low one, is left to the
/// general path.
///
/// With `first` it stops at the first match end and reports that there is
/// one; over WTF-8 storage the position it carries is then not counted.
#[inline(always)]
fn steady(
    program: Program<'_>,
    input: Input<'_>,
    start: usize,
    m: &mut [u32],
    budget: &mut Budget,
    first: bool,
    anchored: bool,
) -> Option<Option<usize>> {
    let Some(bytes) = input.ascii_bytes() else {
        if start != 0 {
            return None;
        }
        return steady_wtf8(program, input.original_bytes()?, m, budget, first, anchored);
    };
    let need = m[H_NEED];
    let ctx = if need == 0 {
        0
    } else if start == 0 {
        CX_EDGE & need
    } else {
        return None;
    };
    let mode = if anchored {
        MODE_ANCHORED
    } else {
        MODE_UNANCHORED
    };
    let s = m[SLOTS + mode * 16 + ctx as usize] as usize;
    if s == 0 || budget.remaining() < bytes.len() {
        return None;
    }
    let (last, end) = if bytes.len() - start >= SKIP_MIN
        && !anchored
        && need == 0
        && program.first_descriptor() != 0
    {
        ascii_skipping(program, bytes, start, s, m, first)?
    } else {
        ascii_steady::<false>(bytes, start, s, m, first, &NO_SKIP)?
    };
    budget.charge(end - start).ok()?;
    m[H_UNITS] = m[H_UNITS].saturating_add((end - start).min(u32::MAX as usize) as u32);
    Some(last)
}

/// Where the steady path may skip ahead: from state `idle`, to the next
/// position the scans below admit.
struct Skip {
    idle: usize,
    lo: u8,
    hi: u8,
    pairs: Prefix,
}

const NO_SKIP: Skip = Skip {
    idle: usize::MAX,
    lo: 0,
    hi: 0,
    pairs: Prefix {
        first: [0; LEADING_BRANCHES],
        second: [0; LEADING_BRANCHES],
        members: 0,
    },
};

/// An unanchored search with no thread alive and no context to carry can only
/// begin a match where the evaluator's candidate scan would start a trial, so
/// from the start state `idle` it skips to the next such position with the
/// same scans: the leading claim's byte pairs, or the bytes word 7's
/// first-character descriptor admits. A descriptor of one admits no ASCII
/// byte. The pairs compare bytes, which over other storage than ASCII is exact
/// only when no non-ASCII character can begin a match and the run is not
/// folded: a folded `k` matches U+212A under `u`.
fn skip_for(program: Program<'_>, m: &[u32], idle: usize, ascii: bool) -> Skip {
    let descriptor = program.first_descriptor();
    let (lo, hi) = match descriptor {
        1 => (128, 127),
        _ => ((descriptor >> 8) as u8, (descriptor >> 16) as u8),
    };
    let members = m[H_PAIRS] as usize;
    let pairs = if members != 0 && (ascii || (m[H_ASCII_START] != 0 && !program.leading_fold())) {
        let byte = |k: usize| m[H_PAIRS + 1 + k / 4].to_le_bytes()[k % 4];
        Prefix {
            first: core::array::from_fn(byte),
            second: core::array::from_fn(|k| byte(k + LEADING_BRANCHES)),
            members: members.min(LEADING_BRANCHES),
        }
    } else {
        NO_SKIP.pairs
    };
    Skip {
        idle,
        lo,
        hi,
        pairs,
    }
}

/// The next position at or after `i` the skip admits, or the subject's end.
#[inline(always)]
fn skip_ascii<const MIXED: bool>(bytes: &[u8], i: usize, skip: &Skip) -> usize {
    let rest = &bytes[i..];
    let found = if skip.pairs.members != 0 {
        first_in_pairs(
            rest,
            rest.len(),
            &skip.pairs.first[..skip.pairs.members],
            &skip.pairs.second[..skip.pairs.members],
        )
        .0
    } else {
        scan_range::<MIXED, false>(rest, skip.lo, skip.hi)
    };
    i + found.unwrap_or(rest.len())
}

/// The steady path over a long ASCII subject, skipping from the start state.
#[inline(never)]
fn ascii_skipping(
    program: Program<'_>,
    bytes: &[u8],
    start: usize,
    s: usize,
    m: &[u32],
    first: bool,
) -> Option<(Option<usize>, usize)> {
    let skip = skip_for(program, m, s, true);
    ascii_steady::<true>(bytes, start, s, m, first, &skip)
}

/// The steady loop over wholly ASCII storage, where a byte is a character and
/// its index its position: the last match end, and where the search stopped.
#[inline(always)]
fn ascii_steady<const SKIP: bool>(
    bytes: &[u8],
    start: usize,
    mut s: usize,
    m: &[u32],
    first: bool,
    skip: &Skip,
) -> Option<(Option<usize>, usize)> {
    let table: &[u32; 128] = m[ASCII..ASCII + 128].try_into().ok()?;
    let mut last = None;
    let mut i = start;
    loop {
        if SKIP && s == skip.idle && i < bytes.len() {
            i = skip_ascii::<false>(bytes, i, skip);
        }
        let Some(&byte) = bytes.get(i) else {
            let entry = m[s];
            if entry == UNKNOWN {
                return None;
            }
            if entry & FLAG != 0 {
                last = Some(i);
            }
            break;
        };
        let entry = m[s + table[usize::from(byte & 127)] as usize];
        // Live, known and unflagged: the steady state, one comparison.
        if entry as i32 > DEAD as i32 {
            s = entry as usize;
            i += 1;
            continue;
        }
        if entry == UNKNOWN {
            return None;
        }
        if entry & FLAG != 0 {
            last = Some(i);
            if first {
                break;
            }
        }
        let next = (entry & !FLAG) as usize;
        if next == DEAD as usize {
            break;
        }
        s = next;
        i += 1;
    }
    Some((last, i))
}

/// The steady path over WTF-8 storage that is not wholly ASCII, from the
/// subject's start, decoding each scalar once.
#[inline(always)]
fn steady_wtf8(
    program: Program<'_>,
    bytes: &[u8],
    m: &mut [u32],
    budget: &mut Budget,
    first: bool,
    anchored: bool,
) -> Option<Option<usize>> {
    let need = m[H_NEED];
    let mode = if anchored {
        MODE_ANCHORED
    } else {
        MODE_UNANCHORED
    };
    let s = m[SLOTS + mode * 16 + (CX_EDGE & need) as usize] as usize;
    if s == 0 || budget.remaining() < bytes.len() {
        return None;
    }
    let (last, end) =
        if bytes.len() >= SKIP_MIN && !anchored && need == 0 && program.first_descriptor() != 0 {
            wtf8_skipping(program, bytes, s, m, first)?
        } else {
            wtf8_steady::<false>(program.unicode(), bytes, s, m, first, &NO_SKIP)?
        };
    budget.charge(end).ok()?;
    m[H_UNITS] = m[H_UNITS].saturating_add(end.min(u32::MAX as usize) as u32);
    Some(last)
}

/// The steady path over a long WTF-8 subject, skipping from the start state.
#[inline(never)]
fn wtf8_skipping(
    program: Program<'_>,
    bytes: &[u8],
    s: usize,
    m: &[u32],
    first: bool,
) -> Option<(Option<usize>, usize)> {
    let skip = skip_for(program, m, s, false);
    wtf8_steady::<true>(program.unicode(), bytes, s, m, first, &skip)
}

/// The steady loop over WTF-8 storage from the subject's start: the last
/// match end, and where the search stopped, in bytes.
#[inline(always)]
fn wtf8_steady<const SKIP: bool>(
    unicode: bool,
    bytes: &[u8],
    mut s: usize,
    m: &[u32],
    first: bool,
    skip: &Skip,
) -> Option<(Option<usize>, usize)> {
    let table: &[u32; 128] = m[ASCII..ASCII + 128].try_into().ok()?;
    let idle = if SKIP { skip.idle } else { usize::MAX };
    // Without a non-ASCII first character, every non-ASCII character keeps
    // the idle state where it is, so the skip passes over them with the
    // ASCII bytes the descriptor or the pairs reject.
    let wide = SKIP && m[H_ASCII_START] != 0;
    let memo = m[H_MEMO] as usize;
    let mut last = None;
    // `i` indexes bytes. A position in UTF-16 units is needed only for a match
    // end, so it is counted then, from the last one counted.
    let mut i = 0;
    // A search for the first match end (`first`) reports only that there is
    // one, so it counts nothing.
    let mut counted = (0, 0);
    let mut position = |i: usize| {
        if !first {
            counted = (i, counted.1 + utf16_units(&bytes[counted.0..i]));
        }
        counted.1
    };
    // The low half of a pair a search without `u` reads as two characters.
    let mut pending = 0;
    loop {
        if pending == 0 {
            while s == idle && i < bytes.len() {
                if wide {
                    i = skip_ascii::<true>(bytes, i, skip);
                    break;
                }
                // The descriptor speaks only for ASCII: the scan stops at
                // every non-ASCII lead byte, as the evaluator's mixed scan
                // does. A character whose cached transition keeps the idle
                // state where it is cannot begin a match either, so it is
                // passed over here; anything else is left to the loop below.
                i += scan_range::<true, true>(&bytes[i..], skip.lo, skip.hi)
                    .unwrap_or(bytes.len() - i);
                let Some(&lead) = bytes.get(i) else { break };
                if lead < 0x80 {
                    break;
                }
                let (point, width) = decode(bytes, i);
                if (0xd800..0xe000).contains(&point) || (point > 0xffff && !unicode) {
                    break;
                }
                match memo_column(m, memo, point) {
                    Some(column) if m[s + column] == s as u32 => i += width,
                    _ => break,
                }
            }
            // A run of ASCII bytes whose transitions are known, live and
            // unflagged.
            while let Some(&byte) = bytes.get(i)
                && byte < 0x80
                && s != idle
            {
                let entry = m[s + table[usize::from(byte)] as usize];
                if entry as i32 <= DEAD as i32 {
                    break;
                }
                s = entry as usize;
                i += 1;
            }
        }
        // The character's position is `here` in bytes, or one unit before it
        // for a pair's low half, whose encoding `i` has already passed.
        let (here, half) = (i, pending != 0);
        let c = if pending != 0 {
            core::mem::replace(&mut pending, 0)
        } else {
            let Some(&lead) = bytes.get(i) else {
                let entry = m[s];
                if entry == UNKNOWN {
                    return None;
                }
                if entry & FLAG != 0 {
                    last = Some(position(i));
                }
                break;
            };
            if lead < 0x80 {
                i += 1;
                u32::from(lead)
            } else {
                let (point, width) = decode(bytes, i);
                i += width;
                if point > 0xffff && !unicode {
                    pending = low_half(point);
                    high_half(point)
                } else if unicode && (0xd800..0xdc00).contains(&point) {
                    return None;
                } else {
                    point
                }
            }
        };
        let column = if c < 128 {
            table[c as usize] as usize
        } else {
            memo_column(m, memo, c)?
        };
        let entry = m[s + column];
        if entry == UNKNOWN {
            return None;
        }
        if entry & FLAG != 0 {
            if first {
                last = Some(0);
                break;
            }
            last = Some(position(here) - usize::from(half));
        }
        let next = (entry & !FLAG) as usize;
        if next == DEAD as usize {
            break;
        }
        s = next;
    }
    Some((last, i))
}

/// The scalar whose WTF-8 encoding begins with the non-ASCII byte at `i`, and
/// its width in bytes. Validated storage holds a whole encoding there.
#[inline(always)]
fn decode(bytes: &[u8], i: usize) -> (u32, usize) {
    let lead = bytes[i];
    let tail = |k: usize| bytes.get(i + k).map_or(0, |&b| u32::from(b & 0x3f));
    if lead < 0xe0 {
        ((u32::from(lead & 0x1f) << 6) | tail(1), 2)
    } else if lead < 0xf0 {
        ((u32::from(lead & 0x0f) << 12) | (tail(1) << 6) | tail(2), 3)
    } else {
        (
            (u32::from(lead & 0x07) << 18) | (tail(1) << 12) | (tail(2) << 6) | tail(3),
            4,
        )
    }
}

/// The UTF-16 units `bytes` encode, all of them whole WTF-8 sequences: one
/// for every byte that begins a sequence, and one more for every four-byte
/// one.
fn utf16_units(bytes: &[u8]) -> usize {
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let (words, tail) = bytes.as_chunks::<8>();
    let mut units = 0;
    for word in words {
        let w = u64::from_le_bytes(*word);
        // Continuation bytes are 10xxxxxx; four-byte leads are 11110xxx.
        let continuation = w & !(w << 1) & HIGH;
        let four = w & (w << 1) & (w << 2) & (w << 3) & HIGH;
        units += 8 - continuation.count_ones() as usize + four.count_ones() as usize;
    }
    for &byte in tail {
        units += usize::from(byte & 0xc0 != 0x80) + usize::from(byte >= 0xf0);
    }
    units
}

/// The column of a non-ASCII character in the first slot of its memo probe,
/// or `None` when it is not there.
#[inline(always)]
fn memo_column(m: &[u32], memo: usize, c: u32) -> Option<usize> {
    let at = memo + 2 * (hash2(c, 0x5bd1_e995) as usize & (MEMO_SLOTS - 1));
    (m[at] == c + 1).then(|| m[at + 1] as usize)
}

/// Whether `m` holds a cache built for a program of this length and header.
/// The words compared on every call are those that tell programs apart most
/// often; a debug build compares the whole header. Neither is proof of
/// identity, which is the host's contract.
#[inline(always)]
fn built_for(words: &[u32; HEADER], len: usize, m: &[u32]) -> bool {
    m.len() >= FRONT
        && m[H_MAGIC] == MAGIC
        && m[H_CAP] as usize == m.len()
        && m[H_LEN] as usize == len
        && m[H_PROGRAM] == words[2]
        && m[H_PROGRAM + 2] == words[4]
        && m[H_PROGRAM + 5] == words[7]
        && m[H_LAYOUT] == LAYOUT
        && (!cfg!(debug_assertions) || m[H_PROGRAM..H_PROGRAM + 9] == words[2..11])
}

struct Plan {
    required: usize,
    threads: usize,
    pred: usize,
    flags: usize,
    rep: usize,
    rps: usize,
    rpl: usize,
    reps: usize,
    memo: usize,
    sigtmp: usize,
    sigw: usize,
    table: usize,
    table_slots: usize,
    stack: usize,
    out: usize,
    saved: usize,
    sig: usize,
    sigcap: usize,
    hash: usize,
    hash_slots: usize,
    region: usize,
}

/// Where everything goes in `cap` words. `minimum` plans the smallest work
/// area, which is what [`minimum_words`] reports.
fn plan(
    n: usize,
    repeats: usize,
    predicates: usize,
    edges: usize,
    cap: usize,
    minimum: bool,
) -> Plan {
    let sigw = (PRED_BASE + predicates).div_ceil(32);
    let small = (n + 64).min(64);
    let threads = if minimum {
        small
    } else {
        // Repeat keys multiply the threads one instruction can hold, so the
        // limit is a multiple of the program; the storage caps it.
        (4 * (n + 64)).min((cap / 64).max(64))
    };
    let mut at = FRONT;
    let pred = take(&mut at, n);
    let flags = take(&mut at, n);
    let rep = take(&mut at, repeats);
    let rps = take(&mut at, n + 1);
    let rpl = take(&mut at, edges);
    let reps = take(&mut at, predicates);
    let memo = take(&mut at, 2 * MEMO_SLOTS);
    let sigtmp = take(&mut at, sigw);
    let table_slots = (4 * threads).next_power_of_two();
    let table = take(&mut at, 2 * table_slots);
    let stack = take(&mut at, 4 * threads);
    let out = take(&mut at, 2 * threads);
    let saved = take(&mut at, 2 * threads + 2);
    let fixed = at;
    let required = fixed + 16 * sigw + 64 + 8 * (FIRST_COLUMNS + 2 + 32);
    let rest = cap.saturating_sub(fixed);
    let sigcap = (rest / 8 / sigw).clamp(16, MAX_CLASSES);
    let sig = take(&mut at, sigcap * sigw);
    let rest = cap.saturating_sub(at);
    let hash_slots = prev_power_of_two(rest / 16).max(64);
    let hash = take(&mut at, hash_slots);
    Plan {
        required,
        threads,
        pred,
        flags,
        rep,
        rps,
        rpl,
        reps,
        memo,
        sigtmp,
        sigw,
        table,
        table_slots,
        stack,
        out,
        saved,
        sig,
        sigcap,
        hash,
        hash_slots,
        region: at,
    }
}

fn take(at: &mut usize, words: usize) -> usize {
    let start = *at;
    *at += words;
    start
}

fn prev_power_of_two(n: usize) -> usize {
    if n == 0 {
        0
    } else {
        1 << (usize::BITS - 1 - n.leading_zeros())
    }
}

/// The program's epsilon successors of `pc`, as `(count, [first, second])`.
/// A consuming instruction's step to the next one is not an epsilon edge.
fn successors(p: Program<'_>, pc: usize) -> (usize, [usize; 2]) {
    let [op, a, b] = p.instruction(pc);
    match op {
        SAVE | START | START_M | END | END_M | WORD | WORD_I | REPEAT_INIT | ATOM_REPEAT
        | REPEAT_BODY => (1, [pc + 1, 0]),
        JUMP => (1, [a as usize, 0]),
        SPLIT => (2, [a as usize, b as usize]),
        BRANCH => (2, [pc + 1, b as usize]),
        REPEAT_CHOICE => {
            let r = p.repeat(a as usize);
            (2, [r[3] as usize, r[4] as usize])
        }
        REPEAT_NEXT => (1, [p.repeat(a as usize)[3] as usize - 1, 0]),
        _ => (0, [0, 0]),
    }
}

fn hash2(a: u32, b: u32) -> u32 {
    (a.wrapping_mul(0x9e37_79b1) ^ b.wrapping_mul(0x85eb_ca77).rotate_left(13))
        .wrapping_mul(0xc2b2_ae3d)
}

/// Build the cache for `program` in `m`: tables derived from the program, and
/// no state yet.
fn build(program: Program<'_>, m: &mut [u32]) -> Result<(), Decline> {
    let n = program.instructions();
    let repeats = program.words[6] as usize;
    if m.len() >= FRONT {
        m[H_MAGIC] = 0;
    }
    // State offsets share a word with the match flag in bit 31.
    if m.len() >= MAX_WORDS {
        return Err(Decline::Capacity);
    }
    let (edges, consuming_count) = census(program);
    // Plan with every consuming instruction its own predicate first; the
    // deduplicated count only shrinks the signature. A work area sized by the
    // storage is preferred, the smallest one accepted.
    let mut minimum = false;
    let mut worst = plan(n, repeats, consuming_count, edges, m.len(), minimum);
    if m.len() < worst.required {
        minimum = true;
        worst = plan(n, repeats, consuming_count, edges, m.len(), minimum);
        if m.len() < worst.required {
            return Err(Decline::Storage {
                required_words: worst.required,
            });
        }
    }
    // Deduplicate predicates through a temporary table at the storage's end,
    // past everything the per-instruction tables use.
    let pred_at = worst.pred;
    let temp_slots = (2 * consuming_count).next_power_of_two().max(2);
    let temp_at = m.len().checked_sub(2 * temp_slots);
    let mut predicates = 0usize;
    let dedupe = temp_at.is_some_and(|at| at >= worst.reps + consuming_count);
    if dedupe {
        let at = temp_at.unwrap();
        m[at..].fill(0);
        for pc in 0..n {
            let [op, a, b] = program.instruction(pc);
            if !consuming(op) {
                m[pred_at + pc] = 0;
                continue;
            }
            let mut slot = hash2(hash2(op, a), b) as usize & (temp_slots - 1);
            loop {
                let rep = m[at + 2 * slot];
                if rep == 0 {
                    m[at + 2 * slot] = pc as u32 + 1;
                    m[at + 2 * slot + 1] = predicates as u32;
                    m[pred_at + pc] = predicates as u32 + 1;
                    predicates += 1;
                    break;
                }
                if program.instruction(rep as usize - 1) == [op, a, b] {
                    m[pred_at + pc] = m[at + 2 * slot + 1] + 1;
                    break;
                }
                slot = (slot + 1) & (temp_slots - 1);
            }
        }
    } else {
        for pc in 0..n {
            if consuming(program.instruction(pc)[0]) {
                predicates += 1;
                m[pred_at + pc] = predicates as u32;
            } else {
                m[pred_at + pc] = 0;
            }
        }
    }
    let p = plan(n, repeats, predicates, edges, m.len(), minimum);
    debug_assert_eq!(p.pred, pred_at);
    // Representative instruction of each predicate: its first.
    m[p.reps..p.reps + predicates].fill(u32::MAX);
    for pc in 0..n {
        let index = m[p.pred + pc] as usize;
        if index != 0 && m[p.reps + index - 1] == u32::MAX {
            m[p.reps + index - 1] = pc as u32;
        }
    }
    // Repeat fields and the empty-check bits each instruction's consumption
    // sets: those of every repeat whose body holds it.
    m[p.flags..p.flags + n].fill(0);
    m[p.rep..p.rep + repeats].fill(0);
    let ok = dfa_fields(program, |id, offset, bits| {
        m[p.rep + id] = offset | bits << 8;
        let r = program.repeat(id);
        let flag = 1u32 << (offset + bits);
        for pc in r[3] as usize + 1..r[4] as usize - 1 {
            m[p.flags + pc] |= flag;
        }
    });
    if !ok {
        return Err(Decline::Ineligible);
    }
    // Reverse epsilon edges, as compressed rows by target.
    m[p.rps..p.rps + n + 1].fill(0);
    for pc in 0..n {
        let (count, targets) = successors(program, pc);
        for &target in &targets[..count] {
            m[p.rps + target + 1] += 1;
        }
    }
    for i in 0..n {
        m[p.rps + i + 1] += m[p.rps + i];
    }
    for pc in 0..n {
        let (count, targets) = successors(program, pc);
        for &target in &targets[..count] {
            let at = m[p.rps + target] as usize;
            m[p.rpl + at] = pc as u32;
            m[p.rps + target] += 1;
        }
    }
    for i in (1..=n).rev() {
        m[p.rps + i] = m[p.rps + i - 1];
    }
    m[p.rps] = 0;
    // Context the program's assertions read.
    let mut need = 0;
    for pc in 0..n {
        need |= match program.instruction(pc)[0] {
            START | END => CX_EDGE,
            START_M | END_M => CX_EDGE | CX_LT,
            WORD => CX_WORD,
            WORD_I if program.unicode() => CX_WORD_UI,
            WORD_I => CX_WORD,
            _ => 0,
        };
    }
    let ascii_start = ascii_start(program, m, p.hash);
    m[ASCII..SLOTS].fill(UNCLASSIFIED);
    m[SLOTS..FRONT].fill(0);
    m[p.memo..p.memo + 2 * MEMO_SLOTS].fill(0);
    m[p.table..p.table + 2 * p.table_slots].fill(0);
    m[p.hash..p.hash + p.hash_slots].fill(0);
    let cap = m.len() as u32;
    let header = &mut m[..HEADER_WORDS];
    header.fill(0);
    header[H_LAYOUT] = LAYOUT;
    header[H_LEN] = program.words.len() as u32;
    header[H_PROGRAM..H_PROGRAM + 9].copy_from_slice(&program.words[2..11]);
    header[H_CAP] = cap;
    header[H_COLUMNS] = FIRST_COLUMNS as u32;
    header[H_PREDICATES] = predicates as u32;
    header[H_SIGW] = p.sigw as u32;
    header[H_NEED] = need;
    header[H_BUMP] = p.region as u32;
    header[H_PEAK] = p.region as u32;
    header[H_GEN] = 1;
    header[H_THREADS] = p.threads as u32;
    header[H_PRED] = p.pred as u32;
    header[H_FLAGS] = p.flags as u32;
    header[H_REP] = p.rep as u32;
    header[H_RPS] = p.rps as u32;
    header[H_RPL] = p.rpl as u32;
    header[H_REPS] = p.reps as u32;
    header[H_MEMO] = p.memo as u32;
    header[H_SIGTMP] = p.sigtmp as u32;
    header[H_TABLE] = p.table as u32;
    header[H_TABLE_SLOTS] = p.table_slots as u32;
    header[H_STACK] = p.stack as u32;
    header[H_OUT] = p.out as u32;
    header[H_SAVED] = p.saved as u32;
    header[H_SIG] = p.sig as u32;
    header[H_SIGCAP] = p.sigcap as u32;
    header[H_HASH] = p.hash as u32;
    header[H_HASH_SLOTS] = p.hash_slots as u32;
    header[H_REGION] = p.region as u32;
    header[H_ASCII_START] = u32::from(ascii_start);
    let pairs = match program.leading() {
        0 => Prefix {
            first: [0; LEADING_BRANCHES],
            second: [0; LEADING_BRANCHES],
            members: 0,
        },
        leading => prefix(program, leading),
    };
    header[H_PAIRS] = pairs.members as u32;
    for (k, chunk) in pairs
        .first
        .chunks(4)
        .chain(pairs.second.chunks(4))
        .enumerate()
    {
        header[H_PAIRS + 1 + k] = u32::from_le_bytes(chunk.try_into().unwrap_or([0; 4]));
    }
    header[H_MAGIC] = MAGIC;
    Ok(())
}

/// Whether every consuming instruction reachable from instruction zero
/// without consuming accepts only ASCII characters, so a character that is
/// not ASCII can begin no match. Counters are ignored, which can only admit
/// more instructions. The walk uses the storage from `free` on as scratch,
/// and answers no when it does not fit there.
fn ascii_start(program: Program<'_>, m: &mut [u32], free: usize) -> bool {
    let n = program.instructions();
    let Some(seen) = m.len().checked_sub(2 * n).filter(|&at| at >= free) else {
        return false;
    };
    let stack = seen + n;
    m[seen..stack].fill(0);
    m[seen] = 1;
    m[stack] = 0;
    let mut depth = 1;
    while depth != 0 {
        depth -= 1;
        let pc = m[stack + depth] as usize;
        let [op, a, b] = program.instruction(pc);
        if consuming(op) {
            let ascii = match op {
                CHAR => a < 128,
                CHAR_I => casefold::equivalents(a, program.unicode())
                    .iter()
                    .all(|&c| c < 128),
                CLASS | CLASS_SORTED => {
                    b & NEGATED == 0
                        && (a..a + b).all(|index| {
                            let [lo, hi] = program.range(index as usize);
                            lo & PROPERTY == 0 && hi < 128
                        })
                }
                _ => false,
            };
            if !ascii {
                return false;
            }
            continue;
        }
        let (count, targets) = successors(program, pc);
        for &target in &targets[..count] {
            if m[seen + target] == 0 {
                m[seen + target] = 1;
                m[stack + depth] = target as u32;
                depth += 1;
            }
        }
    }
    true
}

fn high_half(point: u32) -> u32 {
    0xd800 | ((point - 0x10000) >> 10)
}

fn low_half(point: u32) -> u32 {
    0xdc00 | ((point - 0x10000) & 0x3ff)
}

fn line_terminator(c: u32) -> bool {
    matches!(c, 10 | 13 | 0x2028 | 0x2029)
}

fn read(cursor: &mut Cursor<'_>, unicode: bool, reverse: bool) -> Option<u32> {
    match (unicode, reverse) {
        (true, false) => cursor.next_point(),
        (true, true) => cursor.previous_point(),
        (false, false) => cursor.next_unit().map(u32::from),
        (false, true) => cursor.previous_unit().map(u32::from),
    }
}

/// Whether a consuming instruction accepts `c`, by the evaluator's rules.
fn accepts(program: Program<'_>, [op, a, b]: [u32; 3], c: u32) -> bool {
    match op {
        CHAR => c == a,
        CHAR_I => c == a || casefold::equal(c, a, program.unicode()),
        ANY => !line_terminator(c),
        ANY_S => true,
        CLASS | CLASS_SORTED => class(program, a, b, [c; 4], op == CLASS_SORTED),
        CLASS_I | CLASS_SORTED_I => class(
            program,
            a,
            b,
            casefold::equivalents(c, program.unicode()),
            op == CLASS_SORTED_I,
        ),
        _ => false,
    }
}

/// Class membership for a character's case equivalents, as the evaluator's
/// class step decides it, including a `v` complement's whole-closure rule.
fn class(program: Program<'_>, a: u32, b: u32, values: [u32; 4], sorted: bool) -> bool {
    let (start, end, negated) = (a, a + (b & !NEGATED), b & NEGATED != 0);
    let found = if sorted {
        values.iter().any(|&value| {
            let (mut low, mut high) = (start, end);
            while low < high {
                let middle = low + (high - low) / 2;
                let [lo, hi] = program.range(middle as usize);
                if value < lo {
                    high = middle;
                } else if value > hi {
                    low = middle + 1;
                } else {
                    return true;
                }
            }
            false
        })
    } else {
        (start..end).any(|index| {
            let [lo, hi] = program.range(index as usize);
            if lo & PROPERTY != 0 && hi == 2 {
                return !values
                    .iter()
                    .any(|&value| properties::contains(lo & !PROPERTY, value));
            }
            values.iter().any(|&value| {
                if lo & PROPERTY != 0 {
                    properties::contains(lo & !PROPERTY, value) != (hi != 0)
                } else {
                    value >= lo && value <= hi
                }
            })
        })
    };
    found != negated
}

/// Whether a position assertion holds between `before` and `after`.
fn holds(op: u32, a: u32, before: u32, after: u32, unicode: bool) -> bool {
    match op {
        START => before & CX_EDGE != 0,
        START_M => before & (CX_EDGE | CX_LT) != 0,
        END => after & CX_EDGE != 0,
        END_M => after & (CX_EDGE | CX_LT) != 0,
        WORD | WORD_I => {
            let bit = if op == WORD_I && unicode {
                CX_WORD_UI
            } else {
                CX_WORD
            };
            ((before & bit != 0) != (after & bit != 0)) != (a != 0)
        }
        _ => false,
    }
}

/// The context bits of one character.
fn context(c: u32) -> u32 {
    (if line_terminator(c) { CX_LT } else { 0 })
        | (if casefold::word(c, false) { CX_WORD } else { 0 })
        | (if casefold::word(c, true) {
            CX_WORD_UI
        } else {
            0
        })
}

struct Dfa<'p, 'c> {
    program: Program<'p>,
    m: &'c mut [u32],
    budget: Budget,
    /// Characters this search has read, and how many of them the header's
    /// count since the last clear already holds.
    progress: usize,
    flushed: usize,
    unicode: bool,
}

impl Dfa<'_, '_> {
    #[inline(always)]
    fn h(&self, index: usize) -> usize {
        self.m[index] as usize
    }

    fn charge(&mut self, work: usize) -> Result<(), Stop> {
        self.budget
            .charge(work)
            .map_err(|_| Stop::Error(ExecError::WorkLimit))
    }

    #[inline(always)]
    fn flush_units(&mut self) {
        if self.progress == self.flushed {
            return;
        }
        let total = (self.m[H_UNITS] as usize).saturating_add(self.progress - self.flushed);
        self.m[H_UNITS] = total.min(u32::MAX as usize) as u32;
        self.flushed = self.progress;
    }

    #[inline(always)]
    fn run(&mut self, input: Input<'_>, start: usize, bounds: bool) -> Result<Outcome, Stop> {
        let mut cursor = input.cursor_at(start).ok_or(ExecError::InvalidProgram)?;
        if self.unicode && start != 0 {
            cursor.normalize_unicode_start();
        }
        let start = cursor.position();
        let anchored = self.program.words[2] & Y != 0 || self.program.start_anchored();
        let Some(end) = self.forward(input, cursor, anchored, !bounds)? else {
            return Ok(Outcome::NoMatch);
        };
        if !bounds {
            return Ok(Outcome::Matched(None));
        }
        let begin = if anchored {
            start
        } else {
            self.reverse(input, end, start)?
        };
        Ok(Outcome::Matched(Span::new(begin, end)))
    }

    /// The context of the character before `cursor`, as the assertions read
    /// it: `previous_unit` for a line terminator, the evaluator's own read for
    /// a word character.
    #[inline(always)]
    fn before(&self, cursor: Cursor<'_>) -> u32 {
        let need = self.m[H_NEED];
        if need == 0 {
            return 0;
        }
        if cursor.position() == 0 {
            return CX_EDGE & need;
        }
        self.before_character(cursor)
    }

    #[inline(never)]
    fn before_character(&self, cursor: Cursor<'_>) -> u32 {
        let mut unit = cursor;
        let mut point = cursor;
        let lt = unit
            .previous_unit()
            .is_some_and(|u| line_terminator(u32::from(u)));
        let c = read(&mut point, self.unicode, true).unwrap_or(0);
        let bits = (context(c) & !CX_LT) | if lt { CX_LT } else { 0 };
        bits & self.m[H_NEED]
    }

    fn after(&self, cursor: Cursor<'_>, len: usize) -> u32 {
        if cursor.position() == len {
            return CX_EDGE & self.m[H_NEED];
        }
        let mut unit = cursor;
        let mut point = cursor;
        let lt = unit
            .next_unit()
            .is_some_and(|u| line_terminator(u32::from(u)));
        let c = read(&mut point, self.unicode, false).unwrap_or(0);
        let bits = (context(c) & !CX_LT) | if lt { CX_LT } else { 0 };
        bits & self.m[H_NEED]
    }

    /// The end of the leftmost-first match at or after `cursor`, or of the
    /// first match end found when `first` is set.
    #[inline(always)]
    fn forward(
        &mut self,
        input: Input<'_>,
        mut cursor: Cursor<'_>,
        anchored: bool,
        first: bool,
    ) -> Result<Option<usize>, Stop> {
        let ctx = self.before(cursor);
        let mode = if anchored {
            MODE_ANCHORED
        } else {
            MODE_UNANCHORED
        };
        let mut s = self.start_state(mode, ctx)?;
        let mut last = None;
        let allowance = self.budget.remaining();
        if let Some(bytes) = input.ascii_bytes() {
            let begin = cursor.position();
            let base = self.progress;
            let limit = bytes.len().min(begin.saturating_add(allowance));
            let mut i = begin;
            loop {
                let Some(&byte) = bytes.get(i) else {
                    let mut entry = self.m[s as usize];
                    if entry == UNKNOWN {
                        self.progress = base + (i - begin);
                        entry = self.step_slow(&mut s, None)?;
                    }
                    if entry & FLAG != 0 {
                        last = Some(i);
                    }
                    break;
                };
                if i == limit {
                    self.progress = base + (i - begin);
                    return Err(Stop::Error(ExecError::WorkLimit));
                }
                let column = self.m[ASCII + byte as usize] as usize;
                let mut entry = self.m[s as usize + column];
                if entry == UNKNOWN {
                    self.progress = base + (i - begin);
                    entry = self.step_slow(&mut s, Some(u32::from(byte)))?;
                }
                if entry & FLAG != 0 {
                    last = Some(i);
                    if first {
                        break;
                    }
                }
                let next = entry & !FLAG;
                if next == DEAD {
                    break;
                }
                s = next;
                i += 1;
            }
            self.progress = base + (i - begin);
            self.charge(i - begin)?;
            return Ok(last);
        }
        // Other storage is read a code point at a time, which decodes a
        // four-byte scalar once; without `u` its two halves are then taken
        // as the two characters they are.
        let base = self.progress;
        let mut read_units = 0;
        let mut at = cursor.position();
        let mut pending = None;
        loop {
            let c = match pending.take() {
                Some(low) => Some(low),
                None => match cursor.next_point() {
                    Some(point) if point > 0xffff && !self.unicode => {
                        pending = Some(low_half(point));
                        Some(high_half(point))
                    }
                    other => other,
                },
            };
            if c.is_some() {
                read_units += 1;
                if read_units > allowance {
                    return Err(Stop::Error(ExecError::WorkLimit));
                }
            }
            let entry = match self.cached(s, c) {
                UNKNOWN => {
                    self.progress = base + read_units;
                    self.step_slow(&mut s, c)?
                }
                entry => entry,
            };
            if entry & FLAG != 0 {
                last = Some(at);
                if first {
                    break;
                }
            }
            let next = entry & !FLAG;
            let Some(c) = c else { break };
            if next == DEAD {
                break;
            }
            s = next;
            at += if c > 0xffff { 2 } else { 1 };
        }
        self.progress = base + read_units;
        self.charge(read_units)?;
        Ok(last)
    }

    /// The leftmost start, at or after `lower`, of a match ending at `end`.
    fn reverse(&mut self, input: Input<'_>, end: usize, lower: usize) -> Result<usize, Stop> {
        let mut cursor = input.cursor_at(end).ok_or(ExecError::InvalidProgram)?;
        let ctx = self.after(cursor, input.len_utf16());
        let mut s = self.start_state(MODE_REVERSE, ctx)?;
        let mut found = None;
        let base = self.progress;
        let mut read_units = 0;
        let mut at = end;
        let mut pending = None;
        loop {
            let c = match pending.take() {
                Some(high) => Some(high),
                None if at == 0 => None,
                None => match cursor.previous_point() {
                    Some(point) if point > 0xffff && !self.unicode => {
                        pending = Some(high_half(point));
                        Some(low_half(point))
                    }
                    other => other,
                },
            };
            read_units += 1;
            let entry = match self.cached(s, c) {
                UNKNOWN => {
                    self.progress = base + read_units;
                    self.step_slow(&mut s, c)?
                }
                entry => entry,
            };
            if entry & FLAG != 0 {
                found = Some(at);
            }
            let next = entry & !FLAG;
            let Some(c) = c else { break };
            if at <= lower || next == DEAD {
                break;
            }
            s = next;
            at -= if c > 0xffff { 2 } else { 1 };
        }
        self.progress = base + read_units;
        self.charge(read_units)?;
        found.ok_or(Stop::Error(ExecError::InvalidProgram))
    }

    /// The cached transition from `s` on `c`, or `UNKNOWN`: a character whose
    /// class is in the ASCII table or the first slot of its memo probe.
    #[inline(always)]
    fn cached(&self, s: u32, c: Option<u32>) -> u32 {
        let column = match c {
            None => 0,
            Some(c) if c < 128 => self.m[ASCII + c as usize] as usize,
            Some(c) => {
                let at = self.h(H_MEMO) + 2 * (hash2(c, 0x5bd1_e995) as usize & (MEMO_SLOTS - 1));
                if self.m[at] != c + 1 {
                    return UNKNOWN;
                }
                self.m[at + 1] as usize
            }
        };
        self.m[s as usize + column]
    }

    /// The transition from `s` on `c`, or on the subject's edge for `None`,
    /// when [`Dfa::cached`] does not have it: finding the class, computing the
    /// transition. A clear while computing it moves `s`.
    #[inline(never)]
    fn step_slow(&mut self, s: &mut u32, c: Option<u32>) -> Result<u32, Stop> {
        let column = match c {
            None => 0,
            Some(c) => self.column(c, s)?,
        };
        let entry = self.m[*s as usize + column];
        if entry != UNKNOWN {
            return Ok(entry);
        }
        self.compute(s, column)
    }

    /// The class of `c`, assigning a new one on first sight.
    fn column(&mut self, c: u32, s: &mut u32) -> Result<usize, Stop> {
        if c < 128 {
            let known = self.m[ASCII + c as usize];
            if known != UNCLASSIFIED {
                return Ok(known as usize);
            }
        } else {
            let memo = self.h(H_MEMO);
            let mut slot = hash2(c, 0x5bd1_e995) as usize & (MEMO_SLOTS - 1);
            loop {
                let key = self.m[memo + 2 * slot];
                if key == c + 1 {
                    return Ok(self.m[memo + 2 * slot + 1] as usize);
                }
                if key == 0 {
                    break;
                }
                slot = (slot + 1) & (MEMO_SLOTS - 1);
            }
        }
        let class = self.classify(c, s)?;
        if c < 128 {
            self.m[ASCII + c as usize] = class as u32;
        } else {
            let memo = self.h(H_MEMO);
            if self.m[H_MEMO_USED] as usize >= MEMO_SLOTS * 3 / 4 {
                self.m[memo..memo + 2 * MEMO_SLOTS].fill(0);
                self.m[H_MEMO_USED] = 0;
            }
            let mut slot = hash2(c, 0x5bd1_e995) as usize & (MEMO_SLOTS - 1);
            while self.m[memo + 2 * slot] != 0 {
                slot = (slot + 1) & (MEMO_SLOTS - 1);
            }
            self.m[memo + 2 * slot] = c + 1;
            self.m[memo + 2 * slot + 1] = class as u32;
            self.m[H_MEMO_USED] += 1;
        }
        Ok(class)
    }

    /// Evaluate every predicate on `c` and find or add its class.
    fn classify(&mut self, c: u32, s: &mut u32) -> Result<usize, Stop> {
        let sigw = self.h(H_SIGW);
        let tmp = self.h(H_SIGTMP);
        let predicates = self.h(H_PREDICATES);
        let reps = self.h(H_REPS);
        self.charge(predicates + 1)?;
        self.m[tmp..tmp + sigw].fill(0);
        self.m[tmp] = context(c) & self.m[H_NEED] & !CX_EDGE;
        for index in 0..predicates {
            let pc = self.h(reps + index);
            if accepts(self.program, self.program.instruction(pc), c) {
                let bit = PRED_BASE + index;
                self.m[tmp + bit / 32] |= 1 << (bit % 32);
            }
        }
        let sig = self.h(H_SIG);
        let classes = self.h(H_CLASSES);
        for class in 0..classes {
            let at = sig + class * sigw;
            if self.m[at..at + sigw] == self.m[tmp..tmp + sigw] {
                return Ok(class + 2);
            }
        }
        if classes >= self.h(H_SIGCAP) {
            return Err(Stop::Declined(Decline::Capacity));
        }
        let at = sig + classes * sigw;
        self.m.copy_within(tmp..tmp + sigw, at);
        self.m[H_CLASSES] = classes as u32 + 1;
        let class = classes + 2;
        if class >= self.h(H_COLUMNS) {
            // Rows are as wide as the classes; widen them all, which drops
            // every state, and rebuild the current one.
            let columns = (self.h(H_COLUMNS) * 2).min(MAX_CLASSES + 2);
            self.save(*s);
            self.m[H_COLUMNS] = columns as u32;
            self.clear(false)?;
            *s = self.restore()?;
        }
        Ok(class)
    }

    /// Copy state `s`'s header and threads to the saved area.
    fn save(&mut self, s: u32) {
        let columns = self.h(H_COLUMNS);
        let at = s as usize + columns;
        let count = self.h(at + 1);
        let saved = self.h(H_SAVED);
        self.m.copy_within(at..at + 2 + 2 * count, saved);
    }

    /// Intern the saved state again after a clear.
    fn restore(&mut self) -> Result<u32, Stop> {
        let saved = self.h(H_SAVED);
        let header = self.m[saved];
        let count = self.h(saved + 1);
        match self.intern(header, saved + 2, count)? {
            Some(s) => Ok(s),
            None => Err(Stop::Declined(Decline::Capacity)),
        }
    }

    /// Drop every state. `space` marks a clear forced by a full cache, which
    /// counts towards thrashing.
    fn clear(&mut self, space: bool) -> Result<(), Stop> {
        self.flush_units();
        if space {
            let units = self.m[H_UNITS];
            let built = self.m[H_BUILT];
            if units < built.saturating_mul(UNITS_PER_STATE) {
                self.m[H_BAD] += 1;
            } else {
                self.m[H_BAD] = 0;
            }
            if self.m[H_BAD] >= THRASH_LIMIT {
                self.m[H_STATUS] = 1;
            }
            self.m[H_CLEARS] = self.m[H_CLEARS].saturating_add(1);
        }
        self.m[H_UNITS] = 0;
        self.m[H_BUILT] = 0;
        self.m[H_STATES] = 0;
        self.m[H_BUMP] = self.m[H_REGION];
        self.m[SLOTS..FRONT].fill(0);
        let hash = self.h(H_HASH);
        let slots = self.h(H_HASH_SLOTS);
        self.m[hash..hash + slots].fill(0);
        if self.m[H_STATUS] != 0 {
            return Err(Stop::Declined(Decline::Disabled));
        }
        Ok(())
    }

    /// The state with `header` and the `count` threads at `threads`, built if
    /// new. `None` when the cache has no room for it.
    fn intern(&mut self, header: u32, threads: usize, count: usize) -> Result<Option<u32>, Stop> {
        if count == 0 && header & S_INJECT == 0 {
            return Ok(Some(DEAD));
        }
        let mut h = hash2(header, count as u32);
        for i in 0..2 * count {
            h = hash2(h, self.m[threads + i]);
        }
        let columns = self.h(H_COLUMNS);
        let hash = self.h(H_HASH);
        let slots = self.h(H_HASH_SLOTS);
        let mut slot = h as usize & (slots - 1);
        loop {
            let s = self.h(hash + slot);
            if s == 0 {
                break;
            }
            let at = s + columns;
            if self.m[at] == header
                && self.h(at + 1) == count
                && self.m[at + 2..at + 2 + 2 * count] == self.m[threads..threads + 2 * count]
            {
                return Ok(Some(s as u32));
            }
            slot = (slot + 1) & (slots - 1);
        }
        let size = columns + 2 + 2 * count;
        let s = self.h(H_BUMP);
        if s + size > self.m.len() || self.h(H_STATES) + 1 > slots / 2 {
            return Ok(None);
        }
        self.charge(size / 8 + 1)?;
        self.m[s..s + columns].fill(UNKNOWN);
        self.m[s + columns] = header;
        self.m[s + columns + 1] = count as u32;
        self.m
            .copy_within(threads..threads + 2 * count, s + columns + 2);
        self.m[hash + slot] = s as u32;
        self.m[H_BUMP] = (s + size) as u32;
        if self.m[H_PEAK] < self.m[H_BUMP] {
            self.m[H_PEAK] = self.m[H_BUMP];
        }
        self.m[H_STATES] += 1;
        self.m[H_BUILT] += 1;
        Ok(Some(s as u32))
    }

    #[inline(always)]
    fn start_state(&mut self, mode: usize, ctx: u32) -> Result<u32, Stop> {
        let slot = SLOTS + mode * 16 + ctx as usize;
        let known = self.m[slot];
        if known != 0 {
            return Ok(known);
        }
        self.build_start(mode, ctx)
    }

    #[inline(never)]
    fn build_start(&mut self, mode: usize, ctx: u32) -> Result<u32, Stop> {
        let slot = SLOTS + mode * 16 + ctx as usize;
        let saved = self.h(H_SAVED);
        let (header, count) = match mode {
            MODE_UNANCHORED => (S_INJECT | ctx << 4, 0),
            MODE_ANCHORED => {
                self.m[saved + 2] = 0;
                self.m[saved + 3] = 0;
                (ctx << 4, 1)
            }
            _ => {
                self.m[saved + 2] = self.program.instructions() as u32 - 1;
                self.m[saved + 3] = 0;
                (S_REVERSE | ctx << 4, 1)
            }
        };
        self.m[saved] = header;
        self.m[saved + 1] = count;
        let s = match self.intern(header, saved + 2, count as usize)? {
            Some(s) => s,
            None => {
                self.clear(true)?;
                self.restore()?
            }
        };
        self.m[slot] = s;
        Ok(s)
    }

    /// Begin a closure: a fresh visited generation.
    fn next_generation(&mut self) {
        let mut generation = self.m[H_GEN] + 1;
        if generation >= 128 {
            let table = self.h(H_TABLE);
            let slots = self.h(H_TABLE_SLOTS);
            self.m[table..table + 2 * slots].fill(0);
            generation = 1;
        }
        self.m[H_GEN] = generation;
    }

    /// Mark `(pc, key)` visited; false when it already was in this generation.
    #[inline]
    fn visit(&mut self, pc: u32, key: u32, kind: u32, visited: &mut usize) -> Result<bool, Stop> {
        let table = self.h(H_TABLE);
        let slots = self.h(H_TABLE_SLOTS);
        let tag = self.m[H_GEN] << GEN_SHIFT;
        let word = pc | tag | kind;
        let mut slot = hash2(pc | kind, key) as usize & (slots - 1);
        loop {
            let at = table + 2 * slot;
            let current = self.m[at];
            if current & !(OUT_KEY | PC_MASK) != tag {
                if *visited >= slots / 2 {
                    return Err(Stop::Declined(Decline::Capacity));
                }
                *visited += 1;
                self.m[at] = word;
                self.m[at + 1] = key;
                return Ok(true);
            }
            if current == word && self.m[at + 1] == key {
                return Ok(false);
            }
            slot = (slot + 1) & (slots - 1);
        }
    }

    fn field(&self, id: u32) -> (u32, u32) {
        let packed = self.m[self.h(H_REP) + id as usize];
        (packed & 255, (packed >> 8) & 255)
    }

    /// Compute the transition from `s` on column `column` (zero: the edge),
    /// store it, and return it. A clear while storing it moves `s`.
    fn compute(&mut self, s: &mut u32, column: usize) -> Result<u32, Stop> {
        let columns = self.h(H_COLUMNS);
        let at = *s as usize + columns;
        let header = self.m[at];
        let count = self.h(at + 1);
        let reverse = header & S_REVERSE != 0;
        let inject = header & S_INJECT != 0;
        let ctx = (header >> 4) & 15;
        let need = self.m[H_NEED];
        let (side, sig) = if column == 0 {
            (CX_EDGE & need, 0)
        } else {
            let sig = self.h(H_SIG) + (column - 2) * self.h(H_SIGW);
            (self.m[sig] & need, sig)
        };
        let (before, after) = if reverse { (side, ctx) } else { (ctx, side) };
        let limit = self.h(H_THREADS);
        let stack = self.h(H_STACK);
        let out = self.h(H_OUT);
        let pred = self.h(H_PRED);
        let flags_at = self.h(H_FLAGS);
        let unicode = self.unicode;
        let program = self.program;
        self.next_generation();
        let mut visited = 0usize;
        let mut depth;
        let mut produced = 0usize;
        let mut flagged = false;
        let mut work = 0usize;
        let seeds = count + usize::from(inject);
        'seeds: for seed in 0..seeds {
            let (pc, key) = if seed < count {
                (self.m[at + 2 + 2 * seed], self.m[at + 3 + 2 * seed])
            } else {
                (0, 0)
            };
            self.m[stack] = pc;
            self.m[stack + 1] = key;
            depth = 1;
            while depth > 0 {
                depth -= 1;
                let pc = self.m[stack + 2 * depth];
                let key = self.m[stack + 2 * depth + 1];
                if !self.visit(pc, key, 0, &mut visited)? {
                    continue;
                }
                work += 1;
                let mut push = |m: &mut [u32], pc: u32, key: u32| -> Result<(), Stop> {
                    if depth >= 2 * limit {
                        return Err(Stop::Declined(Decline::Capacity));
                    }
                    m[stack + 2 * depth] = pc;
                    m[stack + 2 * depth + 1] = key;
                    depth += 1;
                    Ok(())
                };
                if reverse {
                    if pc == 0 {
                        flagged = true;
                    }
                    // A consuming instruction just before `pc` reads the
                    // character before this position.
                    if column != 0 && pc > 0 {
                        let index = self.m[pred + pc as usize - 1];
                        if index != 0 {
                            let bit = PRED_BASE + index as usize - 1;
                            if self.m[sig + bit / 32] & (1 << (bit % 32)) != 0
                                && self.visit(pc - 1, key, OUT_KEY, &mut visited)?
                            {
                                if produced >= limit {
                                    return Err(Stop::Declined(Decline::Capacity));
                                }
                                self.m[out + 2 * produced] = pc - 1;
                                self.m[out + 2 * produced + 1] = key;
                                produced += 1;
                            }
                        }
                    }
                    let rps = self.h(H_RPS);
                    let rpl = self.h(H_RPL);
                    let (from, to) = (self.h(rps + pc as usize), self.h(rps + pc as usize + 1));
                    for e in from..to {
                        let q = self.m[rpl + e];
                        let [op, a, _] = program.instruction(q as usize);
                        match op {
                            SAVE | JUMP | SPLIT | BRANCH | REPEAT_BODY => push(self.m, q, key)?,
                            START | START_M | END | END_M | WORD | WORD_I => {
                                if holds(op, a, before, after, unicode) {
                                    push(self.m, q, key)?;
                                }
                            }
                            REPEAT_INIT | ATOM_REPEAT => {
                                let (offset, bits) = self.field(a);
                                let k = (key >> offset) & ((1 << bits) - 1);
                                let r = program.repeat(a as usize);
                                if k >= r[0] && (r[2] & 1 != 0 || k <= r[1]) {
                                    let mask = ((1u32 << (bits + 1)) - 1) << offset;
                                    push(self.m, q, key & !mask)?;
                                }
                            }
                            REPEAT_CHOICE => {
                                let r = program.repeat(a as usize);
                                if pc == r[4] {
                                    let (offset, bits) = self.field(a);
                                    let mask = ((1u32 << (bits + 1)) - 1) << offset;
                                    push(self.m, q, key & !mask)?;
                                } else {
                                    push(self.m, q, key)?;
                                }
                            }
                            REPEAT_NEXT => {
                                let (offset, bits) = self.field(a);
                                let count_mask = ((1u32 << bits) - 1) << offset;
                                let k = (key & count_mask) >> offset;
                                let r = program.repeat(a as usize);
                                let next = if r[2] & 1 != 0 {
                                    (k + 1).min(r[0])
                                } else {
                                    k + 1
                                };
                                if r[2] & 1 != 0 || next <= r[1] {
                                    push(self.m, q, (key & !count_mask) | (next << offset))?;
                                }
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                let [op, a, b] = program.instruction(pc as usize);
                match op {
                    MATCH => {
                        flagged = true;
                        break 'seeds;
                    }
                    SAVE => push(self.m, pc + 1, key)?,
                    JUMP => push(self.m, a, key)?,
                    SPLIT => {
                        push(self.m, b, key)?;
                        push(self.m, a, key)?;
                    }
                    BRANCH => {
                        push(self.m, b, key)?;
                        push(self.m, pc + 1, key)?;
                    }
                    START | START_M | END | END_M | WORD | WORD_I => {
                        if holds(op, a, before, after, unicode) {
                            push(self.m, pc + 1, key)?;
                        }
                    }
                    REPEAT_INIT | ATOM_REPEAT => {
                        let (offset, bits) = self.field(a);
                        let mask = ((1u32 << (bits + 1)) - 1) << offset;
                        push(self.m, pc + 1, key & !mask)?;
                    }
                    REPEAT_CHOICE => {
                        let r = program.repeat(a as usize);
                        let (offset, bits) = self.field(a);
                        let k = (key >> offset) & ((1 << bits) - 1);
                        let mask = ((1u32 << (bits + 1)) - 1) << offset;
                        let exit = key & !mask;
                        if r[2] & 1 == 0 && k >= r[1] {
                            push(self.m, r[4], exit)?;
                        } else if k >= r[0] {
                            if r[2] & 2 == 0 {
                                push(self.m, r[4], exit)?;
                                push(self.m, pc + 1, key)?;
                            } else {
                                push(self.m, r[3], key)?;
                                push(self.m, r[4], exit)?;
                            }
                        } else {
                            push(self.m, pc + 1, key)?;
                        }
                    }
                    REPEAT_BODY => {
                        let (offset, bits) = self.field(a);
                        push(self.m, pc + 1, key & !(1 << (offset + bits)))?;
                    }
                    REPEAT_NEXT => {
                        let r = program.repeat(a as usize);
                        let (offset, bits) = self.field(a);
                        let count_mask = ((1u32 << bits) - 1) << offset;
                        let k = (key & count_mask) >> offset;
                        let consumed = key & (1 << (offset + bits)) != 0;
                        if consumed || k < r[0] {
                            let next = if r[2] & 1 != 0 {
                                (k + 1).min(r[0])
                            } else {
                                k + 1
                            };
                            push(
                                self.m,
                                r[3] - 1,
                                (key & !count_mask) | ((next << offset) & count_mask),
                            )?;
                        }
                    }
                    _ if column != 0 => {
                        let index = self.m[pred + pc as usize];
                        if index == 0 {
                            return Err(Stop::Error(ExecError::InvalidProgram));
                        }
                        let bit = PRED_BASE + index as usize - 1;
                        if self.m[sig + bit / 32] & (1 << (bit % 32)) != 0 {
                            let next = key | self.m[flags_at + pc as usize];
                            if self.visit(pc + 1, next, OUT_KEY, &mut visited)? {
                                if produced >= limit {
                                    return Err(Stop::Declined(Decline::Capacity));
                                }
                                self.m[out + 2 * produced] = pc + 1;
                                self.m[out + 2 * produced + 1] = next;
                                produced += 1;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        self.charge(work)?;
        let entry = if column == 0 {
            DEAD
        } else {
            let header = if reverse {
                S_REVERSE
            } else if inject && !flagged {
                S_INJECT
            } else {
                0
            } | side << 4;
            match self.intern(header, out, produced)? {
                Some(next) => next,
                None => {
                    // Full: keep this state and its successor, drop the rest.
                    self.save(*s);
                    self.clear(true)?;
                    *s = self.restore()?;
                    match self.intern(header, out, produced)? {
                        Some(next) => next,
                        None => return Err(Stop::Declined(Decline::Capacity)),
                    }
                }
            }
        };
        let entry = entry | if flagged { FLAG } else { 0 };
        self.m[*s as usize + column] = entry;
        Ok(entry)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::compiler::{Node, Range, compile};
    use std::{vec, vec::Vec};

    fn program(pattern: &str, flags: &str) -> Vec<u32> {
        let source = Input::utf8(pattern);
        let mut nodes = vec![Node::default(); 256];
        let mut ranges = vec![Range::default(); 1024];
        let mut words = vec![0; 4096];
        let p = compile(
            source,
            flags,
            &mut nodes,
            &mut ranges,
            &mut words,
            &mut Budget::new(10_000_000),
        )
        .unwrap();
        p.words().to_vec()
    }

    /// Generalized UTF-8: surrogates are encoded as three bytes each.
    fn wtf8(points: &[u32]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for &c in points {
            match c {
                0..0x80 => bytes.push(c as u8),
                0x80..0x800 => bytes.extend([0xc0 | (c >> 6) as u8, 0x80 | (c & 0x3f) as u8]),
                0x800..0x10000 => bytes.extend([
                    0xe0 | (c >> 12) as u8,
                    0x80 | ((c >> 6) & 0x3f) as u8,
                    0x80 | (c & 0x3f) as u8,
                ]),
                _ => bytes.extend([
                    0xf0 | (c >> 18) as u8,
                    0x80 | ((c >> 12) & 0x3f) as u8,
                    0x80 | ((c >> 6) & 0x3f) as u8,
                    0x80 | (c & 0x3f) as u8,
                ]),
            }
        }
        bytes
    }

    #[test]
    fn the_steady_decoder_reads_every_scalar() {
        for c in 0x80..=0x10ffff {
            let bytes = wtf8(&[c]);
            assert_eq!(decode(&bytes, 0), (c, bytes.len()), "U+{c:04X}");
        }
    }

    #[test]
    fn utf16_units_count_every_encoding() {
        let points = [0x41, 0xe9, 0x4e2d, 0xd83d, 0xde00, 0x1f600, 0x7f, 0x10ffff];
        for from in 0..points.len() {
            for to in from..=points.len() {
                let units: usize = points[from..to]
                    .iter()
                    .map(|&c| if c > 0xffff { 2 } else { 1 })
                    .sum();
                let mut bytes = wtf8(&points[from..to]);
                assert_eq!(utf16_units(&bytes), units);
                // Long enough for the word loop.
                bytes = bytes.repeat(5);
                assert_eq!(utf16_units(&bytes), 5 * units);
            }
        }
    }

    /// Characters of two, three and four bytes, and a pair encoded as two
    /// separate surrogates, each landing on a class the cache already holds,
    /// are classified by the steady path itself: it answers without leaving
    /// to the general path, and its answer is the class's. A misdecoded value
    /// either misses the memo, and the steady path declines, or lands on
    /// another class, and the answer changes; both fail here.
    #[test]
    fn the_steady_path_classifies_wide_characters_it_has_seen() {
        let pair = [0xd83d, 0xde00];
        for (pattern, flags, hit, miss) in [
            ("é", "", vec![0xe9], vec![0xe8]),
            ("中", "", vec![0x4e2d], vec![0x472d]),
            ("中", "u", vec![0x4e2d], vec![0x4e2e]),
            ("\\u{1F600}", "u", vec![0x1f600], vec![0x1f5c0]),
            ("\\uD83D\\uDE00", "", vec![0x1f600], vec![0x1f601]),
            ("\\uD83D\\uDE00", "", pair.to_vec(), vec![0xd83d, 0xde01]),
        ] {
            let words = program(pattern, flags);
            let p = Program::from_words(&words, &mut Budget::new(1_000_000)).unwrap();
            let mut cache = vec![0u32; 1 << 15];
            let subjects = [
                [&[0x78][..], &hit, &[0x79]].concat(),
                [&[0x78][..], &miss, &[0x79]].concat(),
            ];
            let width = |points: &[u32]| -> usize {
                points.iter().map(|&c| if c > 0xffff { 2 } else { 1 }).sum()
            };
            // Build the states and classes both subjects meet.
            for points in &subjects {
                let bytes = wtf8(points);
                let input = Input::wtf8(&bytes).unwrap();
                for _ in 0..2 {
                    find(p, input, 0, &mut cache, &mut Budget::new(1_000_000)).unwrap();
                }
            }
            for (points, matched) in subjects.iter().zip([true, false]) {
                let bytes = wtf8(points);
                let input = Input::wtf8(&bytes).unwrap();
                let anchored = p.words[2] & Y != 0 || p.start_anchored();
                for first in [true, false] {
                    let answer = steady(
                        p,
                        input,
                        0,
                        &mut cache,
                        &mut Budget::new(1_000_000),
                        first,
                        anchored,
                    );
                    // A search for the first end reports only that there is one.
                    let expected = matched.then(|| 1 + width(&hit));
                    let answer =
                        answer.map(|end| end.map(|at| if first { 1 + width(&hit) } else { at }));
                    assert_eq!(
                        answer,
                        Some(expected),
                        "/{pattern}/{flags} on {points:x?}, first {first}"
                    );
                }
            }
        }
    }
}
