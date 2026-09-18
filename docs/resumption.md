# Resumable execution

`Search` retains one operation's offset state, exclusive caller-owned scratch, work budget and a reference to the host's rooted `Resources` owner. It holds no subject or program view between `advance` calls. The synchronous `find` function runs this same evaluator to completion; there is no second matcher or fallback route.

Pause/resume, explicit scratch replacement and constant-work owner-bound reborrowing are implemented. Perry's runtime runs its searches through `Search::advance`, pausing for its moving collector; those owner/GC witnesses live in Perry.

## Borrow and identity boundary

A `Resources` implementation supplies validated `Program` and `Input` views inside `with_views`. Its callback result cannot contain a borrow of those views. A host can release the borrows, move allocations, update roots and reacquire current bases on the next advance. The owner must preserve the exact immutable program words and subject representation for the search's lifetime, including its sharing rules. Moving storage does not permit changing its contents, encoding or logical identity.

The implementation retains header/layout metadata and checks fresh views against it. At the advance boundary, saved-current-cursor restoration also checks bounds, encoding boundaries and half-pair consistency. Within that borrow, private VM marks restore only offsets against the same immutable input. These checks do not establish that unrelated equal-length buffers are identical. Rooted identity and immutability are the owner's semantic contract; violating it can produce incorrect answers or a panic. The core remains safe Rust and does not introduce unchecked public view constructors.

The ordinary byte/program constructors scan to validate/count. [Owner-bound validation](binding.md) retains the immutable owner and initial metadata, then scopes new views without repeating those scans. The relocation matrix and development probe use that path. The host still has to establish the actual root/immutability and non-collecting-getter invariants. Initial validation remains synchronous and has a separate work/latency boundary.

## Work and completion

`advance(quantum)` returns `Pending`, `Matched`, `NoMatch`, or an explicit error. A zero quantum performs no work. Positive quanta make progress without resetting the operation-wide `Budget`. Initialization, searching, seeking, class tables, duplicate-name selection, backreference comparison, capture reset, rollback and capture validation keep their partial state across calls.

The quantum is a work target, not a strict instruction count or wall-clock deadline. One bounded atomic step may exceed it. The largest current step is a 256-byte admission chunk plus at most 32 comparisons per candidate: at most 8,448 work units. Long seeks and comparisons are incremental; short constant-time seeks remain constant-time even at a pause boundary. View acquisition performed by `Resources` is outside this bound and must be accounted for by the host.

A successful search validates all capture ranges before exposing any. `capture(index)` reads one integer span without a subject borrow; `copy_captures` copies spans to sufficient caller storage. Pending, no-match, cancellation and errors leave caller output untouched. Span copying is a separate operation and holds no subject/program view. Completion, cancellation, exhausted work, invalid program state and changed resource layout are terminal: another `advance` reports the same result and cannot restart the operation. A resource-acquisition failure leaves execution state untouched and may be retried.

## Starting from a position

On ASCII and UTF-16 storage a search reaches its start in constant work. On
other byte storage it seeks from the subject's nearer end, which costs up to half
the subject. That is fine for one search and quadratic for a loop of them: a
global `replace`, `matchAll` or `exec` loop starts a search at every match, and
`split` starts a sticky one at every position. Perry hit this as a work-limit
`RangeError` on a 32,000-unit `split` and a 60,000-unit `replace` (Perry issues
#10164 and #10165), where a seek from an end summed to about n²/4 units against a
100,000,000-unit allowance.

`input::Position` is a UTF-16 position together with where it lies in one
subject's storage: a byte offset with the half-pair flag, a UTF-16 offset, and
the subject's layout. It is offsets only, so it survives relocation like every
other piece of resumable state.

- `Search::position()` is the match's end once a search has matched; otherwise
  the start of the last attempt it made — for a sticky search, its requested
  start — and before it has reached a start, the position it was given, or the
  subject's beginning.
- `Search::new_near(resources, start, near, buffers, budget)` is `Search::new`,
  except the seek to `start` begins at `near` when that is nearer than both
  ends. It costs the distance plus one, charged per unit and paused like any
  other seek. The nearer of the three is always taken, so a hint is never more
  expensive than none.
- `BoundSpan::new_near` and `BoundSpan::position` do the same for reading a
  capture's units, so materializing a match's captures seeks back by the match's
  length from the search's position instead of from an end.

A position from a subject of another layout is refused, as
`ExecError::ChangedResources` from `new_near` and `ReadError::ChangedPosition`
from a reader. So is one that is not a valid position in the subject it is used
on — inside a scalar, or a half-pair flag where no astral character is — when it
is used. Two different strings with identical layouts cannot be told apart, so a
position used on the wrong one can give wrong answers, never unsafety: the same
contract a `Resources` owner already carries. A host should keep a position only
for as long as it keeps the binding it came from.

What it buys, in work units, from `tests/position.rs`:

| Loop | Subject | From the ends | From positions |
|---|---|---:|---:|
| Global search for `a` over `ééé a ` | 2,000 repeats | 6,060,001 | 60,001 |
| | 4,000 repeats | 24,120,001 | 120,001 |
| Sticky `[,;]` at every position over `ééé,aé;` | 1,000 repeats | 12,307,000 | 62,000 |
| | 2,000 repeats | 49,114,000 | 124,000 |

Doubling the subject doubles the work from positions and quadruples it from the
ends.

### Searching again without rebuilding

A host walking every match of one subject runs one search per match. Building a
`Search` for each re-acquires both owners to read their shape and re-checks the
scratch — work the next `advance` does again anyway. `Search::restart_at(start)`
reuses the operation's shape, scratch owner and remaining budget, and starts
from where the last search left off, so a loop pays for neither. The answer, the
work charged and the position are what a new search from `Search::position`
produces; a completed, cancelled or failed search restarts alike, and a pending
capacity request does not survive, so captures are read first.

Per match, best of five on a 1.1 MB ASCII subject with 200,000 matches, on a
machine under other load, so treat the nanoseconds as approximate and the ratio
as the result:

| Pattern | A `Search` per match | One restarted | Synchronous `find` |
|---|---:|---:|---:|
| `/[0-9]+/g` | 139 ns | 112 ns | 95 ns |
| `/([a-z]+)([0-9]+)/g` | 197 ns | 169 ns | 154 ns |

That removes about a fifth of the per-match cost of the resumable path. What
was left was the view acquisition and the state transfer each advance does, and
the transfer is now gone too: the evaluator borrows the operation's state
instead of copying its few hundred bytes in and out every advance. Alternating
binaries under load, that took a further 10 to 20 ns off each search on this
path — a `Search` per match from 150-198 ns to 144-157, one restarted per match
from 123-131 to 115-126 — while the synchronous `find`, which keeps its state
locally, and the generated code did not move.

That result does not survive being compiled into a host. Perry measured the same
two revisions in its own workspace, with its own codegen and link settings and
one advance per search, and the change costs it 20 to 66 instructions per call:
a hoisted `test` 4,835.3 to 4,855.3, an `exec` reading a capture 8,016.1 to
8,039.1, a non-ASCII `test` 5,715.0 to 5,781.0, each reproduced to 0.2
instructions by rebuilding both arms. This crate's own `examples/call_cost`
moves the other way at the same two revisions, 3,825 to 3,767 instructions for
a matching call and 1,031 to 968 for a missing one. The likely reason is that
inlining differs between the two builds and the borrow stops being elided, but
that is a hypothesis: nobody has read the two disassemblies. Where the change
pays is a loop with several advances per search; where it is measured to cost,
it costs a fraction of a percent.

A host can also lend its scratch instead of giving it up: `&mut O` is a
`ScratchOwner` wherever `O` is one, so a search holds a pointer to the host's
buffers rather than a copy. A host that keeps its scratch across calls — a pool,
a per-thread buffer — then builds and moves nothing per search, and the borrow
is what stops a nested search from sharing live scratch with the one it
interrupted; a compile-fail example in the trait's documentation fixes that.
Perry measured a 336-byte structure move per call into `Search` from building
its buffers each time (Perry issue #10166), which is what this removes. It makes
no measurable difference in a Rust microbenchmark, where the move is within one
frame and the compiler elides it; the cost is in a host that constructs the
owner per call.

A host that resumes after an empty match must advance the way the specification
does: one code point under the `u` flag, one unit otherwise. Advancing a single
unit inside a surrogate pair does not make progress, because a `u` search
normalizes its start back to the pair, and the same empty match is found again.

Two costs remain per search and are not changed by this. A program with a
required-text condition, on a subject of 64 units or more, runs admission from
the subject's beginning to that text's first occurrence; that is cheap where the
text occurs early and costs the distance where it first occurs late. And binding
a byte subject validates the whole string, which is the host's to do once per
operation rather than once per search.

### Deciding a search in one call

`Search::new` acquires both owners to learn the operation's shape, and
`Search::advance` acquires them again to run. A host that starts a search per
call, as a JavaScript `test` or `exec` does, usually sees it decided within its
first quantum, so it pays for both acquisitions and for moving a `Search` it
then drops — 360 bytes of state, shape and scratch handles.

`Search::run(resources, start, near, buffers, budget, quantum)` acquires them
once and runs the first quantum in the same borrow. If that decides the search,
it returns `Run::Finished`, which holds the answer, the position, the work left
and the scratch, and reads captures as a `Search` would; no `Search` is built.
Otherwise it returns `Run::Paused` with a `Search` that continues exactly as
`Search::new_near` — or `Search::new` without `near` — followed by `advance`
would have: the same pending update after a capacity request, the same answer,
captures, position and charged work. `Search::run_without_captures` does the
same for a search built with `without_captures`.

Instructions per call, from `examples/call_cost` at one and two million calls,
with per-call constant-work bindings and quantum 4096, the shape of Perry's lent
path:

| Case | `Search::new` + `advance` | `Search::run` | Reading captures, before | after |
|---|---:|---:|---:|---:|
| `/a/` against `"a"` | 1,496 | 1,284 | 1,575 | 1,333 |
| `/z/` against `"a"` | 775 | 572 | 782 | 572 |
| `/needle/`, 29 characters | 1,688 | 1,465 | 1,756 | 1,517 |
| `/[a-z]+!/`, 61 characters | 2,672 | 2,445 | 2,742 | 2,500 |
| `(\w+)@(\w+)\.com`, 25 characters | 7,700 | 7,477 | 7,804 | 7,559 |

Between 200 and 245 instructions a call, whatever the pattern: it is the call's
fixed cost, not its matching. That is a measurement of this crate's driver, and
the last change of this kind — borrowing the operation's state instead of moving
it — gained here and cost Perry 20 to 66 instructions per call once compiled
into its runtime, so the number that decides is the host's.

Perry's is larger. Measured in Perry at its merge train 214 (`c8cf45056`),
release build, both arms on the same Perry commit and the same commit of this
crate, so the patch adopting `Search::run` is the only difference between them.
Instructions per call, five interleaved rounds, the minimum per cell, with a
control binary's instructions subtracted so the figure is the regex call alone:

| Perry call | `Search::new` + `advance` | `Search::run` | |
|---|---:|---:|---:|
| `.test()`, hoisted pattern | 4,382.2 | 4,063.7 | −7.3% |
| `.test()`, literal pattern | 5,698.2 | 5,379.7 | −5.6% |
| `exec`, two groups | 7,566.9 | 7,237.9 | −4.3% |
| `.test()`, unanchored miss | 2,438.6 | 2,133.7 | −12.5% |
| `.test()`, unanchored hit | 5,813.2 | 5,481.2 | −5.7% |
| 200,000-character subject | 1,439,715 | 1,439,003 | −0.0% |

305 to 332 instructions a call whatever the pattern, and nothing on a subject
long enough that matching dominates: it is the call's fixed cost, and more of it
than this crate's driver shows, because a host acquires its views through its
own owners — Perry's are a garbage-collected program and a heap string, so the
acquisition this removes was theirs as well as ours. All reproducers answer
identically on both arms. Perry took `Run::Finished` as the answer in both its
host paths and fell into its existing advance loop on `Run::Paused`, where a
capacity request lands in its scratch-growth branch unchanged. Measured by the
`secret-tests-a0` session.

What the same session could not measure, and what this document briefly claimed
it had: what the rest of 0.1.9 is worth to Perry. That needed a third arm built
against the previous release, and the arm never existed — the build failed
because Perry's seven-day publish-age soak refuses a release that new, and the
script's `cargo … | tail` reported the exit status of `tail`, so a silent
failure relinked the previous arm's binary. Two arms, one artifact, and a
comparison that could not fail. The engine each surviving arm contains was then
settled from the linker's dependency file rather than by inference. It is
recorded rather than deleted because the failure is the instructive kind, and
the one this project's own rules warn about: an exit code is not evidence that
two arms differ, and a comparison that cannot fail proves nothing. What a
release is worth to that host stays unmeasured, which is not the same as zero.

A host can also lend its scratch instead of giving it up: `&mut O` is a
`ScratchOwner` wherever `O` is one, so a search holds a pointer to the host's
buffers rather than a copy. A host that keeps its scratch across calls — a pool,
a per-thread buffer — then builds and moves nothing per search, and the borrow
is what stops a nested search from sharing live scratch with the one it
interrupted; a compile-fail example in the trait's documentation fixes that.
Perry measured a 336-byte structure move per call into `Search` from building
its buffers each time (Perry issue #10166), which is what this removes. It makes
no measurable difference in a Rust microbenchmark, where the move is within one
frame and the compiler elides it; the cost is in a host that constructs the
owner per call.

A host that resumes after an empty match must advance the way the specification
does: one code point under the `u` flag, one unit otherwise. Advancing a single
unit inside a surrogate pair does not make progress, because a `u` search
normalizes its start back to the pair, and the same empty match is found again.

Two costs remain per search and are not changed by this. A program with a
required-text condition, on a subject of 64 units or more, runs admission from
the subject's beginning to that text's first occurrence; that is cheap where the
text occurs early and costs the distance where it first occurs late. And binding
a byte subject validates the whole string, which is the host's to do once per
operation rather than once per search.

### Deciding a search in one call

`Search::new` acquires both owners to learn the operation's shape, and
`Search::advance` acquires them again to run. A host that starts a search per
call, as a JavaScript `test` or `exec` does, usually sees it decided within its
first quantum, so it pays for both acquisitions and for moving a `Search` it
then drops — 360 bytes of state, shape and scratch handles.

`Search::run(resources, start, near, buffers, budget, quantum)` acquires them
once and runs the first quantum in the same borrow. If that decides the search,
it returns `Run::Finished`, which holds the answer, the position, the work left
and the scratch, and reads captures as a `Search` would; no `Search` is built.
Otherwise it returns `Run::Paused` with a `Search` that continues exactly as
`Search::new_near` — or `Search::new` without `near` — followed by `advance`
would have: the same pending update after a capacity request, the same answer,
captures, position and charged work. `Search::run_without_captures` does the
same for a search built with `without_captures`.

Instructions per call, from `examples/call_cost` at one and two million calls,
with per-call constant-work bindings and quantum 4096, the shape of Perry's lent
path:

| Case | `Search::new` + `advance` | `Search::run` | Reading captures, before | after |
|---|---:|---:|---:|---:|
| `/a/` against `"a"` | 1,496 | 1,284 | 1,575 | 1,333 |
| `/z/` against `"a"` | 775 | 572 | 782 | 572 |
| `/needle/`, 29 characters | 1,688 | 1,465 | 1,756 | 1,517 |
| `/[a-z]+!/`, 61 characters | 2,672 | 2,445 | 2,742 | 2,500 |
| `(\w+)@(\w+)\.com`, 25 characters | 7,700 | 7,477 | 7,804 | 7,559 |

Between 200 and 245 instructions a call, whatever the pattern: it is the call's
fixed cost, not its matching. That is a measurement of this crate's driver, and
the last change of this kind — borrowing the operation's state instead of moving
it — gained here and cost Perry 20 to 66 instructions per call once compiled
into its runtime, so the number that decides is the host's.

Perry's is larger. Measured in Perry at its merge train 214 (`c8cf45056`),
release build, three arms from one commit, nine interleaved rounds, the minimum
per cell, with a control binary's instructions subtracted so the figure is the
regex call alone:

| Perry call | `Search::new` + `advance` | `Search::run` | |
|---|---:|---:|---:|
| `.test()`, hoisted pattern | 4,382.2 | 4,063.7 | −7.3% |
| `.test()`, literal pattern | 5,698.2 | 5,379.7 | −5.6% |
| `exec`, two groups | 7,566.9 | 7,237.9 | −4.3% |

318 to 329 instructions a call, more than this crate's driver shows, because a
host acquires its views through its own owners: Perry's are a garbage-collected
program and a heap string, and the acquisition this removes was theirs as well
as ours. Wall clock on a loaded host moved 12.1, 8.2 and 5.8 per cent on the
same runs, and all four reproducers answer identically on every arm. Perry took
`Run::Finished` as the answer in both its host paths and fell into its existing
advance loop on `Run::Paused`, where a capacity request lands in its scratch-
growth branch unchanged. Measured by the `secret-tests-a0` session.

That session also ran a third arm — this crate's main with no adoption, to see
what the release alone was worth to Perry — and reported it as exactly zero.
It is withdrawn: its two arms were byte-for-byte the same binary, so the
comparison could not have shown a difference whatever the answer was. It is
recorded rather than deleted because the failure is the instructive kind, and
the one this project's own rules warn about: a comparison that cannot fail
proves nothing, and an exit code is not evidence that two arms differ. The
adoption arm stands, because its source uses `Run`, which exists only on main,
and its binary differs from the other two.



`Search` exclusively owns a `ScratchOwner`. Existing borrowed `Scratch` implements that trait, and an embedder can instead supply an owner of allocated buffers. Acquiring a scratch view must not allocate, collect, call host code or change live entries. The core allocates nothing and never grows storage implicitly.

Frame or undo exhaustion returns an explicit capacity request and retains the pending update. Further advances report the same request without spending work. `required_scratch` reports capacities sufficient to preserve live state and satisfy that request. The host may allocate replacement buffers outside the advance, then consume the search through `rebuffer`. That operation checks all capacities before writing and transfers live registers, frames and undo entries; it never copies the subject. On failure it returns both unchanged owners. On success it releases the previous scratch owner before returning and resumes the pending update without charging matching work again. Replacement allocation and old-owner cleanup may collect because no subject/program view survives either boundary.

Hosts choose growth, retention and allocation-failure policies and account for old/new buffer overlap. Cancellation stops execution; dropping the search releases its scratch owner and ends the root-owner borrow. `into_buffers` consumes any operation and returns its scratch owner without acquiring a resource view. The operation state and resource borrow end; the returned owner's own type determines its remaining lifetimes. Copy successful captures and save remaining work before consuming the operation. A new search initializes its own live scratch state, so an embedder may reuse buffers across unrelated resources or release them according to an explicit retention policy. The synchronous `find` remains a fixed-buffer call and reports insufficient capacity directly. Compilation does not yet support suspension or mid-operation growth.

## Verification

`tests/resumption.rs` compares complete captures and total work across quanta 1, 2, 3, 7, 31, 256 and uninterrupted execution. Its mock owner allocates new program/subject storage, poisons and frees the previous allocations between advances, and reacquires views under a Rust borrow guard. Cases exercise both directions, capture reset, nullable repetition, assertions, duplicate names, backreferences, UTF-8/UTF-16 and positions between surrogate halves. Long-operation cases cover admission, table scanning, initial/backreference seeks and rollback. Other witnesses cover zero quantum, unavailable resource borrows, cancellation, work/frame exhaustion and a changed resource layout. A compile-fail witness prevents a callback from leaking an input view.

The owned-scratch witness begins with no frames or undo entries and grows only after a capacity request. Allocation and destruction relocate the resources; old scratch is poisoned and released. It checks unchanged complete captures and total matching work, failure without partial destination writes, and owner counts that expose retained old allocations.

The scratch-reuse witness consumes completed, pending, cancelled, work-limited and capacity-blocked operations. It destroys and poisons the previous program/subject owner while keeping the exact scratch allocations, then matches unrelated resources with those allocations. Reclaiming scratch must acquire no additional resource view.

The development `scratch_cost` driver compares fixed buffers, fresh zero-frame/undo buffers, reuse, and reuse with a 64 KiB payload retention cap. Growth uses powers of two up to the same fixed caps (16,384 frames and 131,072 undo entries), with allocation and cleanup outside resource views. `verify` checks every iteration's complete captures and work against synchronous `find`. Timing modes report explicitly owned scratch payload, all buffer allocations/frees, replacement overlap, transferred live metadata and retained payload; these counters exclude allocator metadata, engine state on the stack, program/capture storage and process RSS. Compilation scratch, program capacity and capture capacity are reported separately. External process measurements are still required, and no convenience allocation policy is installed in the core or Perry.

`tests/run.rs` holds `Search::run` to `Search::new` or `Search::new_near`
followed by `advance`: the answer, every capture, the position and the work left,
over eighteen patterns and nine subjects from three starts, with and without a
near position, at quanta of 1, 17 and 4096, with scratch small enough that
capacity requests happen and storage moved and poisoned at every pause. Both
halves are required to occur hundreds of times, not only the decided one. The
boolean form must answer as a boolean search and refuse captures, and a run must
refuse what a new search refuses: short registers, a position from another
subject, and a zero quantum doing work. Ten injected faults are each caught: a
pending search reported as decided, `near` ignored, the boolean form not applied,
a capacity request returned as an error, registers unchecked, a foreign position
accepted, a start past the end not settled, the position reported for the wrong
layout, a paused search built without its shape, and the work left not carried
out of the views. `engine_probe --run` starts every resumable search this way,
and CI runs the engine differential through it at quantum 17 and the candidate
and classes harnesses at 1 and at 4096 with a near position; a planted fault
that swaps a decided match for no match makes the candidate harness report
20,580 differences with `--run` and none without it.

`tests/position.rs` checks a borrowed owner against an owning one: the same
answer, captures and remaining work from every start of every subject in its
corpus, with the borrow handed back through `into_buffers` for the next search.

`tests/position.rs` also covers restarting. It walks every match of every
pattern in its set over each subject twice — a new search from the previous
position per match, and one search restarted per match — and requires the same
captures, remaining work and positions from both, over 362 matches, with the
owner relocated at every pause. A second test restarts a search that is blocked
on a capacity request, one that was cancelled, and one whose start is past the
end, and requires each to behave as a new search would. Three injected faults
are caught: dropping the position the restart starts from, keeping the previous
search's finished state, and refilling the budget.

`tests/position.rs` covers starting from positions. Every pattern in its set — ASCII, two-byte, astral and lone-surrogate WTF-8 and UTF-16 subjects; the `u` flag; lookbehind, backreference, word boundary, end anchor, sticky, lone-surrogate and empty patterns — runs from every start including past the end, with a hint at every position of the subject including between surrogate halves, with storage relocated, poisoned and freed at every pause. Each run must give `find`'s answer and complete captures, charge no more work, and after a match stand at the match's end: 19,404 runs, 5,906 of which started from the hint. The loop witnesses above assert the linear and quadratic growth. Refusal is checked for a position from a subject of another layout and for one that lands inside a scalar of a subject with the same layout. A span reader from a position must read exactly the units a plain reader reads, charge no more, and charge exactly the distance plus the span when the position is two units before it. `a_sticky_loop_from_positions_does_linear_work` also asserts that a failed sticky attempt's position is its start. Six faults injected into the implementation — no layout check, a hint taken when farther, starting at the hint without walking to the start, a reader that forgets to seek, a match's position reported as its start, and a failed attempt's position reported as wherever its scanning stopped — are each caught.

The complete-answer `engine_probe` additionally accepts `--quantum N [--relocate] [--grow] [--near]`. `--near` reads a span reader to a position that varies with the start, relocating between reads, and starts the search from it; CI runs the engine differential and the candidate and class harnesses that way, and their output must be identical to the same runs without it. Locally, on Node 26.5.1, that held for all 12,593 engine cases, 137,513 candidate cases and 124,416 class cases. Growth starts with no frame/undo storage and keeps the ordinary probe's maximum capacities and work allowance. Relocation is a development collector simulation and copies its own backing allocations to test movement; production Perex never copies the subject for traversal. These modes are for differential correctness, not a performance baseline. The ordinary probe keeps its existing protocol.

Perry root tracing, exception cleanup, reentrant operations, collecting coercions/callbacks, cache ownership and allocation accounting need actual host tests. The one-time validation and resumption costs, pause latency, live/peak/retained storage and engine/application CPU/RSS remain separate performance gates. Compiler suspension is also not implemented by this matcher API.
