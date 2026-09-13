# Required-text admission

The compiler can attach one necessary literal/class condition to a program containing repetition. Before a long search enters the VM, an absence check can return no-match. Presence merely permits the ordinary VM to execute; it never supplies captures or changes their selection.

The inference is conservative. Concatenation can retain either child's requirement. Alternatives retain a condition only when both children select the same condition. Groups and required repetitions propagate their child's condition. Optional repetitions and negative assertions contribute no condition. Positive assertions can contribute a condition, including text examined in lookbehind; therefore the initial check examines the entire original subject, including text before the requested starting position.

Literal candidates reference at most 32 consecutive `CHAR` instructions, stopping at every other opcode. Once their first required instruction executes in a successful match, the remaining consecutive characters must execute too. Backward instruction sequences are compared in the original string's order. Small positive interval classes use their existing records. Selection favors longer literals and small classes; equally ranked literals in a sequence prefer the later candidate. This is a heuristic and can miss other useful necessary conditions. The compiler caches each consecutive instruction run instead of repeatedly scanning up to 32 successors per literal.

## Representation and bounds

Program format 5 introduced this check and kept the seven-word header and existing instruction/table widths. Flag bit 7 enables the check, bit 6 records backward literal order, and the upper 24 bits reference an existing instruction. No program words, second compiled literal, or new owning allocation are added. A candidate beyond the encodable index range omits the optimization; it does not reject the pattern.

Inference runs after emission and reuses parser-node fields that are no longer needed. The compiler establishes necessity. `Program::from_words` checks the hint's index/opcode bounds, along with the ordinary format, but does not prove arbitrary hand-authored hints are semantically redundant. The format remains unstable and native-endian.

Execution borrows the original subject. ASCII literals and single ASCII intervals can search original byte storage because ASCII bytes cannot occur inside multibyte UTF-8/WTF-8 encodings. ASCII-insensitive literal comparison uses this path only on ASCII input. Other comparisons use lossless cursors and the same case/class operations as the VM. Before a generic literal scan, a bounded reverse probe checks whether the subject ends with the required literal. A hit admits normal matching; failure still checks the whole subject. This uses at most 32 character comparisons and constant-time access to the original end boundary, including UTF-16 and surrogate encodings. A fixed 32-byte stack buffer contains pattern bytes only; no subject bytes or UTF-16 conversion buffer are created.

The work budget covers the check and subsequent VM execution. Byte scanning charges bounded chunks before reading them, and generic cursor/comparison work is charged throughout. Exhaustion remains an error and leaves capture output untouched. Subjects shorter than 64 UTF-16 units and programs without a hint skip the admission function at the entry point. This threshold is a tuning choice, not a semantic limit.

## Validation and measurement

`tests/admission.rs` exercises long rejection, alternatives and optional branches, lookbehind before the starting position, reverse order, case equivalence, surrogate storage, chunk boundaries, relocation and work exhaustion. It also clears the hint on a program to show that the same bounded long-miss witness again exhausts its allowance. `tools/check-admission.mjs` compares complete answers on long subjects so the optimization is exercised rather than bypassed by the short-input threshold. Existing full-answer suites remain required.

`examples/engine_cost.rs` measures repeated compilation into reusable buffers or repeated execution on one validated borrowed subject. It reports elapsed loop time, checksums, work consumption, exact program size and nominal buffer capacities. Use an identical driver against exact revisions and an external process CPU/RSS tool. Input validation, initial allocation and the first compilation occur before the internal loop timer; external process CPU/RSS includes them. These scopes must remain separate from complete host-operation and GC measurements.

Admission alone does not establish faster matching across all cases. It can add work to successful searches, fail to help when required text appears in the wrong position, and increase compilation work. Those cases and program/scratch/RSS costs must remain visible in comparisons. No whole-application CPU/RSS or production-adoption claim follows from this optimization.

Single ASCII-range admission scans use bounded eight-byte comparisons directly
on the original storage. High-byte UTF-8/WTF-8 lanes cannot satisfy an ASCII
range. Admission still charges the same complete chunk before scanning and
enters the ordinary evaluator whenever the required range is present. No
program-format or scratch-layout change is needed.

## Bounding where a match can begin

Presence alone admits the whole search. When the condition's necessity comes
from instructions the match itself consumes, it also proves *where* a match can
begin: every successful match starting at `s` consumes the condition at some
position at or after `s`, so if no occurrence exists at or after `s`, no match
can begin at `s` — or at any later position, which only removes occurrences.

This turns the pathological arrangement the section above names — required text
present, but before the prefix that must consume its way to it — from a search
that retries every start into one that stops at the first start past the last
occurrence.

### The claim and its format

The compiler already selects the condition by walking the emitted AST. It now
also tracks whether the selected candidate was reached through a positive
assertion. A lookbehind body can satisfy the condition before the match start,
so an assertion-derived condition proves presence only. Lookahead is excluded
with it rather than analysed separately; that is conservative, not required.
Alternatives keep the claim only when both paths carry it.

Program word 9's low byte is the leading-literal run. Bit 31 is this separate
claim. It is a compiler guarantee, like the word 7 and 8 descriptors and unlike
the leading run, which the validator re-derives. `Program::from_words` checks
that no reserved bit is set and that the claim never appears without the
condition it bounds. The claim needs no additional program word.

The claim is restricted to literal conditions, because the bound resumes the
existing literal byte search. A class condition still admits and rejects as
before.

### Execution, lifetime and work

Two offsets in the execution state carry the bound: the position of a known
occurrence, and the position from which the condition has not yet been
searched. The initial admission search records both when it succeeds.

Before scanning for candidate starts, the evaluator keeps its scan inside the
known occurrence. When the scan base passes it, the condition search resumes
from the unsearched position instead of restarting, finds the next occurrence
and continues, or reports no match when none remains. Because that search only
moves forward, the entire bound costs one pass over the subject however many
starts are attempted.

The search reuses the same chunked, charged byte scan as initial admission, so
it is resumable at every quantum, borrows the original storage, constructs no
subject copy, and adds no frame, undo entry or match-scratch buffer. The state
grows by two offsets in one execution state. Exhaustion remains an explicit
error and leaves capture output untouched.

The bound applies where the candidate-start scan applies: ASCII storage with a
start descriptor. A nullable root disables that descriptor, so `a*END` admits
and rejects as before without the bound.

### Checks and measurement

`tests/bound.rs` covers the compiler's claim across repetitions, alternatives,
classes, lookbehind and lookahead; a condition only before every start; later
occurrences found after an earlier one is passed; requested and sticky starts;
work exhaustion at every small allowance; and relocation. `tools/check-bound.mjs`
compares complete answers against Node over conditions placed before, after,
around, split across and repeated through the subject, at filler sizes that
straddle the admission threshold and the scan's chunk boundary, in ordinary and
one-/seventeen-work-unit advances with relocation and scratch growth.

On one authored case — `/a+END/` against `END` followed by 4,000 `a` — charged
work fell from 24,046,265 units to 4,270, a factor of 5,631. That figure is
deterministic and independent of machine load. Alternating the identical driver
between both binaries showed a larger wall-clock ratio, between 110,401x and
162,210x over three rounds, because the retried starts also pushed frames and
undo entries whose real cost exceeds their charge. Measured beside it on the
same loaded machine, `regress` took about 24 ms and Rust `regex` about 7 us.

This removes one arrangement. It gives arbitrary backtracking expressions no
general linear-time guarantee, and patterns without a literal condition, or
with a nullable root that disables the start descriptor, are unchanged.
