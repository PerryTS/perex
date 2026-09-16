# Lexical regular-expression flags

Scoped groups such as `(?i:a)`, `(?-i:a)` and `(?im-s:a)` change the case,
multiline and dot-all rules of their own subpattern. Nested groups inherit the
surrounding settings and restore them at their closing parenthesis. Only `i`,
`m` and `s` are allowed. Repetition within a list, overlap between additions and
removals, and `(?-:...)` are syntax errors. Nonempty additions with an empty
removal list, such as `(?i-:a)`, are valid. Unscoped `(?i)` is not accepted.
These rules follow the [ECMAScript grammar and early errors](https://tc39.es/ecma262/multipage/text-processing.html#sec-patterns-static-semantics-early-errors)
and [UpdateModifiers](https://tc39.es/ecma262/multipage/text-processing.html#sec-updatemodifiers).

## Compilation and execution

The parser temporarily updates its existing flag word while reading the group.
Leaves retain their effective flags in an existing arena field. Noncapturing
wrappers emit no instruction. The emitter selects ordinary or modified variants
of literals, classes, dot, anchors, word boundaries and numbered/named
backreferences. Global flags use these same instruction choices. Compiled
programs retain their original header flags for metadata and binding identity;
Unicode and sticky mode remain program-wide.

Format version 7 adds opcodes 19 through 26 for folded literals/classes, dot-all,
multiline start/end, folded boundaries, and folded numbered/named references.
Validation checks each variant's operands just as for its ordinary counterpart.
Equivalent global and scoped settings produce identical matching code and tables;
only the original flag header differs. Node, frame and undo layouts are unchanged.
There is no runtime flag stack, scope-entry/exit instruction or separate program.

Backreference case comparison is determined at the reference's lexical site,
not where its capture was made. After a paused seek the saved next instruction
PC identifies that site; subsequent comparison phases keep its fold choice.
Class checks retain the usual four-value equivalence set across pauses. Atoms
use their own instruction choices inside the existing bounded-repeat scan.
All reads and comparisons use original input storage, including reverse traversal
and lone surrogates, without a converted or folded copy.

## Required-text admission

A literal hint stops when its lexical case rule changes, even when the next
instruction is also a literal. Alternate branches can share a hint only when
their instruction choices and literal/class data agree. Folded hints use the
same existing equivalence rules as matching. This prevents a case-sensitive
prefilter from rejecting a valid match in an insensitive group, or a prefix
from crossing a group boundary with the wrong rule.

## Verification boundary

`tests/modifiers.rs` checks full captures, nested/global settings, backreference
case, Unicode word boundaries, multiline/dot-all behavior, syntax errors,
program equivalence and malformed modified instructions. Resumption tests pause
folded forward/reverse backreferences and named scans, move and poison old
storage, and grow caller-owned scratch without changing captures or work.

`tools/check-modifiers.mjs` generates complete Node answers for combinations of
scopes and global flags, nested additions/removals, assertions and references,
long admission subjects, surrogate inputs and invalid modifier lists. It has
no exception list. CI runs it alongside the existing full-answer checks.

The original `scoped-modifiers` fixture now agrees with Node; only that entry is
removed from the unsupported list. The reviewed reference differences remain. This feature does
not establish full ECMAScript conformance, actual Perry GC integration or the
required all-case CPU/RSS improvement.
