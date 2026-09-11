# Validation bound to immutable owners

`BoundProgram` and `BoundSubject` validate original storage once and retain its owner plus fixed-size layout metadata. They retain no subject/program slice or movable address. `with_view` obtains a fresh scoped slice from that same owner and performs only length/representation checks, plus the program's nine-word header check. It does not scan the program tables or recount the subject. `BoundResources` combines independent bindings for `Search`, allowing one program binding to serve multiple subject bindings.

This is an experimental embedding boundary. It does not establish Perry's root, sharing or constructor invariants. A cached string length or an equal-sized allocation is not a validation certificate.

## Ownership and identity

The host implements `ImmutableProgram::with_words` and `ImmutableSubject::with_subject`. Every successful callback must expose exactly the original immutable words or bytes/units throughout the binding's lifetime. Collection may move storage between callbacks while preserving that identity and representation. The owner must keep the actual allocation rooted and enforce the string's sharing rules. A root that keeps a mutable buffer alive is insufficient.

View acquisition/release must not allocate, collect or call other host code. Program and subject getters may be nested, so a getter cannot collect another already-borrowed resource. The caller-supplied callback must also avoid collection while any view is live. A resource error must precede callback invocation; after invoking it, return its result successfully so a retry cannot replay work. Host coercion, allocation, cleanup and callbacks belong outside these scopes. The trait signatures prevent returning a view borrowed from the callback's argument; they do not implement a GC or a host effect system.

Each binding owns its storage value, which may itself be a borrow of a rooted owner. Failed construction returns that same value for explicit cleanup. `into_storage` consumes the binding and discards its validation metadata. There is no public operation that attaches cached metadata to an arbitrary new slice or replaces the bound owner. Ordinary byte/unit/word slices implement the storage traits without allocation.

The traits have semantic contracts, like the existing `Resources` trait. A broken implementation that substitutes equal-length content may cause wrong answers or a panic; fixed-size guards cannot prove full content identity. The core remains safe Rust with no unchecked memory access. Bounds/header guards are diagnostics for layout changes, not a substitute for the owner's immutability guarantee.

## Validation and work

Initial byte validation accepts the same generalized WTF-8 as `Input::wtf8`, including separate surrogate encodings, and rejects malformed bytes without repair or copying. Native UTF-16 requires no byte-validation pass. Program binding uses the complete format validator and the caller's work budget. Each later view uses constant core work and no allocation; initial validation, host getter work and collection costs remain separate costs to measure.

Initial validation is still synchronous. It must not be hidden inside every execution advance, and the eventual host must establish a bounded initialization/validation policy for large resources. These bindings do not yet solve initial-validation suspension, inline NaN-boxed strings, malformed Perry byte constructors, sharing enforcement, exception cleanup or per-owner host accounting.

## Verification

The resumption matrix validates bindings once, then relocates and poisons program/subject storage between advances. Complete captures and total matching work agree with uninterrupted execution for every tested quantum, including both directions and surrogate halves. Separate checks reject malformed initial input, invalid programs, exhausted program-validation work and unavailable owners; failed construction returns the owner. Layout/header/representation changes are rejected before the view callback runs. Compile-fail examples prevent either view from escaping its scope.

The development probe uses these bindings in its pause/relocate/grow modes. Its mock collector owns and moves its allocations; production traversal never copies the subject. Actual Perry GC and operation witnesses and CPU/RSS comparisons remain required before adoption.
