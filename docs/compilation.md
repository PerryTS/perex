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

**This is implemented, and the bound is proved rather than tested.** The unit is
one backward branch. Every loop in the generated code closes through the same
three instructions — `cbz x4, out; sub x4, x4, #1; b loop` — so the budget is
decremented only when it is not already zero and never wraps, and between two
backward branches execution only moves forward through the code. A call given
budget `b` therefore executes at most `(b + 1) × instructions` instructions,
whatever the pattern and subject. The [verifier](#verifying-generated-code)
refuses code in which any backward branch takes another form, anything else
writes the budget, or anything branches into the middle of a guard.

`[ab]*[ac]*a*x` over thirty `a`s shows what that is for. The compiler cannot
merge those repeats, so generated code tries every division of the run among
them: 837,222 instructions to report no match, where the interpreter's
required-text check answers at once. With a budget of 1,000 the same call
returns `EXHAUSTED` after 10,773.

**This replaces the first version of this section, which required a static bound
on a program's work and refused any unbounded repeat.** Stage 1 was built that
way and measured against the benchmark before an encoder existed, which is what
stage 1 is for. It admitted four of fifteen patterns — `needle`, `a`, `aaaa`,
`z` — and those map to exactly the cases already within 2x of V8. Both cases
furthest behind were refused, because both use `+`. A tier that applies only
where the gap is smallest is not worth building, and the requirement was wrong
rather than the tier.

With the budget bounding the pause instead, the same measurement admits eleven
of fifteen, including both worst cases and all four repeat diagnostics — sixteen
of the nineteen cases behind V8. The three left out were folding, an end anchor
and a lookbehind, each excluded for scope rather than because it could not be
emitted, and each was then checked rather than assumed:

- A folded ASCII comparison is exact on the storage this tier requires, because
  the only two non-ASCII characters that fold into ASCII cannot occur in it.
  That argument is already made, and already differentially verified, by the
  start scan and by the retreat.
- `^` and `$` are position tests, and `\b` is two membership tests of a range
  set the emitter can write out.
- A lookbehind whose body is a run of ASCII characters is a comparison against
  the bytes just before the position, which the executor already makes over
  bytes rather than by reversing direction.

Admitting those takes the count to seventeen of eighteen patterns, and **all
nineteen cases behind V8**. What still falls back is alternation, which needs
branching control flow in generated code — and the one alternation case measured
is already ahead of V8 at 0.51x, so the tier is not what it needs.

The subset is therefore not a partial answer to the gap. It covers every case
that is behind.

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

One entry point, C ABI, all state in arguments. This is what
`native::emit::emit_search` generates:

```
extern "C" fn(
    subject: *const u8, length: usize, start: usize,
    registers: *mut usize, budget: usize,
) -> isize
```

It returns the position the match began at, `NO_MATCH` (`-1`), or `EXHAUSTED`
(`-2`) once it has taken `budget` backward branches without deciding. A start
past the end is no match, as it is to the interpreter. The host constructs the
function from the mapped buffer.

It writes only into the first `program.register_count()` registers, reads only
within `length`, and returns rather than calling back. The register count is not
an argument: it is fixed by the program, and the verifier checks every store
against it. A compiled search that runs out of budget leaves its registers
holding positions but no answer, and the interpreter re-runs the same search
from the start — correctness never depends on resuming compiled code, which is
what keeps the pausability argument above simple.

An earlier draft of this contract returned the work consumed. Nothing needed it:
a host charges the allowance it passed, which is an upper bound.

## Verifying generated code

Tests show the generated code is right on the inputs they try. Executing it
needs more than that, because a wrong load is not a wrong answer but a read of
someone else's memory. So `emit_search` does not return code it cannot check,
and `native::verify::verify` is public for code that arrives any other way — an
ahead-of-time image read back from a binary is checked the way
`Program::from_words` checks a relocated program.

The verifier decodes the bytes, not the emitter's record of them, follows every
path through them by abstract interpretation, and refuses the code unless it can
show, for every subject, start, budget and register contents:

- every load is inside the subject or inside the code buffer (the class tables
  live after the code);
- every store is into one of the program's registers, and stores a position no
  further than the length;
- every branch lands inside the code and execution never runs off its end;
- the code returns through the link register with a position or one of the two
  codes, names neither the stack pointer nor the zero register, and writes no
  register the calling convention preserves;
- a 32-bit comparison is only ever of a value known to fit 32 bits;
- the budget bound above.

It assumes the host's half of the call: the subject pointer addresses `length`
readable bytes and `length` is a slice length; the register pointer addresses
`register_count` writable slots; the code is mapped as exactly the bytes
verified. It needs one `Facts` of scratch per instruction, supplied by the
caller like every other buffer, and allocates nothing. Code that fails is
cleared before the error is returned: zero is permanently undefined on AArch64,
so a host that maps the buffer anyway faults rather than running unchecked code.

It does not show that the answer is right. The interpreter defines that, and the
differential tests below hold the generated code to it.

Writing the verifier changed the generator, which is the argument for having
one. Three things the emitted code did were unsafe or wrong in ways no test had
exercised:

- The per-start room check added what a match needs to the start and compared
  the sum with the length. A start near the top of the address space wrapped
  that sum, passed, and loaded from before the subject. The code now compares
  each start with the last start that has room, computed once, which rejects a
  start past the end by the same test and is one instruction shorter per start.
- `^` and the lookbehind's distance test compared only the low 32 bits of a
  position, which is wrong past four gigabytes. They are 64-bit comparisons now.
- There was no budget, so nothing bounded the time a call could take.

## What is compiled, and what proves it correct

The first version compiles the subset that covers the measured gap: entry
`SAVE`s, ASCII `CHAR` runs folded or not, `CLASS` of ASCII ranges folded or not,
`ANY`, repeats of those whether bounded or not, the anchors and word boundary,
a lookbehind whose body is a run of ASCII characters, and `MATCH`. What falls
back is alternation, a lookahead or any other assertion needing a sub-search,
properties, backreferences, non-ASCII characters and ranges, and any repeat that
resets a capture per iteration.

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

## What it buys, measured

The tier is implemented far enough to execute, and `bench`
does: it maps what the generator emits, calls it, checks every answer against
the interpreter at every start position first, and times both. Process CPU time,
best of five rounds, the same machine as every other figure here.

| Case | Interpreter | Generated | V8 `test` | V8 `exec` |
|---|---:|---:|---:|---:|
| Captures, 25 characters | 385 ns | **30.2 ns** | 60.1 | 89.3 |
| Class repeat, short | 306 ns | **37.7 ns** | 68.2 | 93.1 |
| `\w+!` over sixty | 144 ns | **21.2 ns** | 36.4 | 49.8 |
| Folded literal | 66.0 ns | **14.2 ns** | 17.3 | 35.8 |
| Literal lookbehind, 256 KiB | 115 ns | **29.8 ns** | 30.0 | 41.0 |
| End-anchored hit, 256 KiB | 53.4 ns | **2.0 ns** | 13.8 | 32.8 |
| Short literal | 51.9 ns | **12.5 ns** | 38.8 | 43.2 |
| `/a/` against `"a"` | 44.8 ns | **1.0 ns** | 18.2 | 33.7 |
| `/z/` against `""` | 15.5 ns | **1.3 ns** | 12.5 | 14.2 |
| `/needle/` over 256 KiB | **11.6 µs** | 217 µs | 69.8 | 69.9 |

Those figures predate the budget. With it, measured 2026-09-13 against the
previous revision built from a scratch worktree and run alternately on the same
machine, best of five rounds each, twice:

| Case | Before | With budget |
|---|---:|---:|
| `\w+!` over sixty | 21.7–21.8 ns | 30.7–31.3 ns |
| `a+!`, `[a-z]+!`, `[^0-9]+!` over sixty | 21.4–21.7 ns | 30.6–31.0 ns |
| Class repeat, short | 38.7–38.9 ns | 42.2–42.5 ns |
| `/a/` against `"a"` | 1.0 ns | 1.2 ns |
| Sixteen-character literal | 3.3 ns | 3.6–3.7 ns |
| `/needle/` over 256 KiB | 220 µs | 228–230 µs |

A sixty-byte scan pays about nine nanoseconds for a test and a decrement on
every byte, and the short class repeat, which also scans, 3.5. The four repeat
diagnostics stay ahead of V8, whose figures on the same shapes are 38.0 to
40.5 ns ([repetition](repetition.md)), but by less. The 256 KiB literal, already
nineteen times slower than the interpreter, is 4% slower still; everything else
moved by under half a nanosecond.

One alternative was measured and dropped: a flag-setting `subs x4, x4, #1`
followed by a conditional branch back, which executes one instruction per
iteration where the form above executes two. It was no faster — 31.3 to 31.6 ns
on the same scans across eight samples, against 30.1 to 31.4 for this form in
the same session — and its argument is weaker, since the
budget wraps and the code must return at once when it does. Charging a scan's
length once rather than per byte would remove most of the cost, but proving that
needs relations between registers the verifier does not track.

Taking whichever path wins per case, **every one of the twenty-five measured
cases is at or better than V8**, on both of its entry points. The worst ratio is
0.99x against the boolean one; the estimate this section used to hold was five to
ten times on the search portion, and the measured figure ranges from four times
on the worst case to more than forty on the shortest.

Two things that number depends on, and neither is done.

**The path has to be chosen, and nothing chooses it yet.** The last row is the
warning: over 256 KiB the generated code is nineteen times *slower* than the
interpreter, because it tries every start position where the interpreter's
admission scan decides a position on two characters at a time and skips almost
all of them. The tier wins where the subject is short and the flat cost
dominates, which is exactly where the gap was, and loses badly elsewhere. The
table above takes the better of the two per case; a host has to make that
decision without running both, and this document does not yet say how. Until it
does, the result above is what the tier *can* deliver rather than what a host
would get.

**One target.** Everything here is AArch64. CI runs x86-64, so CI cannot execute
any of it, and by this project's own rules a code generator CI cannot test is
not yet evidence.

## Staging

1. **Done.** This document, and the analysis that decides what qualifies, with
   no code generation — measured by counting which benchmark cases qualify, and
   already worth its cost: the first cut of the rule covered only the cases that
   needed it least, and the count said so before an encoder existed.
2. **Done.** An AArch64 encoder and a generator for the subset, with every
   emitted program run through an emulator and compared with the interpreter,
   and the harness itself checked by injecting faults into the generator --
   eighteen so far, eighteen caught.
3. **Done.** Measurement against V8 per case, through a host harness that maps
   and calls the code. Recorded above.
4. **Done.** A work budget in the generated code, and a verifier that proves the
   memory, control-flow, calling-convention and budget properties above before
   code is returned.
5. A rule for choosing between the two paths, which the measurement shows is
   required and which nothing implements.
6. The same encoder for x86-64, so CI can execute any of this.

Each stage is independently useful and independently abandonable. Stage 1 costs
nothing and tells us how much of the benchmark the tier could even apply to,
which is worth knowing before anyone writes an encoder.

## Verification

Stage 1 is covered by `tests/compilation.rs`, which fixes the subset's contract
in both directions, including that a program the analysis refuses still compiles
and is still searched. `examples/compilable` reports the count over the
benchmark patterns.

The generated code is covered by the unit tests in `src/native/`:

- `a64::tests` pins every encoding to a word taken from the system assembler.
- `emit::tests::generated_code_agrees_with_the_interpreter` runs every program
  in its corpus through an AArch64 emulator at every start position, and starts
  past the end, and compares the answer and every capture with the interpreter.
  The emulator decodes independently of both the encoder and the verifier.
- `emit::tests::a_budget_never_changes_an_answer` runs the same corpus with
  budgets from zero up, and requires every run either to report exhaustion or
  to agree with an unlimited one, never to exhaust where a smaller budget
  decided, and never to execute more instructions than the proved bound.
  `a_budget_stops_what_backtracking_would_not` is the pathological case above.
- `verify::tests` assembles code that breaks each property — an unbounded load,
  an off-by-one, a wrapping bound, a table read past its end, a store outside
  the registers or of a non-position, a preserved register, a narrow comparison,
  a branch out of the code, falling off the end, a bad return value, and each
  way of evading the budget — and requires the specific refusal, alongside the
  safe version of each where there is one.
- `emit::tests::code_the_verifier_accepts_keeps_its_promises` checks the
  verifier against execution rather than against its own reasoning. It flips one
  or two random bits in every generated program, a hundred times each, and runs
  every mutant the verifier accepts over several subjects, starts including
  `u64::MAX`, and budgets. Any load or store outside its region, preserved
  register written, bound exceeded, return value that is not a position or a
  code, or register left holding something past the length fails the test. On
  the current corpus 1,007 mutants are accepted and 5,693 refused, and 333 of
  those accepted compute something different from the original, so the test is
  not only running harmless changes.

Eighteen faults were injected into the generator's source, one at a time, and
the native tests run against each. All eighteen are caught. Where the verifier
refuses the code it names why; the rest are memory-safe, which is why it accepts
them, and wrong, which is why the differential tests do not:

| Fault | Verifier | Caught by |
|---|---|---|
| No check that the subject has room for a match | refuses: store | verifier and all four emulator tests |
| No per-start check against the last start | refuses: store | same |
| No bound on a repeat's scan | refuses: load | same |
| A scan that ignores what must follow it | refuses: load | same |
| A lookbehind without its distance check | refuses: load | verifier, agreement, budget, mutation |
| A lookbehind that reads one byte late | accepts | agreement |
| A class table indexed into the subject | refuses: load | verifier and all four emulator tests |
| A capture stored one register late | refuses: store | verifier, agreement, budget, mutation |
| A loop with no budget test | refuses: budget | verifier and all four emulator tests |
| A loop with no budget decrement | refuses: budget | same |
| A retreat below the repeat's minimum | refuses: load | same |
| The end-bound skip without its length check | accepts | agreement |
| A match that returns its end, not its start | accepts | agreement |
| A match that returns the subject's address | refuses: return | verifier and all four emulator tests |
| Scratch in a register the convention preserves | refuses: written | verifier, agreement, budget, mutation |
| `^` comparing only the low 32 bits | refuses: compare | verifier, agreement, budget, mutation |
| The budget refilled at every start | refuses: budget | verifier and all four emulator tests |
| A retreat that never gives a character back | accepts | the emulator's instruction bound |

The end-bound fault was missed at first: no pattern in the corpus had a bounded
end-anchored match shorter than its bound, which only an unrepeated class or `.`
before `$` produces. Three such patterns were added, and it is caught. A
nineteenth injection, removing the check that a start is not past the end, was
not a fault: the per-start check already rejects such a start, and the check
was removed from the generator.

`bench`'s `perex-native` maps and calls the code, checks each start against the
interpreter with an unlimited budget and with budgets of 0, 1 and 16, and times
the host's actual path: the generated code within an allowance, and the
interpreter when that runs out.

The cross-backend comparison planned for stage 6 needs no oracle, because the
two backends must produce identical answers.
