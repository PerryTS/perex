# Contributing

Perex currently has an experimental compiler/matcher, borrowed input cursors, capture spans, an embedding design, and executable engine/input/reference tests. Work toward the milestones in `docs/roadmap.md`; do not substitute stub matching results for missing implementation.

## Correctness

`tests/fixtures/core-cases.json` describes patterns, flags, subjects and UTF-16 starting positions. Valid matching cases use `d` so Node exposes complete capture ranges; `g` or `y` is required for nonzero starts. JSON escapes preserve lone-surrogate code units. The fixture format is described in `tests/README.md`.

`node tools/reference.mjs --check` executes the fixtures under Node and compares all answers against the committed JSONL. `--emit` writes answers to stdout. `--compare FILE` checks candidate answers against the reference, rejects duplicate/missing/extra IDs and exits nonzero on any disagreement. The comparator tests deliberately alter a capture and corrupt coverage to show those failures are detected.

To intentionally update fixtures, use the pinned Node version and run `node tools/reference.mjs --write`. Review fixture changes, expected answers and the generated receipt together. Never regenerate expected output just to hide a disagreement. Node is a differential reference; resolve semantic disputes using the selected ECMAScript specification and independent implementations. Expanding to Test262 and structured fuzzing remains required.

## Ownership and safety

Follow `docs/memory-contract.md`. Every allocation needs an explicit owner and lifetime. Do not add production engine fallback, implicit global caches or host GC dependencies. A raw pointer cannot substitute for a traced host reference across relocation.

The crate currently forbids unsafe code. If a later implementation needs a native boundary, propose a narrow documented safety boundary and fail-capable tests as part of that change; do not remove the restriction casually.

## Dependencies and attribution

The crate has no dependencies. Add implementation dependencies only for a concrete requirement, recording their memory behavior and supported targets. Preserve licenses, source revisions and notices when reusing upstream implementation or tests. No QuickJS, Irregexp, Yarr, regress or Test262 source is vendored in this initial repository.

## Measurements

Distinguish construction, first execution, warmed execution, long misses, successful captures, retained program bytes and peak/retained scratch. Correctness failures, cancellation and timeouts must remain visible. External application benchmarks belong to their host projects; do not commit private workloads, machine access information or credentials here.
