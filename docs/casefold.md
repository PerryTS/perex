# Case equivalence on original input

The `i` flag uses ECMAScript's two [Canonicalize rules](https://tc39.es/ecma262/multipage/text-processing.html#sec-runtime-semantics-canonicalize-ch): Unicode mode uses common/simple CaseFolding mappings; legacy mode uses default uppercase conversion, retains multi-unit results unchanged, and forbids non-ASCII characters from mapping to ASCII. Locale-specific casing and full multi-character case folding do not implement these rules.

Unicode 17.0.0 inputs are pinned in `third_party/unicode/17.0.0`, including CaseFolding.txt, UnicodeData.txt, SpecialCasing.txt, the Unicode License V3 and a receipt with original URLs, byte lengths and SHA-256 hashes. `tools/generate-casefold.py --check` verifies each input and the generated Rust output without network access or Python's built-in Unicode mappings. It reconstructs the compressed equivalence cycles for all 1,114,112 Unicode values and all 65,536 legacy units, including unassigned and surrogate values. Each cycle is checked against the canonical mappings and has at most four members.

Only the generated tables enter the library: 320 Unicode rows (5,120 bytes) and 267 legacy rows (4,272 bytes), totaling 9,392 bytes of immutable native data, excluding code and slice descriptors. There is no initialization, writable global state, lazy heap allocation, per-pattern table copy, or subject-sized folded buffer. Original UCD text is development input, not embedded runtime text. This table payload is not a measured RSS delta.

Literals, ranges, negated classes and backreferences use this equivalence relation in the same evaluator. Ranges keep their original numeric endpoints; folding just the endpoints would be incorrect. Unicode ignore-case `\w` includes long s and Kelvin sign before computing `\W`, so complementing a class cannot incorrectly reintroduce those characters through a case equivalent. Word boundaries use the same rule. Backreferences compare original code points in Unicode mode, including reverse lookbehind; captures retain original UTF-16 spans and casing. The evaluator needs at most four temporary scalar values for a comparison, independently of string length.

`tools/check-casefold.mjs` obtains complete expected answers from Node, independently of the generated table. Inputs exercise Unicode folding and uppercase edges in both directions, nearby non-equivalents, classes/complements, captures/backreferences in both directions, built-in classes, boundaries, surrogate values, astral letters and interacting assertions/repetitions. The Unicode files choose inputs, not expected answers. Failures remain strict; this check has no exception list.

The current [program format](engine.md) encodes case folding in each literal,
class, boundary and backreference instruction, including [scoped modifiers](modifiers.md).
The format version also binds Unicode semantics; a later Unicode update must
explicitly version compatibility. Unicode character properties are implemented,
while Unicode sets/string properties (`v`), full conformance, host integration,
efficient suspension/resumption and per-case CPU/RSS acceptance remain outstanding.

## Small folded classes are closed at compile time

A folded class tested each subject character by computing its case equivalents
and searching the class for any of them, and that search is the resumable class
phase, so a repeated folded class could not use the inline scan or the byte run
either. `/[0-9a-f]{8}/i` cost about five hundred instructions a character.

Equivalence is symmetric — `equivalents` walks one cycle of a family from any
of its members — so a character has an equivalent in the class exactly when it
is in the class's case closure: every character some member is equivalent to.
A folded class of at most 256 characters with no property is therefore compiled
to its closure, sorted and merged, as an unfolded class. `[0-9a-f]` closes to
three ranges, `[a-z]` under `u` to `[A-Za-z\u017f\u212a]`. Negation applies to
membership either way and is kept. Larger classes, and classes holding a
property, keep folding at match time.

`folded_class_closure_matches_match_time_folding` in `tests/classes.rs` compares
each closed class with the same class plus the Private Use Area, which has no
case mappings and is too large to close, over the BMP's cased blocks and the
astral ones, plain and negated, under `i` and `iu`. Dropping one equivalent from
the closure fails it. `tools/check-casefold.mjs` and `tools/check-classes.mjs`
supply Node's answers for the same forms.
