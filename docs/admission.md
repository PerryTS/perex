# Required-text admission

The compiler can attach one necessary literal/class condition to a program containing repetition. Before a long search enters the VM, an absence check can return no-match. Presence merely permits the ordinary VM to execute; it never supplies captures or changes their selection.

The inference is conservative. Concatenation can retain either child's requirement. Alternatives retain a condition only when both children select the same condition. Groups and required repetitions propagate their child's condition. Optional repetitions and negative assertions contribute no condition. Positive assertions can contribute a condition, including text examined in lookbehind; therefore the initial check examines the entire original subject, including text before the requested starting position.

Literal candidates reference at most 32 consecutive `CHAR` instructions, stopping at every other opcode. Once their first required instruction executes in a successful match, the remaining consecutive characters must execute too. Backward instruction sequences are compared in the original string's order. Small positive interval classes use their existing records. Selection favors longer literals and small classes; equally ranked literals in a sequence prefer the later candidate. This is a heuristic and can miss other useful necessary conditions. The compiler caches each consecutive instruction run instead of repeatedly scanning up to 32 successors per literal.

## Representation and bounds

Experimental program format 5 keeps the seven-word header and existing instruction/table widths. Flag bit 7 enables the check, bit 6 records backward literal order, and the upper 24 bits reference an existing instruction. No program words, second compiled literal, or new owning allocation are added. A candidate beyond the encodable index range omits the optimization; it does not reject the pattern.

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
