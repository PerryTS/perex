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
