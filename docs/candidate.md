# Candidate starts in original byte storage

The compiler derives a conservative range containing every possible first
consumed ASCII character. The evaluator can skip positions outside that range
before initializing registers or running assertions. Every possible start still
enters the ordinary instruction loop and preserves its capture and match order.
This is an optimization within one matcher, not a second engine or a fallback.

## Compilation and format

The compiler walks the topologically ordered AST after emission and required-text
admission. It reuses node fields whose emission lifetime has ended; it allocates
no additional arena. Literals and classes contribute ASCII members using their
lexical case flags and the same Unicode equivalence/property tables as matching.
Alternatives union their ranges. Concatenation includes the right child's range
only when the left child may be empty. Optional repetition may be empty.
Assertions contribute no consumed character. Unknown backreference content
conservatively admits every ASCII character and may be empty.

A nullable root disables the optimization. Holes in a union are conservatively
widened into one interval, admitting extra starts without losing real ones.
Character classes that cannot consume ASCII can reject an entirely ASCII subject.
Compiler work is charged to the existing budget, including class/property analysis.
Classes without Unicode case folding combine existing property masks and literal
intervals into a temporary 128-bit value; complement and legacy ASCII folding use
bit operations. Unicode case folding still checks the full equivalence families,
including their non-ASCII members. No additional shared property table is stored.

Folded literal inference uses the ASCII bounds of its case family directly.
An exhaustive Rust check compares those bounds against the pinned equivalence
tables for every code point in both modes. This avoids walking non-ASCII case
families merely to discover that they contain no ASCII start.

Program format 10 has nine header words, retaining the first-character descriptor from formats 8 and 9. Word 7 is zero when disabled, one when
no ASCII start is possible, or `2 | (lo << 8) | (hi << 16)` for an inclusive
interval with `0 <= lo <= hi <= 127`. Reserved bits, malformed bounds and old
format versions are rejected. The additional word costs four used program bytes.
Bindings check the full eleven-word header when reacquiring a view. AOT and runtime
programs must use this same format and its version checks.

The validator checks the descriptor's representation, not a proof that it follows
from arbitrary hand-authored instructions. Like required-text admission, the
semantic guarantee belongs to the compiler. A format-valid edited hint can change
answers. Imported programs need trusted compiler provenance as well as structural
validation; passing validation alone is not a correctness certificate.

## Execution, lifetime and work

Scanning borrows the original validated bytes. Entirely ASCII inputs retain their
direct path. Mixed WTF-8 inputs can skip ASCII runs, stopping at every non-ASCII
lead byte for ordinary matching. A position inside a scalar's surrogate pair
always goes through ordinary matching. Native UTF-16 retains its existing path.
No representation is copied or converted. Sticky matching checks only its
requested position. A nonnullable pattern cannot match at the subject's end.

Each scan step reads at most 4096 available bytes, further bounded by its
execution quantum and remaining work, so a host that wants finer pauses gets
them by passing a smaller quantum rather than by this cap. Portable eight-byte arithmetic locates a possible
character; scalar handling covers the first byte and tail. A program whose word
9 carries a leading claim looks for that claim's exact byte pairs instead of
this descriptor's interval, which is a strictly narrower condition on the same
positions; [leading](leading.md) describes it. Only logical positions
through the first candidate are charged, so word-read speculation does not change
matching work across quanta. Subject positions remain cursor offsets/checkpoints;
no subject address survives the scoped borrow. The scan adds a phase but no frame,
undo entry or match-scratch buffer.

Mixed scanning uses the cursor's current byte suffix, never a UTF-16 re-seek from
an end of the string. It checks the skipped prefix is ASCII and advances byte and
UTF-16 offsets together. Word arithmetic masks high bits before lane comparison
and treats every original high bit as a potential start; byte arithmetic must
not carry a non-ASCII lane into neighboring ASCII comparisons. Programs with no
possible ASCII first character still inspect non-ASCII starts. The existing
format-9 descriptor already permits this behavior, so no format change is needed.

## Checks and measurement boundary

Rust checks compare enabled and disabled descriptors and original UTF-16 input,
including nullable prefixes, assertions, backreferences, scoped case flags and
sticky starts. Exhausted work leaves capture output untouched. Resumption checks
move and poison program/subject storage at quanta around word and chunk boundaries
and require identical complete captures and remaining work.

`tools/check-candidate.mjs` independently compares captures, matched units, named
groups, no-match and errors against Node. It records source cases, oracle answers,
actual answers and commands before reporting differences, and can exercise subject
and program relocation plus scratch growth at every pause. Existing broad engine,
property, folding, name, legacy, repetition and modifier checks remain required.

These checks do not establish a CPU/RSS win. Measure every existing search and
compilation case against the exact previous binary and compatible alternatives,
including short direct hits where the new analysis/phase can add overhead.


## Short remainders

A search used to take the same route whatever the subject's length: admission,
then a dispatcher round to seek to the requested start, then another to scan
from there. On a short subject those rounds are most of the search. `/z/`
against `"a"` spent 23.6 ns deciding what one comparison decides, where V8
spends 14.9.

So on entering admission, an ASCII remainder shorter than 64 bytes is scanned
once for a byte the descriptor admits. None decides the search: no start in the
remainder can match. The first one found is where the search begins, since no
start before it can match either, and the start phase is entered there as a
seek would enter it — including the end bound, which a match anchored at the
end would otherwise have applied. The scan is skipped where it could only add
work: no descriptor, sticky or start-anchored searches, which examine one
position, and a remainder of 64 bytes or more, which the existing scan chunks.

The decision depends on the program, the subject and the start, never on the
quantum, so a paused search makes it at the same charge. Positions through the
first candidate are charged, as the scan from the start charges them; a miss of
`n` positions is charged `n`, where it was charged at least `n + 1`. The step
reads fewer bytes than the admission chunk `docs/resumption.md` bounds a pause
by.

### Measurement

The same binary built against the previous and the new engine, in one process,
interleaved per round, best of twenty-one; V8 adjacent in time:

| Case | Before | After | V8 `test` | V8 `exec` |
|---|---:|---:|---:|---:|
| `/z/` against `""` | 19.5 ns | **11.8 ns** | 12.6 ns | 14.5 ns |
| `/z/` against `"a"` | 23.6 ns | **12.5 ns** | 14.9 ns | 16.8 ns |
| `/z/` against ten bytes | 24.4 ns | **13.5 ns** | 15.2 ns | 17.6 ns |
| `/a/` against `"a"` | 50.6 ns | 49.5 ns | 17.8 ns | 34.6 ns |
| Eight-character literal | 56.5 ns | 53.3 ns | 13.4 ns | 27.4 ns |
| Short literal, 29 characters | 56.0 ns | 52.2 ns | 39.0 ns | 45.2 ns |

Through a host's bound resources the same change is 352 instructions a call:
`examples/call_cost`'s `search` stage, which binds the program and subject per
call as a host does and searches at a quantum of 4096, takes 1,104 instructions
for `/qq_[0-9]+/` over `"record_12345"` before this and 752 after, at one and
two million calls. A hit whose first byte is already a candidate pays about
1 per cent more instead, from the checks that decide that.

The three misses go from behind V8 at both entry points to ahead of both. The
short literal hits lose one round and gain between 2 and 7 percent. Of the rest
of the twenty-five cases in `bench/`, none moved by more than 1.1 percent except
the 256 KiB literal lookbehind, which measured between 0 and 2.9 percent slower
across five runs of 21 rounds each: its remainder is too long to scan, and it
pays only for being told so.

The claim that a program is anchored at the subject's start is derived at the
start of every search, and the same measurement priced that derivation at
1.3 ns of the empty search. It now decides the common case — a consuming
instruction straight after the entry `SAVE`s — before building its branch stack.
Without that, the misses were about 1 ns slower and the literal hits up to
1 ns slower; `[^0-9]+!` was 0.8 percent slower than before rather than 1.1.

### Checks

`tests/short_start.rs` compares thirty-four patterns against the same pattern
written as `(?:P|(?!))`, whose descriptor is empty so the scan never runs,
asserting that it is. Subjects are ASCII, from empty to eighty bytes around the
64-byte limit, with the needed bytes absent, first, last, in the middle and
repeated, from starts at zero, one, the middle, the end and past it. The
patterns cover literals, folding, classes, alternation, captures and named
groups, lookbehind and lookahead, end anchors with and without `m`, `^`, the
`y` and `u` flags, backreferences, leading repeats, a class with no ASCII
member, nullable patterns and the empty pattern. A miss the scan decides must
charge no more than the search it replaces, a miss over `n` positions is
charged exactly `n`, and a candidate before an end bound must not become the
start. Six injected faults are each caught: a start one past the candidate, the
end bound ignored, a candidate taken as absent, sticky searches scanned, the
scan read from the subject's start rather than the requested one, and the length
limit removed. A seventh, testing the class with no ASCII member for a NUL byte
instead of deciding it outright, was not a fault: no candidate is found either
way. The engine differential and the candidate, end-candidate, classes and
atom-filter harnesses, plain and at one- and seventeen-unit quanta with
relocation and growth, and the admission, casefold, names, legacy, repetition,
modifiers, sets, Unicode-sets and sequences harnesses report no differences.

`tests/resumption.rs` tested an eighty-member class against `"x"` as a long
operation that must pause at a one-unit quantum. Since candidate scanning landed
that search never reached the class — its descriptor rejects every ASCII byte —
and paused only between phases, which this change removes. It is now tested
against `"é"`, which the class has to be evaluated against.

## Strict end condition

Format 10 adds word 8, encoded like word 7, for possible last-consumed ASCII
characters. The compiler enables it only if every successful path consumes at
least one character and ends at a non-multiline end assertion. Alternation must
preserve the assertion on both paths. A later nullable expression is not enough
to preserve an earlier assertion: it must be unable to consume input. Assertions
contribute no consumed characters; their internal matches and captures stay in
the ordinary evaluator. Scoped multiline flags govern each end assertion.

Before searching, the evaluator charges one work unit, borrows an endpoint cursor
in the original input and reads one unit backwards. An absent unit or impossible
ASCII value proves no match. Non-ASCII values, including surrogate halves, always
continue through the evaluator. This check supports original UTF-16 and UTF-8/WTF-8
storage without a copy, index or new scratch. The descriptor adds four bytes to
each program; binding/header metadata follows the versioned header size. Existing
program versions are rejected.

This removes a class of impossible end-anchored searches before backtracking.
It does not give arbitrary backtracking expressions a linear-time guarantee.
Capture answers, errors and resource limits remain the evaluator's responsibility.
`tools/check-end-candidate.mjs` compares complete answers against Node in ordinary
and one-/seventeen-work-unit advances with relocation and scratch replacement.
The bounded Rust witness also removes the descriptor and requires work exhaustion,
so a missing check cannot make the test itself run without a bound.

## Bounded end-anchored starts

The strict end condition above rejects a search whose final character cannot
match. When the pattern is also *bounded* in length, the same end assertion
proves something stronger: where a match can begin.

If every successful path ends at a non-multiline end assertion and consumes at
most `k` UTF-16 units, then a match starting at `s` ends at `s + k` or earlier,
and it must end at the subject's end, so `s` is at least `len - k`. Every
position before that is skipped without being scanned or tried.

The compiler derives `k` in the same bottom-up pass that derives the end
assertion, reusing the spare bits of the node field that already carries the
`consumes` and `anchored` facts. A character contributes one unit, or two above
the basic plane; `.` and a class contribute two, since either can match an
astral character. A sequence adds its parts, an alternative takes the larger,
an assertion contributes nothing. A repetition or a backreference has no bound
this pass can establish, so it disables the claim rather than guessing one —
`[a-j]{3}$` is bounded in principle but is not claimed here.

Word 8's upper byte holds `k + 1`, so zero means no bound. The range descriptor
keeps the low 24 bits, and readers of it must mask the upper byte off: a bound
present with a disabled range descriptor would otherwise look like a range of
`[0,0]` and reject every subject. Like the other descriptors, the bound's
representation is validated and its semantic guarantee belongs to the compiler.

A sticky search whose requested position is before the bound cannot match, and
reports that without scanning. A non-sticky search seeks forward to the bound
and proceeds normally from there.

The zero-length case is the one to be careful about: `(?<=abc)$` consumes
nothing, so its bound is zero and the search starts at the subject's end, where
a zero-width match is exactly what should be found.

### Measurement

Alternating the identical driver between both binaries on a 256 KiB subject:

| Case | Before | After | Work before/after |
|---|---:|---:|---|
| `/needle$/` hit | 77.2 µs | 82 ns | 305,825 / 24 |
| `/zzneedle$/` miss | 58.5 µs | 34 ns | 320,372 / 11 |

The work-unit reduction is deterministic and independent of machine load; the
wall-clock ratios were measured under a load average near 50 and are reported
as ratios for that reason. A short anchored subject and an unanchored control
were unchanged at 1.0x, so this removes work without adding any to patterns it
does not apply to.

`tools/check-end-candidate.mjs` covers the bound over subjects longer than the
match can be, including zero-width endings, astral characters whose unit count
exceeds their character count, nullable and multiline endings, repetitions that
disable the claim, and sticky starts on both sides of the bound: 34,395 cases
with no differences, unchanged under one-unit resumption with relocation.

## Start-anchored starts

The same argument applies at the other end, and needs no bound. If every
successful path asserts `^` without the `m` flag before it consumes anything, a
match can only begin where that assertion holds: at the subject's start. Every
later start fails at the same `^`, so trying them is work that can only fail.
A search did try them. After `/^[a-z]/` failed at position zero of a
90,005-unit subject it tried every other position, charging 240,021 work units
where one attempt charges a handful. Perry's `test` and `exec` calls pay a
search per call, so a failing anchored pattern paid it on every call (Perry
issue #10166).

The claim is derived from the instructions rather than stored, like the leading
run, so no program word can assert it falsely. From instruction zero, capture
bookkeeping is skipped and a `SPLIT` is followed into both branches; every path
must reach `START`. Anything first that could consume or match elsewhere — a
character, a class, a repeat, an assertion, a jump, or `^` under `m` — disables
it, so `^a|b`, `(?:^)?a`, `(?:^a)+`, `(?<=^)a` and `(?m)^a` are searched as
before. The walk inspects at most 128 instructions and eight pending branches,
once per search, on entering admission.

A start-anchored search then behaves as a sticky one does for choosing starts:
the candidate scan examines only its first position, and no later start is
tried. A requested start of two or more cannot match at all — the `u` flag moves
a start back by at most the one unit that splits a surrogate pair — so such a
search ends before seeking, which on non-ASCII storage would otherwise cost up to
half the subject. Admission still runs: when the required text is absent it
rejects in one scan an attempt whose backtracking could cost far more.

### Measurement

Work units, which are deterministic; wall-clock figures were taken at a load
average above 100 and are not reported:

| Case | Before | After |
|---|---:|---:|
| `/^[a-z]+_[0-9]+$/` against `"!bad_123456"` | 40 | 3 |
| `/^[a-z]/` against `"!bad_123456"` | 27 | 2 |
| `/^[a-z]/`, 90,005-unit subject | 240,021 | 2 |
| `/^abc/`, 90,005-unit subject | 90,011 | 2 |
| `/^[a-z]+_[0-9]+$/` against `"record_123457"`, a match | 46 | 46 |

### Checks

`tests/start_anchor.rs` compares each pattern against the same pattern written
as `(?:P|(?!))`, which the derivation does not treat as anchored, from every
start including past the end, over ASCII, non-ASCII, astral, lone-surrogate and
UTF-16 subjects on both sides of the admission threshold. The patterns cover
folding, captures, alternations of anchored branches, empty and end-anchored
matches, the `u`, `s` and `y` flags, a lookahead after `^`, and the non-anchored
forms above. A second test requires every failing anchored search to stay under
500 work units from starts at zero, one, the middle and the end of a
90,005-unit ASCII subject, a 120,005-unit non-ASCII subject in both WTF-8 and
UTF-16, and a 10,000-unit subject with no candidate position, while the unanchorable form of the same pattern charges more than
50,000. Six injected faults are each caught: `^` under `m` counted as anchoring,
one `SPLIT` branch unchecked, later starts still tried, the candidate scan
running past the first start, the no-seek rule starting at one instead of two,
and that rule removed. The engine differential (plain, one- and seventeen-unit
resumption with relocation, and from positions), and the admission, bound,
candidate, end-candidate, classes, atom-filter, lookbehind, run-skip, names,
legacy, repetition, modifiers, casefold, sets and Unicode-sets harnesses report
no differences.
