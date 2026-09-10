# Perex

An independent ECMAScript regex engine being developed for [Perry](https://github.com/PerryTS/perry) and other embedders.

**Status: project scaffold and semantic reference tests. There is no Perex compiler or matcher yet.** The crate currently builds without dependencies or the Rust standard library. This does not establish that the future engine is allocation-free, conformant, or fast. No crate release is published.

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

Perex does not depend on `perry-runtime`. Perry will use a pinned Perex revision through a thin adapter. Local development can use a Cargo path override; there should be one authoritative engine source. No Perry dependency is installed yet because Perex has no matching API.

Start with [architecture](docs/architecture.md), the [memory contract](docs/memory-contract.md), and the [implementation milestones](docs/roadmap.md). The [research notes](docs/research.md) explain the source material and what existing engine tests do and do not establish.

## Development

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
node tools/reference.mjs --check
node --test tools/reference.test.mjs
```

The semantic fixture oracle requires Node 26.8.1 in CI. The fixture receipt records the exact Node/V8/Unicode versions that generated the committed expected answers. These checks validate the reference fixtures and comparator; they **do not test an unimplemented Perex matcher**.

An engine harness can emit the documented JSONL answers and compare them using:

```sh
node tools/reference.mjs --compare /path/to/candidate.jsonl
```

See [contributing](CONTRIBUTING.md) for fixtures, source attribution and validation expectations. Licensed under MIT.
