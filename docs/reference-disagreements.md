# Unicode zero-width matches: Node reference disagreements

On `a😀b`, Node 26.8.1 evaluates `/\B/du` to an empty match at UTF-16 `[2,2]`, between the emoji's surrogate halves. Perex returns no match. Independently executed QuickJS revision `04be246001599f5995fa2f2d8c91a0f198d3f34c` also returns no match.

The [RegExpBuiltinExec specification](https://tc39.es/ecma262/multipage/text-processing.html#sec-regexpbuiltinexec) converts Unicode-mode input to code points and advances attempted starts with AdvanceStringIndex. For this string the characters are `a`, `😀`, `b`; the interior of the emoji is not an additional character position. At every available position, exactly one adjacent character is a word character, so the [non-boundary assertion](https://tc39.es/ecma262/multipage/text-processing.html#sec-runtime-semantics-compileassertion) fails. This is the basis for retaining Perex's result. These algorithms were checked on 2026-09-10; no complete language-conformance claim follows from this case.

The difference also occurs with capturing and lookahead wrappers and sticky starts at the low surrogate in Node. Adding a consuming dot removes that match. The 24-case authored `tests/fixtures/word-boundary-reference/probe.js` and both engines' unchanged outputs are retained beside it. QuickJS is a development reference only; it is not linked into Perex.

`tests/fixtures/reference-disagreements.json` binds the exact matrix input, original Node answer and Perex answer. The default differential check still reports the mismatch. An explicit development option verifies both sides have remained exactly as reviewed before permitting that listed discrepancy. This does not edit the Node oracle, claim Node parity or exclude the case from the adoption inventory.

Generated compositions revealed two additional instances of the same forbidden interior position. For `((?:(?![^b])(?:))+)` and the full second pattern in `zero-width-probe.js`, both with `du` on `a😀b`, Node reports `[2,2]`; Perex and the independently executed pinned QuickJS report `[3,3]`. At the emoji's code-point position, `[^b]` matches and its negative assertion fails; at `b` it does not match and the assertion succeeds. No attempted Unicode character position exists at UTF-16 offset 2. Both original Node and QuickJS outputs and the exact generated inputs are retained. All three reviewed discrepancies remain visible in strict-mode results.

# Oracle answers that depend on accumulated V8 state

A separate class of difference is not a disagreement about semantics at all: the
oracle returns different answers for the same case depending on what the same
Node process evaluated earlier.

In `tools/check-modifiers.mjs`, eight cases of the form
`(?i:(?<x>a)|(?<x>b))(?-i:\k<x>)` and `(?i:(?<x>a)|(?<x>b))(?i:\k<x>)` on
`"0".repeat(80) + "aBBB"` reported no match while Perex reported a match at
UTF-16 `[81,83]` with `x` captured at `[81,82]`. Re-running the identical case
in a clean Node 26.5.1 process returns the same match Perex returns, as does
`new RegExp(source, flags).exec(subject)` evaluated directly. Replaying the
check's generated cases in order reproduces the no-match; a bisect placed the
transition after roughly 108,000 preceding cases, and no small pair of patterns
reproduces it, so the trigger is cumulative process state rather than one
pattern.

The answer Perex returns is the one the specification requires. Inside
`(?i: … )` the second alternative matches `B` under case folding and captures
it; the `(?-i: … )` backreference then compares that captured `B` against the
following `B` case-sensitively and succeeds. Position 80 fails first because the
`a` there forces `x` to be `a`, which the next position does not repeat.

This is a measurement artifact in the oracle, so it is removed by measuring
again rather than by editing an answer. `reference.mjs` exposes `freshAnswers`
and `stableDifferences`: every differential check compares as before, then
recomputes only the disagreeing cases in a fresh process and compares those
again. A difference that survives is reported unchanged and still fails the
check. The number of cases whose oracle answer was unstable is published in
each report as `unstable_oracle_answers`, so the condition stays visible instead
of disappearing. It is not an allow-list, and it binds no expected answer.

Observed with Node v26.5.1 and V8 14.6.202.34-node.24 on darwin arm64. The
pinned CI version is Node 26.8.1; this has not yet been re-checked there, and no
upstream report has been filed.
