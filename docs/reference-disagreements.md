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
in a clean Node 26.5.1 process returns the same match Perex returns.

This was reduced to a V8 defect and is recorded, with a self-contained
reproduction and a report ready to file upstream, in
`tests/fixtures/v8-modifier-alternation-backreference/`. The affected shape is a
scoped modifier group whose alternation captures, where the winning alternative
required case folding and a backreference then reads that capture; the smallest
case is `/(?i:(a)|(b))\2/d` against `"BB"`, which returns `null` instead of
matching at index 0. Forcing V8's regexp interpreter with `--regexp-interpret-all`
or `--jitless` restores the correct answer, so the natively compiled code is the
wrong one. Named and numbered groups are equally affected, and a two-character
subject is enough.

The answer Perex returns is the one the specification requires. Inside
`(?i: … )` the second alternative matches under folding and captures; the
backreference then compares that capture against the following character.

Because this is a measurement artifact in the oracle, it is removed by measuring
again rather than by editing an answer. `reference.mjs` exposes `freshAnswers`
and `stableDifferences`: every differential check compares as before, then
recomputes only the disagreeing cases in a fresh process and compares those
again. A difference that survives is reported unchanged and still fails the
check. The number of cases whose oracle answer was unstable is published in
each report as `unstable_oracle_answers`, so the condition stays visible instead
of disappearing. It is not an allow-list, and it binds no expected answer.

Observed with Node v26.5.1 and V8 14.6.202.34-node.24 on darwin arm64. The
pinned CI version is Node 26.8.1; this has not yet been re-checked there, and no
upstream report has been filed yet.

# Case-insensitive `v` string members: a V8 disagreement

Under `v` with `i`, the two engines answer a class whose member is a string of
one code point differently:

```js
/[\q{a}]/iv.test("A")   // Perex: true.  Node 26.5.1: false
/[\q{A}]/iv.test("a")   // both: true
/[\q{ab}]/iv.test("AB") // both: true
/[a]/iv.test("A")       // both: true
```

The specification makes a one-character member a character of the class, and a
character of a `v` class folds. `ClassSetOperand :: ClassStringDisjunction`
returns `MaybeSimpleCaseFolding(rer, charSet)`, which "maps each CharSetElement
of charSet character-by-character into a canonical form", so `\q{A}` under `i`
contributes the element `a`. Compiling `Atom :: CharacterClass` then takes "the
CharSet containing every CharSetElement of cs that consists of a single
character" and passes it to `CharacterSetMatcher`, which compares
`Canonicalize(rer, a)` with `Canonicalize(rer, char)` — the subject character is
canonicalized too. `A` therefore matches. Only elements of two or more
characters are compiled separately, as sequences.

V8's answers are not internally consistent with each other. It folds a
one-character member when an operator compares it — `/[\q{a}--[A]]/iv` does not
match `"a"`, so the subtraction saw `A` and `a` as the same member — and it
folds members of two or more characters when matching, but it does not fold a
one-character member when matching: `/[\q{A}]/iv` matches `"a"` and not `"A"`.
Whichever half is intended, the two cannot both be right, and the half that
agrees with `/[a]/iv` is the one Perex implements.

Eleven cases of `tools/check-sets.mjs` are this one difference: a one-character
`\q{…}` member alone, beside a longer member, inside a complement, and on either
side of `--` and `&&`. `tests/fixtures/sets-reference-disagreements.json` binds
each exact input with both engines' answers; the check reports them by default
and `--allow-reviewed-reference-disagreements` permits exactly those, failing if
either side's answer changes. Observed with Node v26.5.1 and V8
14.6.202.34-node.24 on darwin arm64. The spec text was read on 2026-09-16 from
the editor's draft; no third implementation was consulted, and no upstream
report has been filed yet. This does not edit the oracle or claim conformance
beyond the cases listed.
