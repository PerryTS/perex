# Perex

An independent ECMAScript regex engine being developed for [Perry](https://github.com/PerryTS/perry) and other embedders.

**Status: experimental compiler, matcher, borrowed input and capture spans.** One evaluator implements core matching, numbered captures, repetition, assertions and backreferences using caller-owned storage and original subject bytes. Full Unicode/grammar support, efficient interruption/resumption and Perry integration remain outstanding. There is no demonstrated engine CPU/RSS win or production adoption. The crate has no dependencies, uses no standard library, and is not published.

Perex is designed around one matching engine and explicit host memory ownership:

- Immutable, relocatable compiled programs.
- Caller-controlled program storage and compilation/execution scratch.
- Exact JavaScript character and capture semantics, including lone surrogates.
- Integer match/capture spans instead of engine-owned result strings.
- Explicit syntax, memory, resource-limit and cancellation outcomes.
- No hidden process-global cache or second runtime heap.

The public API will be Rust. Implementation-language choices remain open. A future convenience API can use ordinary owned Rust buffers while executing the same core.

## Project boundary

| Perex owns | The embedding host owns |
|---|---|
| Pattern parsing, compilation and matching | GC roots, tracing, relocation and object layout |
| Unicode rules, captures and input cursor semantics | Program/scratch allocation policy and lifetime |
| Program representation and execution state | JS objects, `lastIndex`, callbacks and string operations |
| Engine conformance, fuzzing and microbenchmarks | Whole-application correctness, CPU and memory measurements |

Perex does not depend on `perry-runtime`. Perry will use a pinned Perex revision through a thin adapter. Local development can use a Cargo path override; there should be one authoritative engine source. No Perry dependency is installed yet; the experimental API has not met the full integration/adoption gates.

Start with the [implemented engine and its limits](docs/engine.md), [architecture](docs/architecture.md), the [memory contract](docs/memory-contract.md), and the [implementation milestones](docs/roadmap.md). The [research notes](docs/research.md) explain the source material and what existing engine tests do and do not establish.

The experimental [input API](docs/input.md) reads the original string, including individual surrogate halves inside four-byte UTF-8 characters. Capture spans borrow those units without constructing substrings. Validation, seeking and relocation costs are documented explicitly. The [performance requirements](docs/performance.md) preserve per-case CPU and RSS results alongside complete host measurements.

## Development

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo test --locked --release
python3 tools/generate-casefold.py --check
python3 tools/generate-properties.py --check
cargo build --locked --release --example input_probe
node tools/check-input.mjs target/release/examples/input_probe
cargo build --locked --release --example engine_probe
node tools/check-engine.mjs target/release/examples/engine_probe --allow-listed-unsupported --allow-reviewed-reference-disagreements
node tools/check-casefold.mjs target/release/examples/engine_probe
cargo build --locked --release --example property_probe
node tools/check-properties.mjs target/release/examples/property_probe target/release/examples/engine_probe --allow-reviewed-reference-disagreements
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
