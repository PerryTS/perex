# Experimental compiler and evaluator

Perex now has one compiler and ordered bytecode evaluator. It is an implementation in progress, not a production replacement or a complete ECMAScript engine. There are no matching-library dependencies or fallback routes. All core modules remain `no_std` and forbid unsafe code.

## Implemented behavior

Literals, concatenation, ordered alternatives, numbered capturing and noncapturing groups, dot, ranges and negated classes, digit/word/whitespace escapes, anchors and word boundaries, hexadecimal/Unicode escapes, numbered backreferences, greedy/lazy/count-bounded repetition, and positive/negative lookahead/lookbehind execute through the same program and VM. Unicode and code-unit modes use the original input storage. Captures inside a repetition are reset on each iteration; empty iterations obey minimum-count/progress rules. Assertions are atomic, with positive captures retained and negative captures rolled back. Lookbehind traverses the subject and concatenation in reverse order.

Flags `i`, `m`, `s`, `u` and `y` affect the engine. `d` and `g` are accepted; captures are always available, and the host owns global iteration. `find` takes an explicit UTF-16 start whether or not `g` is present. The host must apply JS `lastIndex` semantics before calling it. [Case equivalence](casefold.md) uses pinned Unicode 17.0.0 data and preserves the different Unicode/legacy rules without folding or copying the subject.

Unicode-mode [character properties](properties.md), including general categories, binary properties, Script and Script_Extensions, use shared immutable tables. Current omissions include Unicode sets/string properties under `v`, named captures/backreferences, scoped flags, quantified assertions and parts of Annex B numeric/control escaping. They are reported as `CompileError::Unsupported`, not fabricated syntax errors or no-match. The parser is not yet a complete syntax validator for omitted features. Invalid duplicate names or invalid `v` expressions can therefore be reported unsupported until that grammar is implemented. No compatibility percentage should describe these outcomes as handled.

## Memory and work

`compile` receives a borrowed pattern, caller-owned `Node`/`Range` scratch, final `u32` storage and a work budget. Nodes contain indices only. The emitter walks the topologically ordered arena iteratively: a long concatenation does not recurse on the native stack, and a counted repetition is not expanded into one program copy per count. Group parsing has an explicit nesting limit of 128; sizes/counters use checked arithmetic and u32 limits. These remain implementation limits, not claims that deeper/larger valid expressions are invalid JavaScript.

The output is one immutable relocatable buffer: a seven-word header, three words per instruction, class-table pairs and eight-word repetition records. A class-table pair is a literal interval or shared Unicode property reference/complement. Capture indices are u32, with tests above 254 groups. The native-u32 format needs four-byte alignment and is versioned but unstable; version 3 includes property references and is not cross-endian serialization. `Program::from_words` validates all opcodes, operand ranges, property IDs/modes and branch/table bounds under a work budget. It does not prove arbitrary hand-authored bytecode has compiler-generated control structure; execution can reject invalid control/capture states and is always work-limited.

`find` receives caller-owned register, backtracking-frame, undo-trail and capture-output slices. Stack/trail entries contain integer positions and register changes, not subject or program pointers. Execution-local cursor marks restore byte/UTF-16 positions in constant time on the same immutable borrow. They are private to the evaluator and are not an unchecked public resumption API. Straight matching without a saved choice does not need an undo history.

One `Budget` covers all start positions, assertions, table scans, backreference comparisons, initial seeks and rollback. Syntax, unsupported features, work exhaustion and each insufficient scratch/storage category are distinct. Capture output is untouched on no-match/error and committed only after a complete match and bounds checks; never consume output without `Ok(true)`. Scratch contents after a call are opaque reusable workspace, with no retained engine borrow or hidden cache.

This initial API is synchronous: borrows end on return, and a finite work allowance bounds the operation. It does not yet suspend and resume through a moving safepoint or grow scratch mid-operation. A host can collect after an error and retry with new buffers, but replay is not a finished efficient resumption design. Program/input revalidation and non-ASCII re-seeking still require scans. Backreference capture offsets currently need a seek before comparison. These costs need removal or measurement before adoption, without copying subjects or hiding an unbounded GC pause.

## Verification and gaps

The engine tests compare complete captures and cover ordered backtracking, capture reset, nullable repetitions, assertions/backreferences in both directions, surrogate modes, program relocation after source/scratch destruction, wide capture indices, long sequences, corruption and work/scratch exhaustion. The 15 engine tests pass in debug and release; the nine Rust input tests and independent Node input probe still exercise the underlying cursor. Deliberately disabling capture reset or swallowing execution errors fails the intended engine tests.

`tools/check-engine.mjs` compares the real Rust engine with Node over all 45 original reference fixtures, an authored pattern/subject/flag matrix, explicit starting positions and 512 deterministic generated compositions. It keeps unsupported answers and reference disagreements visible. Its default mode remains strict and fails while any difference exists. Development CI explicitly enables two reviewed lists: exact unsupported fixture answers and the separately explained [Node Unicode zero-width disagreements](reference-disagreements.md). The current 12,593-case run has 12,582 exact Node agreements, eight explicit unsupported answers and three reviewed Unicode disagreements. Changed coverage, any new unsupported case, and any changed reviewed answer fail. These lists do not establish full conformance and must not become adoption exclusions.

```sh
cargo build --locked --release --example engine_probe
node tools/check-engine.mjs target/release/examples/engine_probe --output artifacts/engine
# Development check, preserving and reporting known omissions/disagreement:
node tools/check-engine.mjs target/release/examples/engine_probe \
  --allow-listed-unsupported --allow-reviewed-reference-disagreements
```

`tools/check-casefold.mjs` additionally checks 93,404 complete answers against Node with no differences, including literals, complements and forward/reverse backreferences across Unicode casing edges. Original UTF-16, canonical WTF-8 and separate-surrogate byte storage have an explicit shared capture witness.

Complete Unicode/grammar support, Test262, broader differential generation, interruption/resumption, per-owner allocation accounting, Perry's GC/operation adapter and the full per-case CPU/RSS comparisons remain required. This change establishes actual matching and its test boundary, not the performance goal.
