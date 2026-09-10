# Removing duplicate single-atom partitions

The compiler can merge nested greedy repetitions of one consuming atom into a
single repeat record. The evaluator and its borrowed subject access are unchanged.
This addresses redundant partitions such as `(?:(?:\w){0,2})+Q`; it does not make
arbitrary backtracking linear or establish a CPU/RSS win.

## Conditions and ordering

The body must reduce through noncapturing wrappers to one literal, character
class or dot. The inner repetition must be greedy and have minimum zero or one;
the outer repetition must also be greedy. An alternative whose first branch is
one such atom and whose second branch is empty is treated as a greedy optional
atom when an outer repetition is applied. Empty-first alternatives are excluded.

For this subset, nested matching tries distinct attainable end positions in
descending consumed-atom count. Other partitions revisit positions already tried,
with no capture changes inside the repeated body. The attainable counts form one
interval: the product of the two minima through the product of the two maxima.
Zero bounds dominate infinity. A finite product that does not fit the program's
u32 bound keeps its original nested representation.

This reasoning depends on the ordered continuation and empty-progress rules in
[ECMAScript RepeatMatcher](https://tc39.es/ecma262/multipage/text-processing.html#sec-repeatmatcher).
It is our compiler optimization, not a rewrite prescribed by the specification.
After a continuation fails, restoring the same input position and captures makes
another visit through a different partition redundant. Reverse traversal changes
the coordinate direction but keeps the same ordering by number of consumed atoms.

Larger inner minima cannot use this rule even when intervals overlap. For example,
`^((?:a{3,5}){0,2})(a*?)$` on seven `a` characters assigns five characters to the
first capture. Flattening its repetition to one interval could assign seven and
would be wrong. Capturing groups inside the repeated body, assertions,
backreferences, compound bodies and lazy choices also retain their original
programs. Captures surrounding an eligible repetition remain intact.

## Storage and limits

Compilation mutates the existing inner node and adjusts noncapturing wrapper
lengths. Removed empty alternatives contain no emitted instructions. It needs no
new parse arena, native recursion or program section. Each removed nested repeat
saves four instructions, one eight-word repeat record and two execution registers.
The general VM still accounts its backtracking frames and undo trail in caller
buffers; actual peak usage and process RSS need separate measurements.

The change preserves the existing nesting and size limits and charges wrapper
traversal to the compilation budget. It does not turn work exhaustion into a
no-match answer.

## Checks

`tests/repetition.rs` checks later matches from the four long-input failure
patterns, program/register reduction, exclusions with observable capture priority,
finite/zero/infinite bounds, reverse matching, and moved programs over original
UTF-8, WTF-8 and UTF-16 subject storage.

`tools/check-repetition.mjs` compares complete answers with Node across nested
finite/infinite/lazy quantifiers, surrounding captures and backreferences,
lookbehind, empty alternatives and Unicode/surrogate inputs. It has no exception
list. The unchanged `check-admission.mjs` retains all 64 newly discovered work-limit
failures as independent witnesses. The first Linux revision passed all 88,464 repetition cases and all 23,528 admission cases, closing the 64 failures. It also passed all 62,482 application-corpus answers. Further admission tuning requires its own checks and measurements; these counts do not establish full ECMAScript conformance or performance across all cases.
