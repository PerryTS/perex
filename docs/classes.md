# Large literal character classes

The compiler normalizes literal-only classes with at least 16 source intervals
inside the existing range arena. It detects already sorted input; otherwise it
uses iterative heapsort with charged comparisons and no extra allocation. A
linear pass merges duplicate, overlapping and adjacent intervals. Later arena
entries have not been allocated yet, so reducing the live range count changes
no existing class reference. Name characters at the other end of the arena are
untouched. Classes containing Unicode property references keep the original
membership representation.

At least eight remaining disjoint intervals select a sorted-class instruction.
Smaller results use the existing linear instruction, often with fewer program
words. Format 9 adds opcodes 27 and 28 for sorted membership without/with lexical
case folding. It retained format 8's candidate descriptor; format 10 adds a ninth header word for the final-character condition.
Validation checks that a sorted class has nonempty bounds, literal ranges only,
and strictly ordered, disjoint intervals. General range validation still checks
character bounds. Older program versions are rejected.

The evaluator uses the same original input cursor and capture logic. It performs
binary search for each distinct case-equivalence member, then applies the class
complement. This supports Unicode and code-unit modes, reverse assertions and
consuming-atom repetition. No native recursion, secondary matcher, subject
conversion or new execution buffer is added.

One lookup performs at most 128 charged comparisons (four values and at most 32
comparisons each). It may exceed a tiny requested quantum by that bounded amount,
within the pre-existing admission-step bound. All program/input views end before
returning. Work exhaustion remains an error with no partial capture output.

Tests compare original interval membership, full results against linear class
instructions, corrupt ordering, and complete captures/work across relocation.
`tools/check-classes.mjs` supplies independent Node answers for reordered,
duplicated, overlapping, complemented and folded classes, including original
surrogate units, assertions, repeats, backreferences and sticky starts. Passing
these checks is not a measured CPU/RSS improvement or full host conformance.
