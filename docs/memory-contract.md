# Host memory contract

This document specifies implementation requirements. The borrowed input/span layer is implemented; program storage, compiler and evaluator APIs remain outstanding. See `input.md` for the implemented layer's limits.

## Programs

A compiled program is a compact immutable buffer. Bytecode, capture layout and pattern-specific tables use checked offsets/indices within its storage. Programs contain no owning reference-counted graph, borrowed subject address or untraced host object. The host owns the buffer and can relocate it between execution borrows. Binary validation, alignment, sizes, versioning and maximum limits must be explicit.

A GC host traces its object-to-program edge and updates it during relocation. Perex does not implement a collector. Multiple host objects may share a program while retaining independent mutable state. Cache references have explicit lifetime and retention budgets; movable addresses are not stable cache identities.

Shared immutable Unicode data can reside in read-only native data. Report its footprint and version separately. Future static/AOT program storage needs an explicit lifetime/version contract of its own.

## Compilation and matching scratch

Compilation uses bounded, stable host-owned scratch and freezes the result into final program storage. Scratch contains no untraced host object references and is released on success, syntax error, cancellation and memory failure.

Matching uses an explicit context for registers, backtracking and capture offsets. Small inline storage, caller buffers and accounted growth can be supported. Retained capacity has a cap. Reentrant operations must use independent contexts.

Stable temporary memory need not be a moving GC allocation. It must have a visible owner, bounded lifetime, accounting and reliable cleanup. Allocation/accounting callbacks may trigger host collection; do not invoke them while holding unregistered interior pointers or an inconsistent owner.

## Subjects and results

Borrow subjects through explicit scopes. Support exact UTF-8/WTF-8 and UTF-16 semantics through a validated input design, including lone surrogates, positions within pairs, and reverse traversal. Never treat arbitrary WTF-8 as Rust `str`.

Traverse the original subject without copying it or building a UTF-16 conversion buffer. Direct ASCII borrowing should be cheap. Backtracking, lookbehind, backreferences and half-pair captures must use the same lossless input model. A UTF-16 input supplied by another host is supported as its original storage; it is not a conversion requirement for a byte-string host. Measure validation, forward/backward traversal, random access and relocation resumption independently. Test oracles may encode/copy data to produce an independent answer; production traversal may not require that copy.

Return integer spans and explicit unset captures into caller-owned storage. Return no durable interior pointer or engine-owned substring. The host materializes result objects using its allocator and applies its own string sharing/immutability rules.

## Relocation, callbacks and completion

End interior borrows before callbacks or a moving safepoint. Preserve program counter, subject position, registers and stack entries as offsets/indices plus rooted host handles; reacquire bases on resume. An alternative host pinning mechanism needs a documented lifetime and measured retention cost. Do not inhibit collection for an unbounded operation.

Expose match, no-match and distinct error outcomes. Exhaustion/cancellation cannot become no-match, a successful negative assertion or truncated output. Budget work over the full operation, including repeated searches and long forward/backreference work. Any native exception/unwind must respect cleanup boundaries. Host-specific mapping to JS abrupt completion belongs in the adapter.

## Required evidence

- Allocation totals and live/peak/retained bytes by owner: programs, compile scratch, match scratch, subject conversion, caches and tables.
- Correct behavior after relocation, cache eviction with live users, nested operations and cancellation.
- A host can reclaim a program after its final reference/cache root disappears.
- Missing-root, stale-base or cleanup-reversal witnesses fail when the corresponding protection is removed.
- Complete captures/errors match the semantic reference. Memory/correctness and application CPU/RSS are separate gates.
