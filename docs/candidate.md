# Candidate starts in original ASCII storage

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

Program formats 8 and 9 have eight header words; format 9 retains this descriptor. Word 7 is zero when disabled, one when
no ASCII start is possible, or `2 | (lo << 8) | (hi << 16)` for an inclusive
interval with `0 <= lo <= hi <= 127`. Reserved bits, malformed bounds and old
format versions are rejected. The additional word costs four used program bytes.
Bindings check the full eight-word header when reacquiring a view. AOT and runtime
programs must use this same format and its version checks.

The validator checks the descriptor's representation, not a proof that it follows
from arbitrary hand-authored instructions. Like required-text admission, the
semantic guarantee belongs to the compiler. A format-valid edited hint can change
answers. Imported programs need trusted compiler provenance as well as structural
validation; passing validation alone is not a correctness certificate.

## Execution, lifetime and work

Scanning borrows the original validated ASCII bytes. Native UTF-16 and non-ASCII
byte subjects retain the existing instruction path; no representation is copied
or converted to enable this optimization. Sticky matching checks only its requested
position. A nonnullable pattern cannot match at the end of an ASCII subject.

Each scan step reads at most 256 available bytes, further bounded by its execution
quantum and remaining work. Portable eight-byte arithmetic locates a possible
character; scalar handling covers the first byte and tail. Only logical positions
through the first candidate are charged, so word-read speculation does not change
matching work across quanta. Subject positions remain cursor offsets/checkpoints;
no subject address survives the scoped borrow. The scan adds a phase but no frame,
undo entry or match-scratch buffer.

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
