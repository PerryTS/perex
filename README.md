# Perex

An independent ECMAScript regex engine for [Perry](https://github.com/PerryTS/perry) and other embedders that own their memory.

Perex is Perry's only regular-expression engine: its runtime's `RegExp`, string methods and glob matching, and its CLI's own patterns, all run on it. One evaluator implements matching, numbered and named captures, repetition, scoped flags, assertions and backreferences over caller-owned storage and the original subject bytes. A search can pause at any instruction, release every borrow, and resume against storage a moving collector has relocated.

Against every pattern harvested from Test262 — 47,658 compared cases — Perex and V8 make the same syntax decision and produce the same complete answer on every one. The remaining gap is the `v` flag's set operators, string members and properties of strings, which Perex reports as unsupported rather than approximating; see [conformance](docs/conformance.md). The crate has no dependencies and uses no standard library.

Against V8, on the twenty-five authored cases in [`bench/`](bench/), Perex is at or better than V8 on every one when the faster of its two execution paths is taken — but taking it is the open part: nothing yet chooses between the interpreter and the [compilation tier](docs/compilation.md), and the tier is AArch64-only and loses badly on long subjects. [`docs/performance.md`](docs/performance.md) records the figures, the method, and every measurement that refuted an idea, which is the standard this project holds its own claims to.

Perex is designed around one matching engine and explicit host memory ownership:

- Immutable, relocatable compiled programs.
- Caller-controlled program storage and compilation/execution scratch.
- Exact JavaScript character and capture semantics, including lone surrogates.
- Integer match/capture spans instead of engine-owned result strings.
- Explicit syntax, memory, resource-limit and cancellation outcomes.
- No hidden process-global cache or second runtime heap.

The API is Rust and never allocates. An owned-buffer convenience layer belongs in the embedder over the same core, as Perry's adapter does.

## Using it

```sh
cargo add perex
```

The engine never allocates. A caller supplies the arena a pattern is parsed
into, the buffer its program is written to, and the registers, frames and undo
trail a search runs on:

```rust
use perex::{Budget, compiler::{Node, Range, compile}, executor::{Frame, Scratch, Undo, find},
            input::Input, span::Span};

let pattern = Input::utf8(r"(\w+)@(\w+)\.com");
let mut nodes = vec![Node::default(); 256];
let mut ranges = vec![Range::default(); 512];
let mut words = vec![0u32; 2048];
let mut budget = Budget::new(1_000_000);
let program = compile(pattern, "", &mut nodes, &mut ranges, &mut words, &mut budget).unwrap();

let mut registers = vec![0usize; program.register_count()];
let mut frames = vec![Frame::default(); 256];
let mut undo = vec![Undo::default(); 1024];
let mut captures = vec![None::<Span>; program.capture_count()];
let mut budget = Budget::new(1_000_000);

let found = find(
    program,
    Input::utf8("mail user@example.com now"),
    0,
    Scratch { registers: &mut registers, frames: &mut frames, undo: &mut undo },
    &mut captures,
    &mut budget,
).unwrap();

assert!(found);
assert_eq!(captures[0].map(|s| (s.start(), s.end())), Some((5, 21)));
assert_eq!(captures[1].map(|s| (s.start(), s.end())), Some((5, 9)));
```

That example is [`examples/basic.rs`](examples/basic.rs), so it is compiled by
`cargo check --all-targets` and cannot rot.

`Budget` bounds a whole compile or search and is never reset per start position,
so a pattern cannot spend unbounded time on a subject. The buffers above are
`Vec` for brevity; nothing requires them to be.

## Benchmarks

[`bench/`](bench/) holds two development-only drivers: a cross-engine comparison
against `regress`, `regex` and `fancy-regex`, and a comparison against V8 at
both of its entry points. A third maps and executes what the compilation tier
emits. See [`bench/README.md`](bench/README.md).

## Project boundary

| Perex owns | The embedding host owns |
|---|---|
| Pattern parsing, compilation and matching | GC roots, tracing, relocation and object layout |
| Unicode rules, captures and input cursor semantics | Program/scratch allocation policy and lifetime |
| Program representation and execution state | JS objects, `lastIndex`, callbacks and string operations |
| Engine conformance, fuzzing and microbenchmarks | Whole-application correctness, CPU and memory measurements |

Perex does not depend on `perry-runtime`. Perry depends on the published crate through thin adapters: a traced, relocation-aware one in its runtime and an owned-buffer one for its build tooling. This repository is the one authoritative engine source.

Start with the [implemented engine and its limits](docs/engine.md), [architecture](docs/architecture.md), the [memory contract](docs/memory-contract.md), and the [implementation milestones](docs/roadmap.md). The [research notes](docs/research.md) explain the source material and what existing engine tests do and do not establish.

The [compilation plan](docs/compilation-plan.md) separates parsing from final
storage allocation without retaining the original pattern view or resetting work.

The [input API](docs/input.md) reads the original string, including individual surrogate halves inside four-byte UTF-8 characters. Capture spans borrow those units without constructing substrings. `span::BoundSpan` traverses captures in bounded steps across owner relocation, including initial seeking, so a host can allocate exact output strings without a subject conversion buffer. Its consumer callback runs inside the input borrow; allocation and collection belong between steps. Validation, seeking and relocation costs are documented explicitly. The [performance requirements](docs/performance.md) preserve per-case CPU and RSS results alongside complete host measurements.

`BoundSpan::retarget` selects another span of the same immutable binding and reuses
the current offset when it shortens the seek. Adjacent reads need no prefix
rescan or subject index. Seeking still consumes the caller's cumulative work
budget in bounded steps; retargeting cannot revive a failed reader.

## Development

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo test --locked --release
python3 tools/generate-casefold.py --check
python3 tools/generate-properties.py --check
cargo run --locked --release --example compilable
cargo build --locked --release --example input_probe
node tools/check-input.mjs target/release/examples/input_probe
cargo build --locked --release --example engine_probe
node tools/check-engine.mjs target/release/examples/engine_probe --allow-listed-unsupported --allow-reviewed-reference-disagreements
node tools/check-casefold.mjs target/release/examples/engine_probe
node tools/check-names.mjs target/release/examples/engine_probe
node tools/check-legacy.mjs target/release/examples/engine_probe
node tools/check-admission.mjs target/release/examples/engine_probe
node tools/check-repetition.mjs target/release/examples/engine_probe
node tools/check-atom-filter.mjs target/release/examples/engine_probe artifacts/atom-filter
node tools/check-atom-filter.mjs target/release/examples/engine_probe artifacts/atom-filter-q1 --quantum 1 --relocate --grow
node tools/check-atom-filter.mjs target/release/examples/engine_probe artifacts/atom-filter-q17 --quantum 17 --relocate --grow
node tools/check-modifiers.mjs target/release/examples/engine_probe
node tools/check-unicode-sets.mjs target/release/examples/engine_probe artifacts/unicode-sets
node tools/check-sets.mjs target/release/examples/engine_probe artifacts/sets
node tools/check-candidate.mjs target/release/examples/engine_probe artifacts/candidate
node tools/check-end-candidate.mjs target/release/examples/engine_probe artifacts/end-candidate
node tools/check-end-candidate.mjs target/release/examples/engine_probe artifacts/end-candidate-q1 --quantum 1 --relocate --grow
node tools/check-end-candidate.mjs target/release/examples/engine_probe artifacts/end-candidate-q17 --quantum 17 --relocate --grow
node tools/check-candidate.mjs target/release/examples/engine_probe artifacts/candidate-q1 --quantum 1 --relocate --grow
node tools/check-candidate.mjs target/release/examples/engine_probe artifacts/candidate-q17 --quantum 17 --relocate --grow
node tools/check-classes.mjs target/release/examples/engine_probe artifacts/classes
node tools/check-classes.mjs target/release/examples/engine_probe artifacts/classes-q1 --quantum 1 --relocate --grow
node tools/check-classes.mjs target/release/examples/engine_probe artifacts/classes-q17 --quantum 17 --relocate --grow
cargo build --locked --release --example property_probe
node tools/check-properties.mjs target/release/examples/property_probe target/release/examples/engine_probe --allow-reviewed-reference-disagreements
node tools/check-test262.mjs target/release/examples/engine_probe /path/to/test262 artifacts/test262 --work 64000000
node tools/reference.mjs --check
node --test tools/reference.test.mjs
```

The semantic fixture oracle requires Node 26.8.1 in CI. The fixture receipt records the exact Node/V8/Unicode versions that generated the committed expected answers. `check-input.mjs` compares the implemented Perex cursors with Node string operations and Unicode RegExp starting positions. The separate `reference.mjs` checks validate the reference fixtures and comparator; they do not themselves test Perex matching. `check-engine.mjs` runs the actual matcher and reports its exact omissions and reviewed reference disagreement; strict mode is required before claiming full reference parity.

An engine harness can emit the documented JSONL answers and compare them using:

```sh
node tools/reference.mjs --compare /path/to/candidate.jsonl
```

See [contributing](CONTRIBUTING.md) for fixtures, source attribution and validation expectations. Licensed under MIT.

Case equivalence uses generated Unicode 17.0.0 data under the [Unicode License V3](third_party/unicode/17.0.0/LICENSE.txt). Pinned inputs, hashes and generation rules are documented in [case folding](docs/casefold.md).

[Unicode character properties](docs/properties.md) use shared immutable data and compact property references in programs. Their exhaustive membership check preserves the documented Node disagreement on the empty historical `Hrkt` script; strict mode remains available and fails on that discrepancy.

[Named captures](docs/names.md) keep packed names and numeric capture lists inside the relocatable program. Matching and backreferences continue to borrow the original subject.

[Required-text admission](docs/admission.md) uses a condition proved by the compiler and original subject storage. Its correctness and CPU/RSS costs require separate checks.

[Candidate-start scanning](docs/candidate.md) skips impossible ASCII start positions using the same evaluator and original storage. Complete-answer and relocation checks precede performance claims.

[Bounded end-anchored starts](docs/candidate.md) skip the whole prefix a match anchored at the subject's end cannot begin in, which removed a 256 KiB scan from 305,825 work units to 24.

[Unicode sets](docs/sets.md) implements the `v` union grammar, its escaping and reserved-punctuation rules, and its complement-after-folding rule. Set operators, string members and properties of strings remain explicit gaps rather than approximations.

[Test262 pattern conformance](docs/conformance.md) compares every pattern harvested from the suite's regular-expression tests against Node, for syntax acceptance and complete match answers. The one remaining gap is Unicode-sets class syntax.

[Leading literal starts](docs/leading.md) reject an impossible start with a byte comparison instead of an initialized trial. The claim is re-derived from the instructions during program validation, so it cannot disagree with them.

[Required-text admission](docs/admission.md) also bounds where a match can begin when the condition is one the match itself consumes, so text present only before every possible start stops the search instead of retrying it.

[Skipping a failed start's run](docs/repetition.md) removes the starts inside a leading atom repeat's run once one of them has failed, since each reaches a subset of the positions the first one tried.

[Single-atom repetition merging](docs/repetition.md) removes duplicate partitions when match ordering can be preserved, using the same evaluator and caller-owned storage.
