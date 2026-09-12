# A compilation tier

Roadmap item 7 allows native compilation "only with explicit memory and
compatibility evidence". This is that evidence, written before any code
generator, because two of its conclusions decide the shape of the thing and one
of them decides whether this crate gives up a guarantee at all.

Stage 1 is implemented: the analysis that decides which programs the tier could
emit code for, and the count of how much of the benchmark that covers. No code
is generated. Everything past that is a proposal the measurements below justify,
not a description of the engine, and it is marked as such.

## What the tier is for

Nineteen of the twenty-five measured cases are behind V8, by 1.09x to 4.31x
against its capturing entry point. [performance](performance.md) records what
they are made of: a one-character hit costs 44.8 ns of which 15.5 ns is entering
and leaving a search, and the remaining 29 ns is about ten primitive operations
at 2 to 3 ns each. V8 spends 33.7 ns on the whole thing because it executes
compiled code.

That is the gap, and it is not reachable by tuning. Six changes closed the parts
that were reachable; five more were measured and reverted, three of them
net-negative because a few nanoseconds of removed work did not survive the
perturbation of removing it. The remainder is the price of interpreting a
bytecode, and only not interpreting removes it.

## Where the unsafe lives, and why it is not here

Executing generated code requires memory that is written and then executed, and
a call through an address. Both are unsafe operations.

Neither has to happen in this crate. Perex is `no_std` with no allocator and no
dependencies: it has no `mmap` and cannot obtain executable memory even if it
wanted to. The host already owns every other buffer the engine uses — program,
subject, scratch — and it is a runtime with a moving collector, so mapping and
protecting memory is work it already does.

So the split is:

| Step | Who | Safe? |
|---|---|---|
| Emit machine code into a caller-owned `&mut [u8]` | Perex | yes |
| Map that buffer executable, W^X | Host | no |
| Build a function pointer for its entry | Host | no |
| Call that function pointer | Perex | yes |

Calling an `extern "C" fn` value is safe Rust. The obligation — that the pointer
addresses valid code with that signature — is discharged where the memory is
mapped, which is the host. This is the same boundary `dlsym` has, and the same
one this crate already draws for subject storage and scratch.

**`#![forbid(unsafe_code)]` therefore stays.** The authorisation to remove it is
not needed for this design and should not be spent on it. If a later stage finds
a reason the split cannot hold, that reason belongs in this document first.

This also matches roadmap item 4: host-specific execution belongs in Perry, not
here.

## Pausability, which decides what may be compiled

This engine exists because Perry's collector must be able to run at any
instruction, and an interpreter that checks a budget between instructions can
stop anywhere. Compiled code that runs to completion cannot.

The resolution is that pausability is a requirement of *long* searches, not of
all of them. A search that runs to completion quickly can run uninterrupted,
because how long it runs is also how long the collector waits. Three hundred
nanoseconds is not a pause worth engineering around; three hundred milliseconds
is, and that is the case the interpreter keeps.

What bounds how long generated code runs is the budget it is given, not the
pattern. Compiled code is called with a small allowance; if it exhausts it, it
returns without an answer and the interpreter re-runs that search from the
start. The collector therefore waits at most that allowance whatever the pattern
does, correctness never depends on resuming compiled code, and no static
analysis has to be trusted for the pause bound.

**This replaces the first version of this section, which required a static bound
on a program's work and refused any unbounded repeat.** Stage 1 was built that
way and measured against the benchmark before an encoder existed, which is what
stage 1 is for. It admitted four of fifteen patterns — `needle`, `a`, `aaaa`,
`z` — and those map to exactly the cases already within 2x of V8. Both cases
furthest behind were refused, because both use `+`. A tier that applies only
where the gap is smallest is not worth building, and the requirement was wrong
rather than the tier.

With the budget bounding the pause instead, the same measurement admits eleven
of fifteen, including both worst cases and all four repeat diagnostics. Against
the twenty-five benchmark cases that is sixteen of the nineteen behind V8;
folding, alternation, a lookbehind and an end anchor are what remain, and each
is a separate decision to admit.

## Memory contract

The code buffer is host-owned, like everything else:

- **Sizing.** `compile_native` reports the exact byte length it needs for a
  given program before emitting, as `Prepared::required_words` already does for
  programs. The host allocates, then calls the emitter.
- **Lifetime.** The buffer is valid for as long as the host keeps the mapping.
  It contains no pointer into subject storage, no pointer into scratch, and no
  host object reference; every address it needs arrives as an argument.
- **Relocation.** Generated code is position-independent or it is re-emitted.
  The first version re-emits, because a regex program is small and re-emitting
  is simpler to prove correct than a relocation table.
- **Accounting.** Code size is reported separately from program size, so the
  per-case RSS requirement in [performance](performance.md) can be met per case
  rather than in aggregate. A tier that wins CPU and loses RSS has not met it.
- **Versioning.** Generated code is tied to the program version and the target,
  and a mismatch is rejected rather than executed. Static or ahead-of-time code
  storage needs its own lifetime and version contract, which the
  [memory contract](memory-contract.md) already anticipates and this document
  does not yet supply.

## The call contract

One entry point, C ABI, all state in arguments:

```
extern "C" fn(
    subject: *const u8, length: usize, start: usize,
    registers: *mut usize, register_count: usize,
    budget: usize,
) -> isize
```

returning the work consumed, or a negative code for no-match and for each
explicit failure. The host constructs this from the mapped buffer; Perex
receives it as a value and calls it.

It writes only into the registers it was given, reads only within `length`, and
returns rather than calling back. A compiled search that runs out of budget
returns the failure code and the interpreter re-runs the same search from the
start — correctness never depends on resuming compiled code, which is what keeps
the pausability argument above simple.

## What is compiled, and what proves it correct

The first version compiles the smallest subset that covers the measured gap:
entry `SAVE`s, ASCII `CHAR` runs, `CLASS` of ASCII ranges, `ANY`, repeats of
those whether bounded or not, and `MATCH`. Everything else — folding,
properties, backreferences, assertions, alternation, and any repeat that resets
a capture per iteration — falls back.

`Program::compilable` is that decision and is implemented; it is a property of
the program alone, and the tier additionally requires wholly ASCII storage, a
host-owned code buffer and a target it has an encoder for. `examples/compilable`
reports it over the benchmark patterns, which is how the subset above was chosen
rather than guessed.

The fallback is not a second engine. It is the same program, run by the
interpreter that is already the only definition of what a pattern means. The
compiled path is an optimization of that definition, and the way that is kept
true is:

- **Both paths run every differential case.** The suite is already 1,117,377
  compared answers per revision. The compiled path runs all of them and must
  agree with the interpreter and with Node, not merely with Node.
- **Disagreement is a failure, never a fallback.** If compiled and interpreted
  answers differ, that is a bug to fix, not a case to route around. A production
  fallback chosen by disagreement is exactly the thing
  [performance](performance.md) forbids.
- **The compiled subset is decided statically,** before the search, from the
  program alone — never from how a search is going.

## Targets and CI

The development machine is `aarch64-apple-darwin`; CI runs `ubuntu-latest`,
which is `x86_64`. A tier that only emits AArch64 cannot be tested by CI, and a
tier CI cannot test is one this project's own rules will not accept as evidence.

Both backends are therefore in scope before the tier is enabled anywhere, and
the emitter is structured so the instruction selection is shared and only the
encoder differs. Until both exist and agree, the tier is compiled out entirely
rather than shipped disabled: an untested code generator behind a flag is a
liability with no benefit.

## What it is expected to buy

From the measurements in [performance](performance.md), taking the entry cost as
unchanged — it is `find` setting up, not interpreting — and the search itself as
the part compilation addresses:

| Case | Now | Entry | Search | V8 `exec` | Search at 5x | At 10x |
|---|---:|---:|---:|---:|---:|---:|
| `/a/` against `"a"` | 44.8 ns | 15.5 | 29 | 33.7 | 21 ns | 18 ns |
| Class repeat, short | 306 ns | 15.5 | 290 | 93.1 | 74 ns | 45 ns |
| Captures, 25 chars | 385 ns | 15.5 | 370 | 89.3 | 89 ns | 52 ns |

Five times is the conservative end for replacing a dispatch loop with straight
code; ten is the optimistic end. At either, the three cases furthest behind
reach parity or better against the entry point that produces captures, which is
the comparison that matches what `find` produces.

This is an estimate from a decomposition, not a measurement, and it is the thing
the tier has to prove. If the first backend does not reach the conservative
column on the cases above, the tier is not worth its safety surface and this
document should record that instead.

## Staging

1. **Done.** This document, and the analysis that decides what qualifies, with
   no code generation — measured by counting which benchmark cases qualify, and
   already worth its cost: the first cut of the rule covered only the cases that
   needed it least, and the count said so before an encoder existed.
2. An AArch64 encoder for the subset, behind a target gate, with both paths run
   over the whole differential suite.
3. The same for x86-64, with the two backends differentially tested against each
   other as well as against the interpreter.
4. Measurement against V8 per case, and a decision recorded here either way.

Each stage is independently useful and independently abandonable. Stage 1 costs
nothing and tells us how much of the benchmark the tier could even apply to,
which is worth knowing before anyone writes an encoder.

## Verification

Stage 1 is covered by `tests/compilation.rs`, which fixes the subset's contract
in both directions, including that a program the analysis refuses still compiles
and is still searched. `examples/compilable` reports the count over the
benchmark patterns. No code is generated yet, so there is nothing else to
verify. Stages 2 and 3 are covered
by the existing differential harnesses run against both paths, and by a
cross-backend comparison that needs no oracle because the two backends must
produce identical answers.
