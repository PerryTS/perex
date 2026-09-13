# Borrowed input and UTF-16 spans

`input::Input` borrows either the original byte slice or a host-supplied UTF-16 slice. It owns no string, conversion buffer, index or cache. Cursor movement never allocates or copies subject storage. `span::Span` contains only integer offsets; `Span::units` reads a bounded portion of the original subject without constructing a substring. An unset capture is `None`, which differs from an empty span.

```rust
use perex::{input::Input, span::Span};

let input = Input::utf8("a😀b");
let mut cursor = input.cursor_at(2).unwrap(); // between the surrogate halves
assert_eq!(cursor.next_unit(), Some(0xde00));
assert_eq!(cursor.previous_unit(), Some(0xde00));
assert!(cursor.normalize_unicode_start());
assert_eq!(cursor.position(), 1);
assert_eq!(cursor.next_point(), Some(0x1f600));

let capture = Span::new(2, 3).unwrap();
assert_eq!(capture.units(input).unwrap().next(), Some(0xde00));
```

## Encoding and coordinates

`Input::utf8` accepts a Rust `str`. `Input::wtf8` validates bytes, allowing surrogate-valued three-byte encodings without replacement or normalization. It also accepts separately encoded adjacent high/low surrogates, so its accepted byte language is generalized UTF-8, a superset of canonical WTF-8. Both four-byte and six-byte representations expose the same logical UTF-16 sequence. This extension is explicit: the [WTF-8 specification](https://wtf-8.codeberg.page/) excludes adjacent surrogate byte pairs from canonical WTF-8.

All public positions count UTF-16 units. `cursor_at` accepts the end position, rejects positions beyond the end, and preserves positions between surrogate halves. `next_unit` and `previous_unit` move one unit; `next_point` and `previous_point` combine a logical high/low pair when both units are available in that direction. An unpaired surrogate remains its numeric value; no Rust `char` is required to represent it.

`normalize_unicode_start` maps an initial position inside a pair back to the containing point. This implements the starting-position distinction in [RegExpBuiltinExec](https://tc39.es/ecma262/multipage/text-processing.html#sec-regexpbuiltinexec). It is separate from exact seeking and from the host's empty-match AdvanceStringIndex operation. Matching modes and host `lastIndex` semantics are not implemented by this input layer.

## Complexity and ownership

| Operation | Time | Additional subject storage |
|---|---|---|
| Borrow UTF-16 | O(1) | None |
| Construct UTF-8/WTF-8 view | O(bytes), count/validate | None |
| Move one unit or point, either direction | O(1) | None |
| Seek in ASCII bytes or UTF-16 | O(1) | None |
| Seek in other byte input | O(minimum UTF-16 distance from either end) | None |
| Copy cursor checkpoint during a borrow | O(1) | Fixed-size cursor state only |
| Read capture units | Initial seek plus O(capture units) | None |
| Reborrow after relocation | Current validation/counting plus initial seek | None |

A cursor contains a scoped subject borrow and integer positions, including an internal half-pair flag. Copying a cursor copies metadata, never the string. Cursors can be independently used for nested/reentrant operations. Across relocation the caller must release every view/cursor/span iterator, retain integer positions and a host root, and acquire a new borrow. The current safe API revalidates and re-seeks after relocation; it does not pretend that this potentially linear setup cost is solved. A future efficient host boundary must preserve validation and lifetime invariants while avoiding repeated scans. It must not copy the subject to do so.

ASCII byte storage has a distinct internal representation, selected during the existing validation/counting pass. Its reads use the original bytes directly. Non-ASCII byte cursors pack the half-pair flag into the highest bit of the physical byte offset; UTF-16 coordinates remain separate integers. Valid Rust slices occupy at most `isize::MAX` bytes, so that bit is free even at the endpoint without imposing an additional subject-length limit. See the [Rust slice validity contract](https://doc.rust-lang.org/std/slice/fn.from_raw_parts.html#safety); Perex does not construct slices using unsafe code.

Seeking through byte storage decodes each stored scalar once and accounts for its one or two UTF-16 units. It creates a half-pair checkpoint only when the destination splits a four-byte scalar. Native UTF-16 point reads inspect adjacent units directly. Both paths preserve exact unit positions, including separately encoded and unpaired surrogates, without a subject index or conversion buffer.

On the measured 64-bit Linux build, `Input` occupies 32 bytes, `Cursor` 48 bytes, a private execution checkpoint 16 bytes, and a backtracking `Frame` 48 bytes. Previously these occupied 40, 64, 24 and 56 bytes respectively. These are tested implementation sizes, not a stable ABI or a whole-process RSS result. A checkpoint copies only its two integer fields. Ordinary cursor restoration stays within one borrow; the [resumable evaluator](resumption.md) can reacquire a view of the same immutable representation through its rooted owner and restore a private checked checkpoint.

The Rust lifetime boundary prevents mutation/freeing of the subject during an ordinary safe borrow. Tests save positions, move the contents to a different backing allocation, poison and free the original, and resume against the new borrow. These are non-GC host tests, not proof that Perry's GC adapter traces or roots correctly.

## Verification scope

`tests/input.rs` checks forward/reverse walks and seeks, every code point including surrogate values, all short byte encodings, malformed encoding boundaries, every four-unit word over a surrogate-boundary alphabet, nested cursors and capture spans. Test-only encoders and UTF-16 reference logic are separate from the production byte decoder.

Private checkpoint tests cover every usable offset bit, endpoint values and restoration at every boundary of ASCII and surrogate-containing subjects in each original storage format. `tests/storage.rs` checks complete captures after program relocation and destruction of the original pattern/program storage, exercising half-pair checkpoints through alternatives, lookbehind, assertions, backreferences and repetition. Deliberately clearing the half-pair bit on checkpoint restoration fails the intended capture witness.

`tools/check-input.mjs` runs the actual Rust `input_probe` example against Node string access and sticky Unicode RegExp start positions. It includes the existing engine fixtures' pattern/subject strings and deterministic generated unit sequences, using canonical and separate-surrogate encodings. Its line-by-line comparison includes the value and resulting position of each forward/reverse read; missing or extra observations fail.

This evidence concerns input access. It is not an engine conformance result, allocation accounting for an unimplemented evaluator, or a CPU/RSS comparison against existing engines.
