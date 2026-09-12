# Unicode sets (`v`)

The `v` flag is not `u` with extra operators. It reserves punctuation, requires
more escaping, nests classes, adds members that are strings rather than code
points, and complements a set *after* closing it under case folding instead of
testing each case equivalent. Treating it as `u` would give wrong answers, so
the parts that are not implemented report an explicit unsupported outcome
rather than reusing the `u` path.

## What is implemented

The union grammar: members, ranges, nested classes, class escapes and property
escapes, with the escaping and reserved-punctuation rules the grammar requires.

- `[a]`, `[a-z]`, `[^a-z]`, `[a[b]c]`, `[[a-z][0-9]]`
- `[\d]`, `[\w]`, `[\s]` and their complements
- `\p{…}` and `\P{…}`, inside a class or alone
- `( ) [ ] { } / - \ |` rejected unless escaped
- doubled `&& !! ## $$ %% ** ++ ,, .. :: ;; << == >> ?? @@ ^^ `` ~~` rejected
- `\&`, `\-`, `\!`, `\#`, `\%`, `\,`, `\:`, `\;`, `\<`, `\=`, `\>`, `\@`,
  `` \` ``, `\~` accepted as escaped members

Members are appended to the same range scratch a `u` class uses, so the program
representation, class normalization and the evaluator's membership loop are
unchanged. A `v` class costs no more program storage than the equivalent `u`
class.

## The complement rule

Without `i`, `v` and `u` agree on every class this implementation accepts.
With `i` they differ, and only for a complemented property:

```js
/\P{Ll}/iu.test("a")   // true
/\P{Ll}/iv.test("a")   // false
```

Under `u` the match asks whether *some* case equivalent of the character is
outside the set. Under `v` the set is closed under case folding first, and the
complement then removes the whole closure — `a` folds into `Ll`, so `A` is
removed with it.

Property terms are stored as `(PROPERTY | id, kind)`. The kind is 0 for a plain
term, 1 for `\P` under `u`, and 2 for `\P` under `v`. The evaluator negates
membership of the entire closure for kind 2 and of each equivalent for kind 1.
The start-descriptor analysis uses the same rule, so a skipped start cannot
disagree with a trial. No program word is added; only a previously invalid
value of an existing word becomes meaningful, and `Program::from_words` rejects
anything above 2.

## What is not implemented

These report `CompileError::Unsupported { feature: "Unicode sets" }` at the
offset where they appear. They are never answered as no-match, and never
reported as syntax errors.

- **Set operators**: `[a--b]` subtraction and `[a&&b]` intersection. Computing
  them requires materializing a property's intervals into caller scratch, which
  changes what the host must size that scratch for.
- **String members**: `[\q{abc|de}]`. A class member spanning more than one
  character needs the evaluator to consume a variable number of characters and
  to order alternatives by length.
- **Properties of strings**: `\p{RGI_Emoji}`, `\p{Basic_Emoji}`,
  `\p{Emoji_Keycap_Sequence}`, `\p{RGI_Emoji_Modifier_Sequence}`,
  `\p{RGI_Emoji_Flag_Sequence}`, `\p{RGI_Emoji_Tag_Sequence}`,
  `\p{RGI_Emoji_ZWJ_Sequence}`. These need both the string matching above and
  generated sequence data. Their names are known, so an unnegated one under `v`
  reports the gap; `\P` of one, or any of them under `u`, stays a syntax error,
  which is what the grammar requires.
- **Nested complement**: `[[^a]]`, for the same materialization reason as the
  operators.

A gap is only ever declared for a pattern the grammar accepts. Declaring one
for an invalid pattern would hide a missing syntax error, so `tools/check-sets.mjs`
fails if that happens.

## Checks

`tools/check-sets.mjs` compares complete answers against Node over class bodies
covering members, ranges, nesting, every class and property escape, each
reserved punctuator singly and doubled, each syntax character escaped and
unescaped, character escapes, astral members and string members, against 26
subjects under `v`, `iv`, `gv`, `u` and `iu`. It compares syntax acceptance as
carefully as matching, because a new grammar most often fails by accepting what
the specification rejects.

At 16,770 cases: 15,782 compared, 0 differences, 4,342 patterns rejected by both
engines and none by only one, and 0 gaps declared on invalid patterns.

`tests/unicode_sets.rs` asserts the three outcomes stay separated: what compiles,
what is an explicit gap, and what is a syntax error.

## Effect on the corpus

Across the patterns harvested from Test262 (see [conformance](conformance.md)),
this took the unsupported count from 161 patterns to 106, added 550 cases to the
compared population and 350 to the patterns correctly rejected as syntax errors,
with differences remaining at 0. The 106 that remain are 41 properties of
strings, 33 string disjunctions, 16 subtractions and 16 intersections — that is,
74 needing string matching and 32 needing set operators.
