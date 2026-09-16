# Compiler and evaluator

Perex has one compiler and ordered bytecode evaluator, and it is Perry's production regex engine. It is not a complete ECMAScript regular-expression implementation, and claims no conformance beyond what [conformance](conformance.md) measures. There are no matching-library dependencies or fallback routes. All core modules remain `no_std` and forbid unsafe code.

Compilation supports a one-call API and an explicit [prepared plan](compilation-plan.md).
Both use the same parser and emitter; preparation permits exact output allocation
after the host ends its pattern borrow.

## Implemented behavior

Literals, concatenation, ordered alternatives, numbered/named capturing and noncapturing groups, dot, ranges and negated classes, digit/word/whitespace escapes, anchors and word boundaries, hexadecimal/Unicode escapes, numbered/named backreferences, greedy/lazy/count-bounded repetition, and positive/negative lookahead/lookbehind execute through the same program and VM. Unicode and code-unit modes use the original input storage. Captures inside a repetition are reset on each iteration; empty iterations obey minimum-count/progress rules. Assertions are atomic, with positive captures retained and negative captures rolled back. Lookbehind traverses the subject and concatenation in reverse order.

Flags `i`, `m`, `s`, `u` and `y` affect the engine. `d` and `g` are accepted; captures are always available, and the host owns global iteration. `find` takes an explicit UTF-16 start whether or not `g` is present. The host must apply JS `lastIndex` semantics before calling it. [Case equivalence](casefold.md) uses pinned Unicode 17.0.0 data and preserves the different Unicode/legacy rules without folding or copying the subject.

[Legacy numeric/control escapes and quantified lookahead](legacy.md) follow Annex B, including octal/backreference ambiguity and distinct Unicode-mode syntax errors.

Unicode-mode [character properties](properties.md), including general categories, binary properties, Script and Script_Extensions, use shared immutable tables. Scoped `i`, `m` and `s` groups use lexical instruction choices, as described in [modifiers](modifiers.md). The `v` flag implements the class grammar — union, the set operators, nested complements, escaping and reserved punctuation, property escapes, string members, properties of strings and the complement-after-folding rule — as described in [sets](sets.md). Nothing of it is reported unsupported any more; `CompileError::Unsupported` remains a distinct outcome for a feature that is not implemented, and is not used for a pattern the grammar rejects.

## Memory and work

`compile` receives a borrowed pattern, caller-owned `Node`/`Range` scratch, final `u32` storage and a work budget. Nodes contain indices only. The emitter walks the topologically ordered arena iteratively: a long concatenation does not recurse on the native stack, and a counted repetition is not expanded into one program copy per count. Group parsing has an explicit nesting limit of 128; sizes/counters use checked arithmetic and u32 limits. These remain implementation limits, not claims that deeper/larger valid expressions are invalid JavaScript.

The output is one immutable relocatable buffer: an eleven-word header, three words per instruction, class-table pairs, eight-word repetition records, a filter section for the [properties of strings](sets.md), and an optional [name section](names.md). A class-table pair is a literal interval or shared Unicode property reference/complement. Capture indices are u32, with tests above 254 groups. The native-u32 format needs four-byte alignment and is versioned but unstable; version 13 adds the properties of strings — an instruction naming a shared set of sequences and the filter an operator left on it — retaining version 10's final-character descriptor for strict end-anchored patterns, version 9's [sorted literal classes](classes.md), version 8's candidate-start descriptor and version 7's consuming-atom repeat records and lexical flag instructions. Older versions are rejected; this is not cross-endian serialization. `Program::from_words` validates all opcodes, operand ranges, property IDs/modes and branch/table bounds, packed identifier names and capture-index lists under a work budget. It also validates the exact five-instruction shape and capture exclusion for optimized atom repeats. It does not prove arbitrary hand-authored bytecode has compiler-generated control structure or that a candidate/admission hint is a semantic consequence of its instructions; execution can reject invalid control/capture states and is always work-limited.

[Required-text admission](admission.md) can reject a long search when a compiler-proved literal/class condition is absent from the original subject. It uses existing instructions and header bits, without increasing program size. Presence still executes the same VM for complete answers.

[Candidate-start scanning](candidate.md) skips impossible positions in original ASCII storage before initializing match registers. A conservative compiler-derived first-character range adds one header word and no execution scratch buffer. Nullable patterns disable this scan. A possible start still enters the same VM, including assertions and captures.

`find` receives caller-owned register, backtracking-frame, undo-trail and capture-output slices. Stack/trail entries contain integer positions and register changes, not subject or program pointers. Execution-local cursor marks restore byte/UTF-16 positions in constant time on the same immutable borrow. They are private to the evaluator and are not an unchecked public resumption API. Straight matching without a saved choice does not need an undo history.

`is_match` answers only whether a match exists, which is what `RegExp.prototype.test` needs. It is the same evaluator over the same scratch and budget, and differs from `find` in one place: a match ends the search where `find` goes on to check the capture registers it is about to copy out. Nothing reads them, so nothing checks them, and a match is charged two work units per capture less; a miss is charged the same. Registers are still written, since backreferences read them. `Search::without_captures` makes a resumable search do the same, after which `Search::capture` and `Search::copy_captures` refuse with `ExecError::Captures`; `Search::restart_at` keeps the choice.

Against `find` on the same engine, in one process, interleaved, best of 21, with V8's `test` adjacent in time:

| Case | `find` | `is_match` | V8 `test` |
|---|---:|---:|---:|
| `/a/` against `"a"` | 50.0 ns | 41.5 ns | 18.2 ns |
| Sixteen-character literal | 57.0 ns | 51.0 ns | 15.3 ns |
| Short literal, 29 characters | 53.0 ns | 47.0 ns | 37.6 ns |
| Folded literal | 68.8 ns | 62.4 ns | 21.0 ns |
| Captures, 25 characters | 446.9 ns | 423.4 ns | 60.2 ns |
| `\w+!` over sixty | 174.9 ns | 152.1 ns | 36.5 ns |
| End-anchored hit, 256 KiB | 58.4 ns | 50.6 ns | 13.8 ns |
| `/z/` against `"a"` | 12.3 ns | 11.4 ns | 15.2 ns |

Matches gain up to 17 percent, the shortest the most: 3 percent on the short class case, and under 1 percent where a long scan dominates. No case is slower, and `find` itself is unchanged within noise. None of the matches it applies to moves ahead of V8's `test`; `/a/` against `"a"` is still 2.3 times it. The `Search` form saves the same checking round but not the copy, which a resumable search already leaves to the host; it was not timed separately.

[Consuming-atom repetitions](repetition.md) scan and retry with one frame per active repeat instead of a frame and register history per character. Capturing, compound and nullable repeated bodies retain the general instructions in the same evaluator. The optimization does not bound storage for arbitrary patterns or establish an application CPU/RSS improvement.

One `Budget` covers all start positions, assertions, table scans, backreference comparisons, initial seeks and rollback. Syntax, unsupported features, work exhaustion and each insufficient scratch/storage category are distinct. Capture output is untouched on no-match/error and committed only after a complete match and bounds checks; never consume output without `Ok(true)`. Scratch contents after a call are opaque reusable workspace, with no retained engine borrow or hidden cache.

The synchronous `find` API uses the same evaluator as the [resumable `Search`](resumption.md). Its trials — the loop that fetches and executes instructions — are compiled a second time for a search whose quantum is unbounded, which `find`, `is_match` and `Search::advance(usize::MAX)` all are: the same instructions, charges and phases, without comparing the work done against a quantum it can never reach before every instruction. A search with a finite quantum runs the first compilation, as before. An operation can retain partial work and release every input/program view between advances, then restore offsets against fresh validated views from a rooted immutable owner. Explicit replacement transfers live scratch to host-allocated buffers after frame/undo exhaustion, preserving the operation's remaining work. [Bindings](binding.md) avoid repeated program/input validation during view reacquisition. The host must enforce their identity, immutability and non-collecting-getter requirements; Perry's runtime adapter does, and its moving-collector witnesses exercise that boundary. Initial validation remains synchronous, and initial/backreference seeks in byte storage can be linear.

## Verification and gaps

The engine tests compare complete captures and cover ordered backtracking, capture reset, nullable repetitions, assertions/backreferences in both directions, surrogate modes, program relocation after source/scratch destruction, wide capture indices, long sequences, corruption and work/scratch exhaustion. The 15 engine tests pass in debug and release; the nine Rust input tests and independent Node input probe still exercise the underlying cursor. Deliberately disabling capture reset or swallowing execution errors fails the intended engine tests.

`tools/check-engine.mjs` compares the real Rust engine with Node over all 45 original reference fixtures, an authored pattern/subject/flag matrix, explicit starting positions and 512 deterministic generated compositions. It keeps unsupported answers and reference disagreements visible. Its default mode remains strict and fails while any difference exists. Development CI explicitly enables two reviewed lists: exact unsupported fixture answers and the separately explained [Node Unicode zero-width disagreements](reference-disagreements.md). The current 12,593-case run has 12,590 exact Node agreements, no unsupported answers and three reviewed Unicode disagreements. Changed coverage, any new unsupported case, and any changed reviewed answer fail. These lists do not establish full conformance and must not become adoption exclusions.

```sh
cargo build --locked --release --example engine_probe
node tools/check-engine.mjs target/release/examples/engine_probe --output artifacts/engine
# Development check, preserving and reporting known omissions/disagreement:
node tools/check-engine.mjs target/release/examples/engine_probe \
  --allow-listed-unsupported --allow-reviewed-reference-disagreements
```

`tools/check-casefold.mjs` additionally checks 93,404 complete answers against Node with no differences, including literals, complements and forward/reverse backreferences across Unicode casing edges. Original UTF-16, canonical WTF-8 and separate-surrogate byte storage have an explicit shared capture witness.

`tools/check-names.mjs` checks 54,873 complete answers against Node with no differences, including 5,252 Unicode identifier boundary values, duplicate-name grammar, forward/self references, capture resets and lookbehind. Five additional Rust tests check named metadata, relocation, original string storage and bounded failures.

`tools/check-legacy.mjs` adds 112,748 exact Node answers for 1,047 numeric spellings, forward/named capture counts, atom/quantifier boundaries, all 256 control-prefix suffix bytes and quantified assertions. Three Rust tests pin the corresponding interactions.

`tests/is_match.rs` requires `is_match` to give `find`'s answer, and to charge exactly two units per capture less on a match and the same on a miss, over eighteen patterns — captures, named and numbered backreferences, lookbehind captures, capture resets in a repeat, anchors, nullable and empty patterns, `u`, `v` and `y` — and fourteen subjects from every start through past the end. A boolean `Search` must give the same answer and charge the same work at quanta of one, seventeen and unbounded, with its storage moved and poisoned at every pause, and must refuse both capture reads; restarting must keep the choice. Five injected faults are each caught: `is_match` still checking captures, a boolean match reported as none, restarting dropping the choice, captures readable after a boolean search, and `without_captures` doing nothing.

The two compilations of a trial are held to each other by the resumption, position and boolean-search tests, which compare an unbounded search's answer, captures and charged work with a search paused at quanta of one and seventeen. A trial that charged each instruction twice when unbounded fails them, and one that charged nothing never finishes a backtracking case. Stopping an unbounded trial at a finite quantum, or giving a chained one none, only adds dispatcher rounds, and is not a fault.

Test262 pattern conformance is measured in [conformance](conformance.md). Still open: structured differential fuzzing; per-owner allocation accounting; and whole-application CPU/RSS comparisons in the host. Engine-level CPU figures are in [performance](performance.md).
