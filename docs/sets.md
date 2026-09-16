# Unicode sets (`v`)

The `v` flag is not `u` with extra operators. It reserves punctuation, requires
more escaping, nests classes, adds members that are strings rather than code
points, and complements a set *after* closing it under case folding instead of
testing each case equivalent. Treating it as `u` would give wrong answers, so
the parts that are not implemented report an explicit unsupported outcome
rather than reusing the `u` path.

## What is implemented

The class grammar: members, ranges, nested classes, class escapes and property
escapes, the set operators and nested complements, string members, with the
escaping and reserved-punctuation rules the grammar requires.

- `[a]`, `[a-z]`, `[^a-z]`, `[a[b]c]`, `[[a-z][0-9]]`
- `[\d]`, `[\w]`, `[\s]` and their complements
- `\p{…}` and `\P{…}`, inside a class or alone
- `( ) [ ] { } / - \ |` rejected unless escaped
- doubled `&& !! ## $$ %% ** ++ ,, .. :: ;; << == >> ?? @@ ^^ `` ~~` rejected
- `\&`, `\-`, `\!`, `\#`, `\%`, `\,`, `\:`, `\;`, `\<`, `\=`, `\>`, `\@`,
  `` \` ``, `\~` accepted as escaped members
- `[[a-z]--[aeiou]]`, `[a--b--c]`, `[\p{L}&&\p{ASCII}]`, `[a&&b&&c]`
- `[[^a]]`, `[[^a]b]`, `[^[a-z]--[aeiou]]`
- `[\q{abc|de}]`, `[\q{ab}xy]`, `[\q{}]`, and the operators over them

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

## String members

`\q{…}` lists alternatives, and what each one contributes depends only on how
many code points it spells:

| Written | Contributes |
|---|---|
| `\q{ab}` | the string `ab` |
| `\q{a}` | the code point `a`, like any other member |
| `\q{}` | the empty string |

A class whose members are all code points is one class, exactly as before. A
class that also holds strings is an ordered alternation: its longer members
first, then the class of its code points, then the empty member if it has one.

```text
[\q{abc|ab}xy]   →   (?:abc|ab|[xy])
```

That is the specification's own construction. It takes the elements longer than
one character, sorts them by descending length, compiles each as a sequence of
single-character matchers, appends one matcher for every single-character
element, appends an empty matcher when the set holds the empty string, and
joins them with the ordinary two-alternative matcher. Ordered alternation is
therefore not an approximation of the rule, it is the rule — including the
backtracking it implies: `/[\q{abc|ab}]c/v` matches `abc` by trying `abc`,
failing on the `c` that follows it, and falling back to `ab`.

Because a member of one code point is a code point, it folds under `i` exactly
as a written character does, and an operator removes it exactly as it removes a
written character. V8 answers `/[\q{a}]/iv` differently; see
[reference disagreements](reference-disagreements.md).

A string costs one instruction per code point plus the split and jump of its
alternative. Nothing is materialized and no program word, opcode or evaluator
rule is added.

## Operators over strings

An operator asks a different question of each kind of member. Code points are
positional — every one of them consumes exactly one code point wherever it
appears — so the table above still applies to the code point part of an
operand. Strings are not: `[[a-z]--\q{ab}]` still matches `a`, because
removing the string `ab` from a set of code points removes nothing at all. So
the strings of the two operands are compared as text while the class body is
parsed, and only the code points are given an assertion:

| Written | Code points | Strings |
|---|---|---|
| `[A--B]` | not `B`'s code points here, then `A`'s | `A`'s strings that `B` does not spell |
| `[A&&B]` | `A`'s code points here, then `B`'s | the strings both spell |
| `[A B]` | one class where both are plain members | both lists, in written order |

Strings are compared by the text they spell, folded first under `i`, which is
what the specification's `MaybeSimpleCaseFolding` does to every element of a
set before an operator sees it. A string an operator removes never becomes
instructions.

`[^…]` refuses a set that may hold strings: `[^\q{ab}]` is a syntax error, and
so is `[^[\q{ab}]--[\q{ab}]]`, whose set has no members at all. That is the
grammar's `MayContainStrings` rule, decided from the syntax rather than from
what survives the operators, and it is tracked as the syntax states it — a
union may hold strings if any operand may, an intersection if every operand
may, a subtraction if its left operand may.

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

- **Properties of strings**: `\p{RGI_Emoji}`, `\p{Basic_Emoji}`,
  `\p{Emoji_Keycap_Sequence}`, `\p{RGI_Emoji_Modifier_Sequence}`,
  `\p{RGI_Emoji_Flag_Sequence}`, `\p{RGI_Emoji_Tag_Sequence}`,
  `\p{RGI_Emoji_ZWJ_Sequence}`. Their members are sequences the pinned Unicode
  data does not carry: the emoji sequence files are not among the inputs
  `third_party/unicode/17.0.0/receipt.json` records, and a set of some thousands
  of sequences does not fit the parse scratch a caller sizes from the pattern,
  so this needs generated data and a program reference to it rather than the
  string machinery above. Their names are known, so an unnegated one under `v`
  reports the gap; `\P` of one, or any of them under `u`, stays a syntax error,
  which is what the grammar requires.

A gap is only ever declared for a pattern the grammar accepts. Declaring one
for an invalid pattern would hide a missing syntax error, so `tools/check-sets.mjs`
fails if that happens.

## Checks

`tools/check-sets.mjs` compares complete answers against Node over class bodies
covering members, ranges, nesting, every class and property escape, each
reserved punctuator singly and doubled, each syntax character escaped and
unescaped, character escapes, astral members, string members and the operators
over them, against 27 subjects under `v`, `iv`, `gv`, `u` and `iu`. It compares
syntax acceptance as carefully as matching, because a new grammar most often
fails by accepting what the specification rejects.

At 21,840 cases: 21,710 compared, 7,150 patterns rejected by both engines and
none by only one, 0 gaps declared on invalid patterns, and 11 differences — all
of them one reviewed disagreement about the specification, bound case by case
in `tests/fixtures/sets-reference-disagreements.json` and explained in
[reference disagreements](reference-disagreements.md). Strict mode still
reports them; `--allow-reviewed-reference-disagreements` permits exactly those
inputs with exactly those two answers.

`tests/unicode_sets.rs` additionally answers the operators against the sets
they describe: subtraction and intersection over ranges, properties and nested
classes, chains of each, astral operands, the empty class, nested complements
inside and beside a union, a complemented operator class, and the `i` cases
where closure before the operator is what makes `v` differ from `u`. It answers
string members against the sequences they spell: the order between members of
different lengths, the fallback to a shorter member when what follows the class
fails, a member of one code point under `i`, the empty member, astral members,
quantifiers and lookbehind over a member, and each operator over strings.

`tests/unicode_sets.rs` asserts the three outcomes stay separated: what compiles,
what is an explicit gap, and what is a syntax error.

## Effect on the corpus

Across the patterns harvested from Test262 (see [conformance](conformance.md)),
the union grammar took the unsupported count from 161 patterns to 106, the
operators and nested complements took it to 74, and string members took it to
47: 48,248 cases compared, 0 differences, 3,390 patterns rejected by both
engines and none by only one. Every one of the 47 that remain names a property
of strings.
