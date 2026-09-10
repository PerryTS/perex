# Experimental resumable execution

`Search` retains one operation's offset state, exclusive caller-owned scratch, work budget and a reference to the host's rooted `Resources` owner. It holds no subject or program view between `advance` calls. The synchronous `find` function runs this same evaluator to completion; there is no second matcher or fallback route.

This implementation is under validation. Pause/resume and explicit scratch replacement are implemented experimentally. Efficient view reacquisition and Perry's adapter remain outstanding; this is not moving-GC adoption evidence.

## Borrow and identity boundary

A `Resources` implementation supplies validated `Program` and `Input` views inside `with_views`. Its callback result cannot contain a borrow of those views. A host can release the borrows, move allocations, update roots and reacquire current bases on the next advance. The owner must preserve the exact immutable program words and subject representation for the search's lifetime, including its sharing rules. Moving storage does not permit changing its contents, encoding or logical identity.

The implementation retains header/layout metadata and checks fresh views against it. Private cursor restoration also checks bounds, encoding boundaries and half-pair consistency. These checks do not establish that unrelated equal-length buffers are identical. Rooted identity and immutability are the owner's semantic contract; violating it can produce incorrect answers or a panic. The core remains safe Rust and does not introduce unchecked public view constructors.

The current safe byte/program constructors still scan to validate/count. A normal immutable embedder can retain validated borrowed views, but a moving host needs an established invariant that permits efficient reacquisition. Revalidating a whole subject/program on every pause is not an acceptable final host design and is not claimed solved here. The test collector deliberately revalidates to exercise correctness; its timings would include that extra work.

## Work and completion

`advance(quantum)` returns `Pending`, `Matched`, `NoMatch`, or an explicit error. A zero quantum performs no work. Positive quanta make progress without resetting the operation-wide `Budget`. Initialization, searching, seeking, class tables, duplicate-name selection, backreference comparison, capture reset, rollback and capture validation keep their partial state across calls.

The quantum is a work target, not a strict instruction count or wall-clock deadline. One bounded atomic step may exceed it. The largest current step is a 256-byte admission chunk plus at most 32 comparisons per candidate: at most 8,448 work units. Long seeks and comparisons are incremental; short constant-time seeks remain constant-time even at a pause boundary. View acquisition performed by `Resources` is outside this bound and must be accounted for by the host.

A successful search validates all capture ranges before exposing any. `capture(index)` reads one integer span without a subject borrow; `copy_captures` copies spans to sufficient caller storage. Pending, no-match, cancellation and errors leave caller output untouched. Span copying is a separate operation and holds no subject/program view. Completion, cancellation, exhausted work, invalid program state and changed resource layout are terminal: another `advance` reports the same result and cannot restart the operation. A resource-acquisition failure leaves execution state untouched and may be retried.

## Scratch ownership and growth

`Search` exclusively owns a `ScratchOwner`. Existing borrowed `Scratch` implements that trait, and an embedder can instead supply an owner of allocated buffers. Acquiring a scratch view must not allocate, collect, call host code or change live entries. The core allocates nothing and never grows storage implicitly.

Frame or undo exhaustion returns an explicit capacity request and retains the pending update. Further advances report the same request without spending work. `required_scratch` reports capacities sufficient to preserve live state and satisfy that request. The host may allocate replacement buffers outside the advance, then consume the search through `rebuffer`. That operation checks all capacities before writing and transfers live registers, frames and undo entries; it never copies the subject. On failure it returns both unchanged owners. On success it releases the previous scratch owner before returning and resumes the pending update without charging matching work again. Replacement allocation and old-owner cleanup may collect because no subject/program view survives either boundary.

Hosts choose growth, retention and allocation-failure policies and account for old/new buffer overlap. Cancellation stops execution; dropping the search releases its scratch owner and ends the root-owner borrow. The synchronous `find` remains a fixed-buffer call and reports insufficient capacity directly. Compilation does not yet support suspension or mid-operation growth.

## Verification

`tests/resumption.rs` compares complete captures and total work across quanta 1, 2, 3, 7, 31, 256 and uninterrupted execution. Its mock owner allocates new program/subject storage, poisons and frees the previous allocations between advances, and reacquires views under a Rust borrow guard. Cases exercise both directions, capture reset, nullable repetition, assertions, duplicate names, backreferences, UTF-8/UTF-16 and positions between surrogate halves. Long-operation cases cover admission, table scanning, initial/backreference seeks and rollback. Other witnesses cover zero quantum, unavailable resource borrows, cancellation, work/frame exhaustion and a changed resource layout. A compile-fail witness prevents a callback from leaking an input view.

The owned-scratch witness begins with no frames or undo entries and grows only after a capacity request. Allocation and destruction relocate the resources; old scratch is poisoned and released. It checks unchanged complete captures and total matching work, failure without partial destination writes, and owner counts that expose retained old allocations.

The complete-answer `engine_probe` additionally accepts `--quantum N [--relocate] [--grow]`. Growth starts with no frame/undo storage and keeps the ordinary probe's maximum capacities and work allowance. Relocation is a development collector simulation and copies its own backing allocations to test movement; production Perex never copies the subject for traversal. These modes are for differential correctness, not a performance baseline. The ordinary probe keeps its existing protocol.

Perry root tracing, exception cleanup, reentrant operations, collecting coercions/callbacks, cache ownership and allocation accounting need actual host tests. The one-time validation and resumption costs, pause latency, live/peak/retained storage and engine/application CPU/RSS remain separate performance gates. Compiler suspension is also not implemented by this matcher API.
