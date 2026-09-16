# Validation bound to immutable owners

`BoundProgram` and `BoundSubject` validate original storage once and retain its owner plus fixed-size layout metadata. They retain no subject/program slice or movable address. `with_view` obtains a fresh scoped slice from that same owner and performs only length/representation checks, plus the program's eleven-word header check. It does not scan the program tables or recount the subject. `BoundResources` combines independent bindings for `Search`, allowing one program binding to serve multiple subject bindings.

This is the embedding boundary. It does not by itself establish a host's root, sharing or constructor invariants; the host must, as Perry's runtime adapter does. A cached string length or an equal-sized allocation is not a validation certificate: the two constructors in [binding what the host already validated](#binding-what-the-host-already-validated) accept one only because the host asserts it, and say so.

## Ownership and identity

The host implements `ImmutableProgram::with_words` and `ImmutableSubject::with_subject`. Every successful callback must expose exactly the original immutable words or bytes/units throughout the binding's lifetime. Collection may move storage between callbacks while preserving that identity and representation. The owner must keep the actual allocation rooted and enforce the string's sharing rules. A root that keeps a mutable buffer alive is insufficient.

View acquisition/release must not allocate, collect or call other host code. Program and subject getters may be nested, so a getter cannot collect another already-borrowed resource. The caller-supplied callback must also avoid collection while any view is live. A resource error must precede callback invocation; after invoking it, return its result successfully so a retry cannot replay work. Host coercion, allocation, cleanup and callbacks belong outside these scopes. The trait signatures prevent returning a view borrowed from the callback's argument; they do not implement a GC or a host effect system.

Each binding owns its storage value, which may itself be a borrow of a rooted owner. Failed construction returns that same value for explicit cleanup. `into_storage` consumes the binding and discards its validation metadata. Metadata is never attached to a new slice except through the two constructors below, whose contract is the host's. Ordinary byte/unit/word slices implement the storage traits without allocation.

The traits have semantic contracts, like the existing `Resources` trait. A broken implementation that substitutes equal-length content may cause wrong answers or a panic; fixed-size guards cannot prove full content identity. The core remains safe Rust with no unchecked memory access. Bounds/header guards are diagnostics for layout changes, not a substitute for the owner's immutability guarantee.

## Validation and work

Initial byte validation accepts the same generalized WTF-8 as `Input::wtf8`, including separate surrogate encodings, and rejects malformed bytes without repair or copying. Native UTF-16 requires no byte-validation pass. Program binding uses the complete format validator and the caller's work budget. Each later view uses constant core work and no allocation; initial validation, host getter work and collection costs remain separate costs to measure.

Initial validation is still synchronous. It must not be hidden inside every execution advance, and the eventual host must establish a bounded initialization/validation policy for large resources. These bindings do not yet solve initial-validation suspension, inline NaN-boxed strings, malformed Perry byte constructors, sharing enforcement, exception cleanup or per-owner host accounting.

## Binding what the host already validated

A host that searches once per call — a JavaScript `exec` or `matchAll` loop, or `test` in a loop — binds on every call. `BoundSubject::new` decodes the whole string each time and `BoundProgram::new` revalidates every word, so a loop over a long subject costs its length on every iteration. Perry measured those loops quadratic (Perry issues #10165 and #10166). Two constructors bind in constant work instead, and neither needs the host to keep a cache.

- `BoundSubject::new_counted(storage, utf16_len)` takes the UTF-16 length the host already holds. It checks only what is constant work: UTF-16 storage must have exactly that many units, and byte storage a length that many units could occupy, one to three bytes each. The representation follows the same rule as validation, ASCII exactly when bytes and units are equal in number.
- `BoundProgram::witness()` returns a `ProgramWitness`, the program's length and header as plain data, obtainable only from a binding that validated it. `BoundProgram::new_witnessed(storage, witness)` checks that length and header, as every view already does, and needs no budget. A host keeps the witness beside the program, in the object that owns it, and replaces it when the program is replaced.

Both refuse with `ChangedLayout` and return the storage when their check fails.

What neither checks is the host's to guarantee, in the same way immutability already is. The bytes must be generalized UTF-8 holding exactly `utf16_len` units, and the program words behind the witness's header must be the words that were validated. Storage that breaks that can give wrong answers or panic, never break memory safety: every access is still checked, safe Rust. For Perry that means its string constructors must produce valid WTF-8 with a correct length, which binding one of its strings this way now depends on.

Per call, on this machine, release build, best of five:

| Binding | Validated | Trusted |
|---|---:|---:|
| Program `/([a-z]+)([0-9]+)/`, 81 words | 68.7 ns | 1.7 ns |
| ASCII subject, 1,500 units | 664 ns | 0.5 ns |
| ASCII subject, 150,000 units | 66.2 µs | 0.6 ns |
| Non-ASCII subject, 1,600 units | 2.13 µs | 0.5 ns |
| Non-ASCII subject, 160,000 units | 213 µs | 0.8 ns |

This removes validation from a call. A non-ASCII subject still seeks to a search's start from its nearer end unless the search is given a [position](resumption.md#starting-from-a-position), and a position carried from one call to the next is only sound if the host can tell the string is the same one; that is the host's problem, and a wrong answer if it gets it wrong.

## Verification

The resumption matrix validates bindings once, then relocates and poisons program/subject storage between advances. Complete captures and total matching work agree with uninterrupted execution for every tested quantum, including both directions and surrogate halves. Separate checks reject malformed initial input, invalid programs, exhausted program-validation work and unavailable owners; failed construction returns the owner. Layout/header/representation changes are rejected before the view callback runs. Compile-fail examples prevent either view from escaping its scope.

`tests/counted.rs` covers the constant-work constructors. Every pattern in its set — over empty, ASCII, two-byte, astral and lone-surrogate WTF-8 and UTF-16 subjects, including a lone surrogate in UTF-16 — runs from every start over a validated binding and over a counted subject with a witnessed program, with storage relocated, poisoned and freed at every pause. The answers, complete captures, remaining work and positions must be identical, and a position from one binding must be accepted by the other. The refusals are checked for UTF-16 lengths that differ in either direction, byte lengths no unit count allows, and witnesses of programs with a different length or header, each returning the storage. A third test states the trust rather than implying it: invalid bytes and a program operand validation rejects are both accepted when the length and header fit, and each constructor borrows its owner exactly once. Five faults injected into the constructors — no unit check, no byte-length check, ASCII bytes bound as general bytes, no header check, and a witness that loses the program's length — are each caught.

The development probe uses these bindings in its pause/relocate/grow modes. Its mock collector owns and moves its allocations; production traversal never copies the subject. Actual Perry GC and operation witnesses and CPU/RSS comparisons remain required before adoption.
