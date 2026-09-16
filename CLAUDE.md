# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Perex is an independent ECMAScript regex engine (compiler + evaluator) built for embedders that own their own memory — chiefly the Perry JS runtime. `no_std`, no dependencies, `#![forbid(unsafe_code)]`, published on crates.io as `perex`, and Perry's only regex engine.

`AGENTS.md` holds the binding project constraints and `CONTRIBUTING.md` the fixture/measurement rules. Read both before changing behavior; the notes below do not replace them.

## Commands

Crate checks (all of these run in CI, `.github/workflows/ci.yml`):

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked            # debug
cargo test --locked --release  # release; some capacity/work tests differ
```

Single test file / single case (files in `tests/` are separate binaries):

```sh
cargo test --locked --test engine
cargo test --locked --test resumption relocation   # substring filter within that binary
```

Generated Unicode tables must stay in sync with pinned UCD input; `--check` verifies provenance, hashes and the exact generated file, and running without it rewrites `src/{casefold,property}_data.rs`:

```sh
python3 tools/generate-casefold.py --check
python3 tools/generate-properties.py --check
```

Differential checks against Node. These are the real correctness gate — the Rust tests alone are not. Build the probe first, then run the harnesses:

```sh
cargo build --locked --release --example engine_probe
node tools/check-engine.mjs target/release/examples/engine_probe \
  --allow-listed-unsupported --allow-reviewed-reference-disagreements
```

`check-engine.mjs` is **strict by default** and fails while any omission or disagreement exists; the two `--allow-…` flags are the development CI exception lists. The README lists the full harness set; the usage shapes differ:

- `PROBE [OUTPUT_DIR]`: `check-admission`, `check-casefold`, `check-legacy`, `check-modifiers`, `check-names`, `check-repetition`
- `PROBE OUTPUT_DIR [PROBE_ARGS...]`: `check-candidate`, `check-end-candidate`, `check-classes`, `check-unicode-sets`, `check-sequences`, `check-atom-filter` — CI runs each three times, plain and with `--quantum 1|17 --relocate --grow` (paused execution + simulated collector + scratch growth)
- `check-input.mjs PROBE` (needs `input_probe`), `check-properties.mjs PROPERTY_PROBE ENGINE_PROBE [OUTPUT_DIR]` (needs both `property_probe` and `engine_probe`)
- `node tools/reference.mjs --check` and `node --test tools/reference.test.mjs` validate the fixtures/comparator only; they do not exercise Perex.

CI pins Node 26.8.1, which is also the version recorded in `tests/fixtures/reference.json`. A different local Node can produce spurious disagreements.

Cost drivers (diagnostics, not adoption evidence): `examples/input_cost.rs`, `engine_cost.rs`, `scratch_cost.rs`.

## Architecture

One compiler, one program format, one evaluator. There is no fallback engine and must never be one.

**Pattern text → program.** `compiler::compile` parses into a caller-owned arena of `Node`/`Range` scratch (indices only, no source pointers survive) and emits a validated immutable `u32` program. `compiler::prepare` splits the same pipeline so the host can drop its pattern borrow before allocating final output: `Prepared::required_words()` gives the exact size, `emit` consumes the plan. Emission walks the topologically ordered arena iteratively — no native-stack recursion on long concatenations, no per-count expansion of `a{n}`. Submodules under `src/compiler/` add optimizer analyses (`admission`, `candidate`, `classes`, `repetition`) and grammar corners (`escapes` for Annex B, `names`).

**Program format** (`src/program.rs`). One relocatable buffer: header, three words per instruction, class-table pairs, eight-word repeat records, a filter section for properties of strings, optional packed name section. Everything is relative offsets/indices — never a host pointer. `Program::from_words` revalidates opcodes, operand ranges, property IDs, branch/table bounds and name/capture lists under a work budget, so a relocated or re-read buffer is always checked. `HEADER` and `VERSION` in `src/program.rs` are authoritative (currently 11 words / version 13); older versions are rejected outright, and a few docs still quote an earlier number. Word 9 (leading-literal run) is *re-derived* from the instructions during validation rather than trusted — hint words that can't be re-derived (words 7/8) are only allowed to be conservative, never to supply capture semantics.

**Subject access** (`src/input.rs`). `Input` borrows original storage — `&str`, generalized WTF-8 bytes (lone and separately-encoded surrogates accepted), or host `&[u16]`. **Never build a UTF-16 conversion buffer or copy the subject**; that is the core project constraint. All public positions count UTF-16 units, including positions between surrogate halves. ASCII byte storage gets a distinct internal representation with O(1) seeks; non-ASCII byte cursors pack a half-pair flag into the high bit of the byte offset.

**Evaluator** (`src/executor.rs`). An ordered backtracking VM over caller-supplied `Scratch` (registers, `Frame`s, `Undo` trail) — all integer offsets, no subject or program pointers. Two entry points share the identical VM:
- `find(...)` — synchronous, fixed buffers.
- `Search` — resumable. It holds no program or input view between `advance(quantum)` calls; it reacquires them from a `Resources` owner, so the host may relocate storage between advances. `rebuffer` transfers live state to larger host-allocated scratch after a frame/undo exhaustion request.

A single `Budget` covers a whole compile or search — it is never reset per start position or per assertion.

**Results** are `Span` values (UTF-16, end-exclusive; `None` ≠ empty span) written into caller storage, committed only on `Ok(true)`. `span::BoundSpan` reads capture units in bounded steps across owner relocation so a host can size an exact output string without a conversion buffer.

**Bindings** (`src/binding.rs`). `BoundProgram`/`BoundSubject`/`BoundResources` validate once and retain the owner plus fixed-size layout metadata, so reacquiring a view costs constant work instead of a full rescan. The immutability/identity guarantee is the host's semantic contract — length and header checks are diagnostics, not proof.

**Unicode data.** `src/property_data.rs` and `src/casefold_data.rs` are generated; do not hand-edit. Programs store shared property IDs, not copied intervals. Case equivalence never folds or copies the subject.

## Working in this repo

- **Never edit oracle answers to hide a disagreement.** Fixture regeneration (`node tools/reference.mjs --write`) is a deliberate, reviewed act on the pinned Node version; review fixtures, answers and receipt together.
- Keep the three outcomes distinct: syntax error, `CompileError::Unsupported`, and no-match. Reporting an unimplemented feature as a syntax error or a no-match is a correctness bug, and unsupported cases must stay visible rather than being counted as handled.
- Matching changes need complete-capture witnesses; memory changes need lifetime/relocation witnesses (relocate, poison and free the old storage, then continue). Each `docs/<feature>.md` ends with a Verification section naming its tests and harnesses — update the doc with the code.
- Optimizations (admission, candidate starts, leading literals, atom repeats, sorted classes) must run the same VM for the answer. They may only reject impossible work; they never choose between matches or produce captures.
- Do not claim compatibility, conformance percentages or performance that has not been measured, and preserve failed measurements rather than deleting them. `docs/performance.md` defines the per-case CPU/RSS gates; aggregate wins do not satisfy them.
- New dependencies, `unsafe`, implicit global caches, engine-owned result strings and host GC assumptions are all out of bounds. Perry-specific GC/adapter code belongs in Perry, not here.
- Bumping the program format means rejecting older versions and updating both the validator and the docs that quote the layout.
