# Named captures

The compiler implements named captures, named backreferences, forward/self references, and duplicate names permitted in different alternatives of an enclosing disjunction. It validates decoded Unicode identifier names in legacy and Unicode modes, using the shared Unicode 17.0.0 `ID_Start`/`ID_Continue` data with the ECMAScript additions `$`, `_`, ZWNJ and ZWJ. Names are case-sensitive and are not normalized, including under `i`. A duplicate name in the same alternative is a syntax error even if one declaration is optional or inside an assertion.

The grammar and duplicate-name rule follow [ECMAScript RegExp early errors](https://tc39.es/ecma262/multipage/text-processing.html#sec-patterns-static-semantics-early-errors). Legacy `\k` follows [Annex B pattern grammar](https://tc39.es/ecma262/multipage/additional-ecmascript-features-for-web-browsers.html#sec-regular-expressions-patterns): without named declarations it remains an identity escape. The compiler scans for later declarations only when a legacy `\k` requires that distinction. This scan borrows the pattern and does not run for ordinary patterns without that escape.

## Storage and execution

Format version 4 preserves the seven-word header and all existing instruction/class/repetition widths. Programs with names set a header flag and append:

- One name-count word.
- Three words per name: UTF-16 length, capture-index count and absolute payload offset.
- A contiguous payload for each name: two UTF-16 units per word (unused high half zero), followed by ascending u32 capture indices.

Names appear in first-declaration order, even when a forward reference occurred earlier. `Program::named_groups` borrows these names and lists. It allocates no objects or strings. A host constructs its own JS `groups`/indices objects from this metadata and the numeric capture outputs. Programs without names add zero words. Adding the one-character name `x` to `(a)\1` costs six words; its instructions and numeric registers stay identical.

Compiler scratch uses the existing `Node` arena for name metadata/declarations and the back of the caller's `Range` arena for decoded name points. Class records grow from the front, with checked collision detection. Pattern-derived names are packed into final program storage; the program retains no pattern or scratch borrow. Subject text is never copied into name metadata.

A unique named reference compiles to the existing numeric backreference instruction. A duplicate name uses one instruction that chooses its completed capture from the shared index list, then performs the same forward/reverse comparison on original subject storage. Unset references match empty; repetition/reset and assertion rollback use the existing numeric registers. More than one completed member at that instruction is an invalid program state.

Name interning currently compares interned names linearly. Duplicate-name validation checks each declaration pair using parent links and the lowest common ancestor in the AST; that ancestor must be an alternative node. This can cost quadratic work in the number of declarations, plus ancestor traversal. All comparisons, walks and serialization consume the compilation budget. These are explicit optimization targets, not a demonstrated compilation-performance win.

`Program::from_words` validates the optional section's bounds, contiguous layout, identifier encoding, capture indices and all named-reference operands under its budget. As with other instructions, this is format validation, not a proof that arbitrary hand-authored bytecode represents a legal source AST. The format remains unstable and native-endian. Release the program borrow before moving its storage and validate the new borrow.

## Evidence and remaining scope

`tools/check-names.mjs` compares 54,873 complete results with Node 26.8.1, with no exceptions or differences. It includes 5,252 Unicode identifier boundary values from pinned UCD input, escaped/raw names, invalid grammar, duplicate alternatives, 1,024 generated compositions, repeat resets, forward/self references, reverse execution, Unicode start positions, original capture units and 260 named captures. Names in the development JSON output are sorted as UTF-16 units to match the reference contract.

Five Rust tests exercise program/source/scratch lifetime separation, relocation with old program storage overwritten, borrowed WTF-8 and UTF-16 subjects, name ordering and capture-index lists, unchanged unnamed program size, resource errors and malformed/truncated tables. Existing engine and input tests remain required. The differential generator is development tooling, not a production fallback.

This closes named-capture support. Full grammar, efficient GC suspension/resumption, the Perry adapter, memory accounting and all-case CPU/RSS acceptance remain outstanding. No speed or RSS win is established by these correctness checks.
