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
Bindings check the full nine-word header when reacquiring a view. AOT and runtime
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

Each scan step reads at most 256 available bytes, further bounded by its execution
quantum and remaining work. Portable eight-byte arithmetic locates a possible
character; scalar handling covers the first byte and tail. Only logical positions
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
