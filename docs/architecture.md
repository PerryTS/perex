# Architecture

Perex is one independent ECMAScript regex compiler and matcher, built on one compact bytecode representation and evaluator. An optional native [compilation tier](compilation.md) emits code for a subset of programs; the bytecode evaluator remains the definition of every answer.

The core consumes exact pattern text and flags, compiles supported syntax into immutable relocatable storage, and executes against a lossless subject view with explicit starting position and caller-controlled scratch. It returns match/capture spans or explicit errors. The implemented APIs and remaining semantics/storage limits are described in `engine.md` and `input.md`. The program format is versioned and native-endian: a program from another format version is rejected, and it is not a cross-endian serialization format.

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
