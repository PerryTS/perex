# Architecture

Perex is one independent ECMAScript regex compiler and matcher. The initial implementation will use one compact bytecode representation and evaluator. Native/AOT compilation is future work after the portable core passes semantic, memory and CPU gates.

The core will consume exact pattern text and flags, compile into immutable relocatable storage, and execute against a lossless subject view with explicit starting position and caller-controlled scratch. It will return match/capture spans or an explicit error. The implemented input/span API is described in `input.md`; compiler/matcher APIs and the binary format remain outstanding and are not represented by placeholder compile/find functions.

## Engine and host

The engine owns grammar, character/Unicode modes, assertions, backreferences, capture reset and ordering, matching and compiled representation. Initial flag/syntax coverage must be documented against a pinned ECMAScript/Unicode baseline before claiming conformance.

The host owns persistent storage, allocation policy, collection, reentrancy, JS objects and operation semantics. For a JavaScript host this includes `lastIndex` coercion/update, overridden exec methods, empty-match progress in string algorithms, replacement callbacks, result arrays and observable exception behavior.

Subject cursors must distinguish code-unit matching from Unicode matching. Public spans use UTF-16 coordinates. A byte-oriented fast path must preserve this contract, including half-pair matches, starts within pairs and reverse lookbehind traversal.

The production engine must traverse the original host string directly. It must not require conversion to another encoding or a copied substring to search, backtrack, compare backreferences or traverse lookbehind.

## Ownership

The same core supports explicit host-provided storage and, later, an ordinary owned-buffer convenience API. Neither interface installs another execution engine. Programs contain relative offsets/indices, not an untraced host-pointer graph. Scratch has explicit capacity, lifetime and cleanup rules. See `memory-contract.md`.

## Integration

Perry's adapter stays in Perry. A dependency should pin a tested Perex Git revision until releases exist; developers can override that dependency with a local path while working on both repositories. Do not copy the engine into a second tree or add a dependency before a useful matcher API exists.

Core tests and microbenchmarks run independently. Host tests exercise moving collection, object lifetimes, callbacks and complete application behavior. A passing engine corpus cannot certify a host adapter.
