# Lazy DFA

`perex::dfa` answers whether a program matches, and where its match begins
and ends, by walking a deterministic automaton built while it searches. The
automaton is data in storage the host owns. It is built from the program's
instructions one state at a time, only for states a search reaches, and only
for programs that run. No program gains words beyond one header bit, and no
code is generated.

The backtracking evaluator stays the definition of every answer and the only
source of captures. The automaton is a second reading of the same instructions
for the subset whose meaning a finite automaton can hold exactly, and the
differential suites compare the two on every case they run (see Verification).

## Eligibility

Format 15 adds one bit to header word 7, `DFA` (bit 25). The compiler sets it
and `Program::from_words` re-derives it from the instructions and rejects a
program whose bit disagrees, as it does for the start-anchored claim, so a
program can neither claim nor hide eligibility.

A program is eligible when every instruction is one of:

- characters, classes (plain, folded, sorted, properties and their `u`/`v`
  complements), `.` with and without `s`;
- capture bookkeeping (`SAVE`), `SPLIT`, `BRANCH`, `JUMP`, `MATCH`;
- `^`, `$` with and without `m`, `\b`, `\B`, folded or not;
- repeats (`REPEAT_INIT/CHOICE/BODY/NEXT`, `ATOM_REPEAT`) in the shape the
  compiler emits, whose counters fit (below).

Backreferences, lookahead and lookbehind, and properties of strings (which
consume more than one character per instruction) make a program ineligible.
Their programs are searched by the evaluator alone, exactly as before.

A repeat's counter is part of the automaton's state, so it must be bounded. A
repeat with a maximum counts up to it; an unbounded one counts only up to its
minimum, since every count from there on behaves the same. Each repeat needs
the bits of that bound plus one bit for the empty-iteration check, at most
1023 per bound, and the repeats enclosing any one instruction must fit together
in 32 bits. Repeats that do not enclose each other share bits.

## States and threads

A thread is an instruction index and a 32-bit key holding the counters and
empty-check bits of the repeats around it. A forward state is an ordered list
of threads: those that have just consumed the character before the current
position, in the order the evaluator would try them. It also records what is
known about that character — whether it is a line terminator, a word character
with and without Unicode folding, or absent at the subject's start — and
whether new starts are still being added.

The transition on the next character (or the end of the subject) takes the
state's threads in order and follows every non-consuming instruction depth
first, first arm first, as the evaluator does:

- `SPLIT`, `BRANCH`, `JUMP`, `SAVE`: both arms in order, the jump, nothing.
  `BRANCH` is followed as a `SPLIT`; its first-character set only skips work.
- Assertions are decided here, with the previous character known from the
  state and the next character known from the transition.
- `REPEAT_CHOICE` reads the thread's counter, and `REPEAT_NEXT` the empty-check
  bit, exactly as the evaluator reads its two registers: an iteration that
  consumed nothing after the minimum fails. Leaving a repeat clears its bits,
  so equal continuations have equal keys.
- A thread that reaches a thread already reached at this position is dropped:
  everything it could do, the earlier and higher-priority one does first.
- A consuming instruction that accepts the character becomes a thread of the
  next state, with the empty-check bits of its enclosing repeats set.

This is the evaluator's order of exploration without its frames, so the first
thread to reach `MATCH` is the match the evaluator would return at this
position, if no thread before it matches later.

### Leftmost-first

An unanchored search adds a thread at instruction zero after all others at
every position, so earlier starts keep priority over later ones. When a thread
reaches `MATCH`, the position is recorded as a match end, every thread after it
is dropped and starts stop being added. Threads before it continue: if one of
them matches later, it is the match the evaluator would have found first, and
its end replaces the recorded one. The search ends when no thread remains or
the subject ends; the last recorded end is the end of the evaluator's match,
and its start is the leftmost start that has any match.

`is_match` stops at the first recorded end.

### The start

The start comes from a reverse automaton over the same instructions. Its
thread at instruction `pc` and position `p` means: the evaluator at `pc` at `p`
can reach `MATCH` at the end already found. Moving left, it follows the
program's edges backwards and consumes characters backwards. Any path will do
here, not the evaluator's preferred one, and over the set of paths the
empty-iteration check changes nothing: removing an empty optional iteration
from an accepting path leaves an accepting path with the same ends. Counters
count iterations from the right, and leaving a repeat at its `REPEAT_INIT`
requires the count to lie between its minimum and maximum. Every position where
instruction zero is reached is a start; scanning left to the search's own start
finds the leftmost one. No match can begin left of the leftmost start with a
match, so that is the evaluator's start.

A sticky or start-anchored search has one start, the requested one, and needs
no reverse pass.

## Characters

The automaton reads what the evaluator reads: code points under `u` or `v`,
code units otherwise, through the same cursor over the subject's original
storage. Characters are grouped into classes by the predicates the program
applies to them: one predicate per distinct consuming instruction, evaluated
with the evaluator's own rules (case equivalents, property tables, the `v`
complement), plus the line-terminator and word-character bits the program's
assertions read. Two characters with the same predicate results are
indistinguishable to the program, so transitions are kept per class.

Classes are found lazily. ASCII characters keep their class in a 128-entry
table; other characters are kept in a hashed memo. A character not yet seen
has its predicates evaluated once, and the result is matched against the
classes so far or added as a new one. A search over wholly ASCII storage reads
bytes directly; others decode through the cursor.

Positions are UTF-16 units throughout, and a Unicode search starts at the
code point holding the requested position, as the evaluator's does.

## Cache

The cache is host-owned `u32` storage passed to every call as `&mut [u32]`:
a header, the ASCII class table, the start-state slots, tables derived from the
program (each instruction's predicate and the empty-check bits its consumption
sets, each repeat's field, the reverse edges; in the header, the leading
claim's byte pairs and whether a non-ASCII character can begin a match), the
non-ASCII class memo, the
class signatures, a work area for one transition, a hash of states, and the
region states are allocated from. It holds offsets only, so the host may move
it between calls, and nothing in it refers to the subject.

It belongs to one program. The host keeps one cache per program; each call
compares the program's length and the header words that most often tell
programs apart with the ones the cache was built for, and rebuilds it on a
mismatch. That comparison is a diagnostic, like the binding's header check, not
proof of identity: a cache handed a different program with the same length and
header can answer wrongly, which is the host's error. A debug build compares
the whole header.

The host decides its size, which is the hard cap; nothing grows.
`dfa::minimum_words(program)` is the least storage the program can be searched
in, and `dfa::stats(cache)` reports the storage, the words in use, the peak,
the states, classes and clears, and whether the cache disabled itself. A larger
cache holds more states and a larger work area; the work area allows four
threads per instruction, capped by a sixty-fourth of the storage.

When the state region fills, or the state hash passes half full, every state is
dropped and the search goes on from its current state, which is rebuilt first.
Widening the rows for a new class drops them the same way. A clear that came
after fewer than eight characters per state built since the previous one counts
as thrashing; after three such clears in a row the cache disables itself and
every later call declines until the host resets it (`dfa::reset`). A
transition that needs more threads than the work area holds, or a subject with
more classes than the signature table holds, declines that search only.

## Answers

- `dfa::is_match(program, input, start, cache, budget)`: `Tested::Match`,
  `Tested::NoMatch` or `Tested::Declined(reason)`.
- `dfa::find(program, input, start, cache, budget)`: `Found::Match(span)` with
  the span of group zero, `Found::NoMatch`, or `Found::Declined(reason)`.

The reasons are `Ineligible`, `Storage { required_words }`, `Disabled` and
`Capacity`. A declined search has changed nothing but the cache and the budget,
and the host runs the evaluator. Captures always come from the evaluator:
`Search` started at the span's start finds the same match there first, and the
probe checks exactly that on every case. A no-match needs no evaluator at all.

Each character costs one unit of the budget and each thread a transition visits
one more; an exhausted budget is `ExecError::WorkLimit`, never a no-match. The
automaton never pauses: its work is linear in the subject plus the states it
builds, so it takes no quantum. A start beyond the subject is a no-match, as
the evaluator's is; sticky and global semantics and `lastIndex` stay the
host's.

### The steady path

Once a program's states and classes are cached, a search over byte storage
runs without building anything: one table read for an ASCII character's class,
one for the transition, and one comparison that tells a live, known, unflagged
transition from everything else. Over wholly ASCII storage it starts anywhere;
over other WTF-8 storage it starts at the subject's start and decodes each
scalar once, taking runs of ASCII bytes the same way. A non-ASCII character's
class is read from the first slot of its memo probe. Positions in UTF-16 units
are counted only for a match end, from the last one counted, so a scan that
finds nothing never counts them. Anything not cached — and a high surrogate
under `u`, which may pair with a separately encoded low one — sends the search
to the general path from its start, before any state has changed. Over wholly
ASCII storage the reverse pass has a steady path of its own, under the same
rule.

### The idle skip

An unanchored search in its start state with no assertion context — no thread
alive, only the start to be added at the next position — can begin a match
only where the evaluator's candidate phase would start a trial. It skips there
with the evaluator's own scans (`src/executor/candidate.rs`), and only runs
transitions from the position the scan stops at:

- a program with a leading claim (word 9) looks for the claim's byte pairs,
  `first_in_pairs`, which stops at a small fraction of the positions the first
  character alone admits;
- otherwise, the bytes word 7's first-character descriptor admits, by the same
  word arithmetic `first_in_range` uses, four words to a branch.

Over other WTF-8 storage the descriptor speaks only for ASCII, so the scan stops
at every non-ASCII character, as the evaluator's mixed scan does; a character
whose cached transition leaves the start state where it is is passed over
without leaving the skip. When the cache's build found that no consuming
instruction reachable from instruction zero without consuming accepts a
non-ASCII character (a character instruction, an unfolded or ASCII-folded one,
or a class whose ranges all lie below 128 — anything else counts as accepting
one), no non-ASCII character can begin a match, and the scan passes over them
with the ASCII bytes it rejects; the byte pairs are then exact over this storage
too, unless the leading run is folded, since a folded `k` matches U+212A under
`u`. The skip is set up only when at least 32 bytes remain.

## For a host

Perry, or any host, needs:

- one cache per compiled program, allocated by the host to a size it chooses
  (at least `dfa::minimum_words`), kept with the program and accounted with it;
- `dfa::is_match` for `test` and any operation that only needs to know whether
  there is a match;
- `dfa::find` for the bounds, then `Search` from the span's start only when the
  operation needs captures beyond group zero;
- the evaluator whenever the answer is `Declined`, exactly as before.

The program format is the only change to existing APIs: format 15 programs
carry the eligibility bit, and programs of earlier formats are rejected as
before. Nothing else in the evaluator, the bindings or the resumable search
changed.

## Verification

- `tests/dfa.rs`: answers against the evaluator for literals, classes, case
  folding, properties, `v` classes, assertions, counted, lazy and nullable
  repeats, sticky and anchored searches and long subjects on the steady path,
  every start position; 1,500 random eligible patterns against the evaluator;
  eligibility derived both ways on validation; storage below the minimum;
  clearing a full cache with exact answers; thrashing that disables the cache
  until reset; a cache moved and poisoned between calls and rebuilt for another
  program; an exhausted budget as an error. Long subjects cover the idle skip
  over both storages: leading pairs, exact and folded (`ka`, `sk` and `ak`
  under `iu` against U+017F and U+212A), first characters that may or may not
  be non-ASCII, and matches after two-, three- and four-byte characters, whose
  positions are counted late.
- Unit tests in `src/dfa.rs`: the steady path's decoder against every scalar
  and surrogate from U+0080 to U+10FFFF; the late UTF-16 count against every
  encoding width; and two-, three- and four-byte characters and a pair encoded
  as two surrogates, each landing on a class the cache already holds, answered
  by the steady path itself — it must answer without leaving to the general
  path, and answer the class's transition.
- `examples/engine_probe.rs` with `PEREX_DFA=1` (roomy cache) or `small` (the
  minimum plus 256 words, which clears and thrashes): every eligible case is
  answered by `is_match` and `find` as well, with captures from the evaluator
  started at the automaton's start, and any disagreement is a `dfa-mismatch`
  outcome; where the evaluator runs out of work, the automaton's answer is
  printed for Node to check. CI runs every engine harness three times: without
  the automaton, with `1` and with `small`.
- `tools/check-dfa-fuzz.mjs PROBE OUTPUT_DIR --seed N --cases N`: random
  eligible patterns, flags and starts over subjects with case pairs, folding
  letters, line terminators, pairs and lone halves, against Node. The only
  differences it sets aside are those of the reviewed `\B`-between-halves Node
  disagreement in [reference disagreements](reference-disagreements.md); every
  other difference fails. A case whose match the evaluator could not finish
  within the probe's allowance is listed as `evaluator_work_limit`: the
  automaton found the match, but its captures need the evaluator.
- Sabotage: trying a split's second arm first, a greedy repeat that prefers to
  exit, folding dropped from characters or from classes, states allocated past
  the storage, thrashing that never disables, empty iterations accepted, the
  reverse pass ignoring a repeat's bounds, starts added after a match, `\n` as
  the only line terminator, a backreference program marked eligible, the steady
  skip passing over the top of its range, the WTF-8 skip passing over
  non-ASCII characters, a misdecoded three-byte and a misdecoded four-byte
  scalar on the steady path, a class reaching past ASCII counted as an ASCII
  first character, folded leading pairs compared as bytes over WTF-8, and a
  late position count that ignores four-byte sequences each turn the tests red.
  The misdecoded three-byte scalar once stayed green: the wrong value missed
  the class memo and the general path answered correctly. The steady-path unit
  test now requires the steady path itself to answer.

## Measurements

Instructions per search, exact (callgrind on x86-64), with the warm-up — the
cache's construction and the first states — excluded: the difference between
1,000 + N and 1,000 searches (5 + N and 5 for 1 KB, 1 + N and 1 for 64 KB).
Each search is what a host pays per call: bind the counted subject, view both
through `BoundResources`, search from 0. The evaluator runs
`run_without_captures` (`test`) or `run` (`exec`); the automaton runs
`is_match` or `find` with one cache of 65,536 words kept across calls. The
binding alone costs 129 of each short-subject figure. Subjects joined by commas
are searched in turn. The evaluator's `test` figures at this release are
identical to 0.1.12's on every row.

| Case | test 0.1.12 | `is_match` | exec 0.1.12 | `find` |
|---|---:|---:|---:|---:|
| `/^\p{DI}$/u`, `t,o,k,e,n,1,7, ` | 385 | 291 | 384 | 286 |
| the same, plus `─,│,é,中,😀` | 794 | 313 | 793 | 318 |
| `/^\p{DI}$/u` on `中` | 1,451 | 351 | 1,450 | 371 |
| `/^\p{DI}$/u` on U+00AD (match) | 1,553 | 368 | 1,648 | 438 |
| emoji pattern, `t,o,k,e,n,1,7, ` | 940 | 292 | 939 | 297 |
| the same, plus `─,│,é,中,😀` | 2,195 | 325 | 2,201 | 374 |
| emoji pattern on `t` | 400 | 292 | 399 | 297 |
| emoji pattern on `1` | 2,561 | 292 | 2,560 | 297 |
| emoji pattern on `中` | 1,629 | 369 | 1,628 | 387 |
| emoji pattern on `😀` (match) | 5,534 | 424 | 5,629 | 948 |
| `/ab\|cd\|ef\|gh/` on `xxgh` | 2,020 | 324 | 2,115 | 448 |
| `/(foo\|bar\|baz)=(\d+)/` on `a=1&baz=42` (match) | 3,658 | 390 | 3,811 | 637 |
| `/a/` on `t` | 400 | 292 | 399 | 297 |
| `/a/` on `a` (match) | 1,058 | 288 | 1,153 | 373 |
| `/qq_[0-9]+/` on `record_12345` | 452 | 424 | 451 | 429 |
| `/\w+/g` on `foo bar` (match) | 2,083 | 294 | 2,178 | 489 |
| `/^[a-z]+_[0-9]+$/` on `record_12345` (match) | 2,879 | 425 | 2,974 | 438 |
| `/\s+$/` on `foo bar   ` (match) | 3,913 | 401 | 4,008 | 556 |
| `/\d{3}-\d{4}/`, 1 KB ASCII, no match | 6,188 | 3,186 | 6,187 | 3,184 |
| the same, 1 KB WTF-8 | 6,175 | 4,179 | 6,174 | 4,183 |
| the same, 64 KB ASCII | 359,130 | 176,939 | 359,129 | 176,937 |
| the same, 64 KB WTF-8 | 359,437 | 236,940 | 359,436 | 236,944 |
| `/zq\|qz\|xj/`, 1 KB ASCII, no match | 4,567 | 4,136 | 4,566 | 4,134 |
| the same, 64 KB ASCII | 227,145 | 215,816 | 227,144 | 215,814 |
| the same, 64 KB WTF-8 | 14,221,128 | 215,898 | 14,221,127 | 215,902 |

No row costs more than the evaluator's. Short searches are dominated by the
evaluator's per-search setup and backtracking, which the automaton does not
have; of the automaton's own figure, about 160 instructions above the binding
are its fixed cost: the header and cache checks, the start state, the steady
loop's setup and the budget. Long scans skip with the evaluator's own scans
(see the idle skip), so they cost what the scan costs plus the transitions from
the positions it stops at; over WTF-8 storage, where the evaluator's candidate
scan stops at every non-ASCII character, the automaton passes over the ones
that cannot begin a match. `find` adds the reverse pass to the start, which
over ASCII storage takes its own steady path. Where the operation needs
captures beyond group zero, the evaluator still runs from the span's start.

Cache storage: the minimum is 2,723–2,815 words (about 11 KB) for these
patterns and 12,275 words (48 KB) for the emoji pattern; with 65,536 words of
storage, the most in use was 10,635–11,576 words for the small patterns and
33,220 for the emoji pattern. Most of it is the work area and tables sized
from the storage and the program; the states themselves were one to thirteen.

