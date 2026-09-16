# Test262 pattern conformance

Perex is a pattern engine, not a JavaScript runtime, so it cannot execute
Test262. What it can use is the corpus. Test262's regular-expression tests
contain patterns written by hand to probe the grammar's edges — the cases a
generator does not reach because nobody thought to generate them.

`tools/check-test262.mjs` harvests those patterns, compiles each one with both
engines, and compares complete answers on a fixed subject matrix.

## Scope

This measures **pattern syntax and matching only**. `RegExp.prototype` methods,
`lastIndex`, the `Symbol.match`/`replace`/`split`/`search` protocols,
`RegExp.escape` and species construction are the embedding host's
responsibility. Their test files still contribute patterns to the harvest, but
their own semantics are out of scope here and are not claimed.

A passing run is therefore not "Perex passes Test262". It is: every pattern
Test262 writes down is accepted or rejected exactly as V8 accepts or rejects
it, and where both accept, both produce the same complete answer on the
subjects tried.

## Harvest

Extraction is deliberately conservative. A pattern is taken only from a form
whose bounds are unambiguous: a `RegExp` constructor call with literal string
arguments, or a regular-expression literal located with the grammar's own rules
for classes, escapes and the division ambiguity. Anything else is skipped and
counted, so coverage is a reported fact rather than an assumption.

Against Test262 `4249661388e5d3f92a85186213da140a6481490f`, from 2,117 files:
4,872 unique pattern/flag pairs, with 216 forms skipped (181 non-literal
constructor arguments, 20 escapes this extractor does not evaluate, 15
ambiguous slashes).

Each pattern runs against ten subjects covering the shapes the engine
distinguishes: empty, ASCII, digits and case, an embedded newline, astral
characters, a lone surrogate, combining marks, and text past the admission
threshold. That gives 48,720 cases.

## Result

| | |
|---|---|
| Cases compared | 48,248 |
| **Differences** | **0** |
| Syntax rejections, both engines | 3,390 |
| Syntax disagreements | **0** |
| Unsupported (Unicode sets) | 470 cases, 47 distinct patterns |
| Oracle could not finish | 2 cases |
| Unstable oracle answers | 0 |

Measured on Node v26.5.1 with `--work 64000000`; CI re-measures on the pinned
Node 26.8.1.

Syntax agreement is exact: of every harvested pattern, the two engines reject
the same 3,390 cases and accept the rest, with no pattern accepted by one and
rejected by the other.

The one gap is **Unicode sets**: 47 distinct patterns, which Perex reports as
an explicit unsupported feature rather than answering wrongly. Every one of
them names a property of strings — `\p{RGI_Emoji}`, `\p{Basic_Emoji}`,
`\p{Emoji_Keycap_Sequence}` and the rest — alone, in a union, or as an operand
of `--` or `&&`; six of those also use a string disjunction beside one. The
union grammar, the set operators, nested complements and string members
`\q{…}` are implemented; see [sets](sets.md) and
[issue #1](https://github.com/PerryTS/perex/issues/1).

## Cost, not correctness

Two cases are excluded because **the oracle** could not finish them:
`^(([a-z]+)*[a-z]\.)+[a-z]{2,}$` and its capturing variant. These are the
classic catastrophic-backtracking shape. On an 86-character subject Node did
not return within ten minutes and was stopped; Perex returns an explicit
work-limit outcome in bounded time. `reference.mjs` runs the oracle in bounded
child processes and bisects a timing-out chunk, so such a case is named and
excluded rather than hanging the check.

On the shorter subject where Node does finish, it answers in about 8 ms while
Perex needs about 22.0 million work units and 134 ms for the same answer. That
is a cost gap of roughly seventeen times on this pattern, not a wrong answer:
with an allowance that admits the work, Perex agrees with Node. The probe's
`--work` option sets that allowance explicitly, so an expensive pattern
produces a measured result instead of a hard-coded limit being mistaken for a
semantic difference. This pattern belongs in the performance inventory.

## Running it

Test262 is not vendored here, so the path is given:

```sh
cargo build --locked --release --example engine_probe
node tools/check-test262.mjs target/release/examples/engine_probe \
  /path/to/test262 artifacts/test262 --work 64000000
```

CI checks out `tc39/test262` at the pinned revision above. Changing that
revision changes the corpus, so the harvest counts in this document are tied to
it and must be re-measured when it moves.
