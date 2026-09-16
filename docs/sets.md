# Unicode sets (`v`)

The `v` flag is not `u` with extra operators. It reserves punctuation, requires
more escaping, nests classes, adds members that are strings rather than code
points, and complements a set *after* closing it under case folding instead of
testing each case equivalent. Treating it as `u` would give wrong answers, so
the parts that are not implemented report an explicit unsupported outcome
rather than reusing the `u` path.

## What is implemented

The class grammar: members, ranges, nested classes, class escapes and property
escapes, the set operators and nested complements, with the escaping and
reserved-punctuation rules the grammar requires.

- `[a]`, `[a-z]`, `[^a-z]`, `[a[b]c]`, `[[a-z][0-9]]`
- `[\d]`, `[\w]`, `[\s]` and their complements
- `\p{…}` and `\P{…}`, inside a class or alone
- `( ) [ ] { } / - \ |` rejected unless escaped
- doubled `&& !! ## $$ %% ** ++ ,, .. :: ;; << == >> ?? @@ ^^ `` ~~` rejected
- `\&`, `\-`, `\!`, `\#`, `\%`, `\,`, `\:`, `\;`, `\<`, `\=`, `\>`, `\@`,
  `` \` ``, `\~` accepted as escaped members
- `[[a-z]--[aeiou]]`, `[a--b--c]`, `[\p{L}&&\p{ASCII}]`, `[a&&b&&c]`
- `[[^a]]`, `[[^a]b]`, `[^[a-z]--[aeiou]]`

Members are appended to the same range scratch a `u` class uses, so the program
representation, class normalization and the evaluator's membership loop are
unchanged. A `v` class costs no more program storage than the equivalent `u`
class.

## The operators, and what they compile to

A property is stored as a reference to a shared table rather than as intervals,
so computing a difference or an intersection over one would mean materializing
every code point it admits. Nothing is materialized. Every operand matches
exactly one code point, so an operator is the constructs the engine already
has:

| Written | Built as |
|---|---|
| `[A--B]` | not `B` here, then `A` |
| `[A--B--C]` | not `B` here, not `C` here, then `A` |
| `[A&&B]` | `A` here, then `B` |
| `[A&&B&&C]` | (`A` here, then `B`) here, then `C` |
| `[[^A]]` | not `A` here, then any code point |

That is exact under `i` as well, and for the reason the section below gives: a
class matches a character when some case equivalent of it is a member, which is
membership of the set closed under folding, and `v` asks for the complement of
the closed set. No program word, opcode or evaluator rule is added, and a union
of ordinary members is still one class, so an ordinary `v` class costs exactly
what it did.

What it costs is an assertion at each position an operator class is tried. On a
10,500-unit subject whose match is at unit 19, `[[a-z]--[aeiou]]` charges 19
work units and 91 ns where the hand-written `[b-df-hj-np-tv-z]` charges 15 and
66 ns, and `[[a-z]&&[b-z]]` charges 15 and 80 ns.

The grammar's own rule that a range is not an operand is kept: `[a-z--[aeiou]]`
is a syntax error, as it is in V8, and the set it looks like is spelled
`[[a-z]--[aeiou]]`.

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

Tracked in [issue #1](https://github.com/PerryTS/perex/issues/1). These report `CompileError::Unsupported { feature: "Unicode sets" }` at the
offset where they appear. They are never answered as no-match, and never
reported as syntax errors.

- **String members**: `[\q{abc|de}]`. A class member spanning more than one
  character needs the evaluator to consume a variable number of characters and
  to order alternatives by length. An operator whose operand is one is
  unsupported with it.
- **Properties of strings**: `\p{RGI_Emoji}`, `\p{Basic_Emoji}`,
  `\p{Emoji_Keycap_Sequence}`, `\p{RGI_Emoji_Modifier_Sequence}`,
  `\p{RGI_Emoji_Flag_Sequence}`, `\p{RGI_Emoji_Tag_Sequence}`,
  `\p{RGI_Emoji_ZWJ_Sequence}`. These need both the string matching above and
  generated sequence data. Their names are known, so an unnegated one under `v`
  reports the gap; `\P` of one, or any of them under `u`, stays a syntax error,
  which is what the grammar requires.

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

At 16,770 cases: 16,250 compared, 0 differences, 4,342 patterns rejected by both
engines and none by only one, and 0 gaps declared on invalid patterns.

`tests/unicode_sets.rs` additionally answers the operators against the sets they
describe — subtraction and intersection over ranges, properties and nested
classes, chains of each, astral operands, the empty class, nested complements
inside and beside a union, a complemented operator class, and the `i` cases
where closure before the operator is what makes `v` differ from `u`.

`tests/unicode_sets.rs` asserts the three outcomes stay separated: what compiles,
what is an explicit gap, and what is a syntax error.

## Effect on the corpus

Across the patterns harvested from Test262 (see [conformance](conformance.md)),
the union grammar took the unsupported count from 161 patterns to 106, and the
operators and nested complements took it to 74: 47,998 cases compared, 0
differences, 3,410 patterns rejected by both engines and none by only one. The
74 that remain are 41 properties of strings and 33 string disjunctions, all of
them needing a member that matches more than one character.
