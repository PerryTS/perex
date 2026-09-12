# Performance and adoption requirements

The target is one production regex engine that is correct for the host's entire regex workload and beats the compatible alternatives, or reaches CPU and RSS parity within the owner's accepted 10% variation. The tolerance applies to both measures across the covered cases; an aggregate improvement alone does not satisfy that target. Input-layer tests, a faster subset, an upstream engine's results, or a passing CI run are not adoption evidence.

## Comparisons that must remain visible

- Construction/validation, first execution and warmed execution, including short inputs where setup dominates.
- Positive and negative searches, sticky starts, complete captures and boolean tests that still require internal captures.
- Long misses, alternatives, assertions in both directions, backreferences, quantified capture reset and Unicode properties/sets.
- ASCII, multibyte text, astral characters, lone surrogates and positions between surrogate halves.
- Program storage, shared table footprint, compile scratch, live/peak/retained match scratch and host caches, as well as whole-process peak and settled RSS.
- Cancellation, resource exhaustion and allocation failure as explicit outcomes. A fast false no-match is a correctness failure.
- Moving collection, nested operations and result materialization in the actual host. The engine must traverse original subject storage without a converted copy.

Reference engines are development-only competitors. Record their exact revisions, flags, build configuration and complete answers. A competitor that rejects a pattern cannot provide a successful-match performance baseline for it; keep the rejection visible and use the engines that support the case. Do not install a production fallback to make coverage pass.

The host owns private workload capture, source/binary receipts and application measurements. Core performance cases should also be reproducible independently with authored or correctly attributed inputs. Maintain a per-case inventory: merging results into one median must not conceal a slower or higher-memory case. New real workload cases expand the inventory.

## Evidence before adoption

Run clean alternating comparisons on the same machine, preserve all clean outcomes and every failure, and account for measurement noise with repeated samples. Keep cold/warm and construction/execution results distinct. For each covered case, identify the best compatible measured competitor for CPU and for memory, even when these are different competitors. Losses outside the owner's 10% tolerance remain work to do; uncertain observations require stronger evidence. The old host campaign's aggregate thresholds or memory allowance do not override this project's objective.

Then repeat complete host-operation checks and whole-application CPU/RSS comparisons against the retained binary. Traceability must connect the measured binary to the exact Perex and host revisions. Engine microbenchmarks cannot prove a host improvement, and host totals cannot prove every engine case is faster.

No finite benchmark suite proves fastest behavior for all possible patterns, inputs and future competitors. The claim must state the actual tested workload, competitors and uncertainty; missing evidence remains incomplete work. The goal is not achieved until the real host cases, correctness, ownership and per-case performance requirements have verified evidence.

## Current measurement tool

`examples/input_cost.rs` decomposes the implemented input layer only. It creates one roughly 256 KiB subject and repeatedly validates or traverses that same allocation; seeking targets the middle. Cases cover ASCII, multibyte BMP, astral characters, separately encoded surrogate pairs, lone surrogates, original UTF-16 ASCII and original UTF-16 astral storage. Each constructs its original representation directly; the UTF-16 cases do not convert a byte subject for traversal.

Modes are `validate`, `forward`, `backward` and `seek`. Output includes a checksum, subject byte/unit lengths, iteration count, input/cursor state sizes and `loop_ns`. Subject construction and the initial input borrow precede that internal timer; process CPU and peak RSS measured externally also include setup. Keep both costs explicit and use the identical driver on both engines or revisions being compared. This is an input-cost diagnostic, not a whole-engine comparison or an adoption result:

```sh
cargo build --locked --release --example input_cost
/usr/bin/time -v target/release/examples/input_cost astral forward 2048
```

## Measured against V8, 2026-09-12

V8 is the engine Perry replaces, so it is the comparison that decides adoption.
Earlier comparisons in this project used `regress` and Rust `regex`; the latter
is a linear-time automaton with SIMD prefilters and answers a different
question. Both drivers measure process CPU time, because this host's load
average reached 103 during these runs, where wall clock measures how often the
scheduler ran a process rather than how much work it did. The figure for each
case is the minimum of three passes.

The harness, cases and raw results are the host's, under
`secret-tests/perex-bench`.

### Ahead

| Case | Perex | V8 |
|---|---:|---:|
| Required text absent, 4 KiB of `a` | 1.65 µs | 23.5 ms |
| Required text before its prefix | 1.60 µs | 23.4 ms |
| Class miss over 256 KiB | 27.3 µs | 901 µs |
| Long literal miss | 77.3 µs | 125 µs |

The first two are the catastrophic-backtracking shapes. Bounded work and the
admission bound are not a curiosity against a backtracking competitor: they are
the largest single advantage Perex has.

### Behind

Between 1.38x and 7.7x. The worst is a capture-heavy short match; the rest of
the spread sits between two and four.

| Case | Perex | V8 | |
|---|---:|---:|---:|
| Captures, 25-character subject | 450 ns | 58.5 ns | 7.7x |
| Three-way alternation, 256 KiB | 186 µs | 41.1 µs | 4.5x |
| Class repeat, short subject | 301 ns | 67.2 ns | 4.5x |
| Folded literal | 86.7 ns | 21.0 ns | 4.1x |
| End-anchored hit, 256 KiB | 54.0 ns | 17.1 ns | 3.2x |
| Literal lookbehind, 256 KiB | 122 ns | 41.0 ns | 3.0x |
| Short literal | 49.3 ns | 35.8 ns | 1.4x |

The diagnostics cluster between 1.5x and 4.3x whatever the pattern's length,
which is the flat per-search cost of interpreting rather than compiling. It is
visible most directly in the ladder below.

### The floor, and what it is

A diagnostic ladder separates fixed cost from marginal cost:

| Case | Perex | V8 |
|---|---:|---:|
| `/z/` against `""` | 16.6 ns | 11.2 ns |
| `/z/` against `"a"` | 23.6 ns | 13.8 ns |
| `/a/` against `"a"` | 46.6 ns | 16.9 ns |

An empty search is 1.48x. That is the cost of a bytecode pausable at every
instruction so the host's collector can run, which is why this engine exists;
V8 emits native code and owes a collector nothing. No further tuning of this
design removes it.

The scan is not where the remaining gap is. A single-pattern scan runs at
0.104 ns/byte against V8's 0.158 ns/byte for a three-pattern one — Perex
out-scans V8 per pattern. The gap is scanning several literals in one pass.

### Attempts that did not work

Recorded because a measurement that refuted a plausible idea is worth as much
as one that confirmed it, and because each of these looked obviously right.

- **Inline class test in the `CLASS` instruction arm.** No change: a repeated
  class is dispatched from the atom scan and never reaches that arm. Moving the
  test to the scan afterwards gave 1.6x.
- **Completing a pure literal without the trial loop.** Within noise; the
  remaining instructions cost about 1 ns each, not the 6.5 ns they cost before
  the leading-run skip landed.
- **A skip table over the leading literals.** 1.08x. Its dependent load chain
  is latency-bound where the existing lane arithmetic is not.
- **Caching the leading characters per scan step.** 1.5x *worse*, and worse
  again after the step was enlarged sixteenfold and the first-byte filter
  restored. Reading an instruction per character is not the cost that the
  arithmetic suggested it was.

### What closing the rest would take

Both remaining routes require removing `#![forbid(unsafe_code)]`:
`core::arch` intrinsics are unsafe, `core::simd` is nightly while CI pins
stable, and executing compiled code is unsafe. Auto-vectorisation does not
substitute, for the scan-rate reason above.

That is a safety decision about an engine a runtime points at untrusted input,
not a performance one, and it is not made here. It is tracked as [issue #2](https://github.com/PerryTS/perex/issues/2).
