# Consuming-atom repetitions

Two compiler optimizations operate within the same program and evaluator:
merging redundant nested partitions, described below, and representing a
consuming atom's scan and retry with bounded storage.

## One retry record per consuming repeat

After emission, the compiler identifies a repeat whose complete body is one
literal, class or dot, with no captures inside it. Its entry becomes
`ATOM_REPEAT` (opcode 18), and word 7 of its existing eight-word repeat record
becomes mode 1. The remaining four instructions and all program offsets stay
in place. Mode 0 continues to execute the general repeat instructions. Format
version 7 validates the optimized record's exact shape, consuming instruction,
empty capture range and entry/continuation relationship. This pass charges the
compiler's work budget. It does not yet reduce program size or register count.

A greedy repeat scans mandatory and optional atoms, saves the minimum endpoint,
and publishes one retry frame at the last accepted endpoint. After a failed
continuation it restores captures through the existing undo mechanism and
retreats one unit or point before trying the continuation again. Every traversed
position was already accepted by the same deterministic consuming atom, so this
retreat needs no class recheck or saved record per character. A lazy repeat first
consumes its minimum and tries the continuation, extending by one atom after
each failure until its bound or input is exhausted. Both directions use the
original input cursor, including positions between surrogate halves.

These are implementations of the repeat ordering discussed below. The restricted
body has no internal choice, capture reset or zero-length success to preserve.
Surrounding captures and assertions still use the general register/undo logic.
Compound, capturing and nullable bodies keep their general repeat instructions.
This is one compiler and evaluator, without another compiled matching program.

The scan retains counts, offsets and the cursor before the current attempted
atom. A pause during a class check preserves its range index. A failed attempt
restores its saved cursor. A scratch request while publishing a retry preserves
the completed scan or retreat, so replacement buffers resume only the pending
store. Cancellation and work exhaustion remain explicit errors.

Atom state shares enum storage with the admission literal buffer, whose work
has ended before matching begins. Frame PCs are checked u32 program indices;
subject positions and frame/undo indices remain full-width `usize`. On the
tested 64-bit target frames remain 48 bytes, and the shared work enum occupies
at most 40 bytes. This avoids increasing ordinary frame size to hold the repeat
limit. These are structure sizes, not a whole-process RSS measurement.

Focused tests match `^([a-z]+)$` through 65,536 characters with one frame and
two undo entries over original UTF-8 and UTF-16 storage. They compare complete
captures with the existing general instructions, exercise greedy/lazy/reverse
retries and surrogate halves, reject malformed records, and exhaust every work
allowance below successful completion. Resumption tests move and poison old
subject/program allocations, replace scratch, shrink it to live usage, and
cancel at observable pauses over lone-surrogate and separately encoded pair
storage. These tests do not establish full reference parity or a CPU/RSS win;
the broader compatibility and paired performance checks remain separate gates.

## Removing duplicate single-atom partitions

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
