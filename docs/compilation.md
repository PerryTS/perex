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

`[ab]*[ac]*x` over eighty `a`s shows what that is for. The compiler cannot
merge those repeats, so generated code tries every division of the run between
them: 1,807,007 instructions to report no match, where the interpreter's
required-text check answers at once. With a budget of 1,000 the same call
returns `EXHAUSTED` after 10,359.

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
back is a third open repeat, alternation, a lookahead or any other assertion needing a sub-search,
properties, backreferences, non-ASCII characters and ranges, and any repeat that
resets a capture per iteration.

`Program::compilable` is that decision and is implemented; it is a property of
the program alone, and the tier additionally requires wholly ASCII storage, a
host-owned code buffer, a target it has an encoder for, and no more than two
open repeats at once — two is what both targets hold in registers, and a third
would have to live in memory in the generated code and in what verifies it. `examples/compilable`
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

That table takes the better of the two per case. What a host gets is whichever
the rule below picks, without running both, and the section after it says what
that costs.

**One verified target.** The generator emits both AArch64 and x86-64, and both
are held to the interpreter over the corpus by emulators that decode the bytes.
Only AArch64 is *verified*, though, so only AArch64 is returned to a host. CI
executes both through those emulators on whatever it runs on; what it cannot
yet do is execute either natively, which needs the host half as well.

## Choosing the path

The last row of the table above is the warning: over 256 KiB generated code is
nineteen times *slower* than the interpreter, because it tries every start
where the interpreter's admission scan reads raw bytes and skips almost all of
them. So a host has to choose, and it has to choose without running both.

`native::emit::preferred(program, bytes)` is that choice. It is the program's
and the subject's length, never the subject's contents and never how a search
is going: both paths answer identically, so a wrong choice costs time and not
correctness.

Two things it asks. First, whether the generated code walks the subject at all:
a program anchored at the beginning has one start, and one whose match must end
at the subject's end starts near that end, and neither cares how long the
subject is. Second, for everything else, whether the subject is short enough
that entering a search costs more than the walk.

That length was measured rather than guessed, over subjects that match nothing,
which is the case that tries every start. Ratios are generated code over the
interpreter, so below 1.00 the tier wins:

| bytes | `needle` | `NeEdLe`/i | `[0-9]+[A-Z]+` | `a+!` | `(\w+)@(\w+)\.com` |
|---:|---:|---:|---:|---:|---:|
| 8 | 0.10x | 0.08x | 0.31x | 0.32x | 0.02x |
| 16 | 0.24x | 0.16x | 0.58x | 0.56x | 0.09x |
| 24 | 0.36x | 0.25x | 0.85x | 0.83x | 0.12x |
| 32 | 0.47x | 0.31x | **1.13x** | **1.06x** | 0.12x |
| 64 | 0.95x | 0.58x | 2.87x | 1.35x | 4.90x |
| 128 | 1.51x | 0.80x | 4.54x | 1.53x | 6.62x |
| 2048 | 9.62x | 5.12x | 8.41x | 2.34x | 11.67x |
| 524288 | 15.09x | 6.55x | 9.59x | 2.51x | 5.90x |

Thirty-two bytes is the shortest crossing rounded to a power of two. At that
length two of the five shapes are already 1.06x and 1.13x, which is the price
of one number rather than five; one byte further and the worst of them is
4.54x, which is why the number is not larger.

Measuring this found something the ratios could not have hidden. A
start-anchored program's generated code tried every start and failed each at
the same `^`, where the interpreter tries one: 16,665x slower over 512 KiB.
Generated code now stops after the first start when the program is
start-anchored, by the same derivation the interpreter uses, and the same case
is 0.05x — twenty times faster than the interpreter rather than four orders of
magnitude slower.

### What the rule gives up

A length cannot see what a subject holds, and three of the measured cases are
faster in generated code for reasons only the subject shows:

| Case | Interpreter | Generated | The rule picks |
|---|---:|---:|---|
| `a+!`, `[a-z]+!`, `\w+!`, `[^0-9]+!` over sixty | 99.8–159.1 ns | 29.3–30.6 ns | interpreter |
| `(?<=0123456789)abc` over 256 KiB | 115.8 ns | 29.1 ns | interpreter |
| `[0-9]+[A-Z]+` over 256 KiB | 1.44 ms | 910 µs | interpreter |

The first two match early, so generated code never walks far; the third is a
case the interpreter is slow on. A host that knew would take between 1.6x and
5.3x more. Both are content, and the rule refuses to look at content, because
the alternative is a decision that changes with the subject — which is how a
fallback stops being an optimization and starts being a second engine.

Closing that is not a better rule; it is generated code that skips starts the
way the interpreter does. That is worth doing and is not done.

### What a host gets

The same cases as the table above, with the path the rule picks:

| Case | Interpreter | Generated | Host takes | Against the interpreter |
|---|---:|---:|---:|---:|
| `/a/` against `"a"` | 50.1 ns | 1.1 ns | generated | **0.02x** |
| Sixteen-character literal | 56.9 ns | 3.5 ns | generated | **0.06x** |
| Captures, 25 characters | 432.1 ns | 29.3 ns | generated | **0.07x** |
| Short classes | 339.4 ns | 41.4 ns | generated | **0.12x** |
| Short literal | 54.0 ns | 11.8 ns | generated | **0.22x** |
| Folded literal | 66.6 ns | 13.6 ns | generated | **0.20x** |
| End-anchored hit, 256 KiB | 56.8 ns | 2.4 ns | generated | **0.04x** |
| `/z/` against `""` | 22.1 ns | 1.3 ns | generated | **0.06x** |
| `\w+!` over sixty | 159.1 ns | 30.2 ns | interpreter | 1.00x |
| Literal lookbehind, 256 KiB | 115.8 ns | 29.1 ns | interpreter | 1.00x |
| `/needle/` over 256 KiB | 11.1 µs | 220 µs | interpreter | 1.00x |

No case is worse than the interpreter, which is the property the rule is for.
Eight of the eleven take between four and forty-five times less time.

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
5. **Done.** A rule for choosing between the two paths, measured rather than
   guessed, with what it gives up recorded beside what it takes.
6. The same encoder for x86-64, so CI can execute any of this. **The generator
   is done**: `native::x64` encodes the System V convention Linux and macOS
   use, the instruction selection is shared with AArch64 through
   `native::machine`, and the corpus — every supported pattern, every subject,
   every start — is run through an emulator for that target and compared with
   the interpreter, with the budget property checked there too. What remains is
   the verifier for it, and until that exists `emit_search` emits AArch64 only:
   this crate does not hand a host code it cannot check.
7. Generated code that skips starts as the interpreter does, which is what the
   rule's give-ups are made of.

Each stage is independently useful and independently abandonable. Stage 1 costs
nothing and tells us how much of the benchmark the tier could even apply to,
which is worth knowing before anyone writes an encoder.

## Verification

Stage 1 is covered by `tests/compilation.rs`, which fixes the subset's contract
in both directions, including that a program the analysis refuses still compiles
and is still searched, and stage 5 by the same file: the path rule takes the
tier while a subject is short, keeps it at any length for a program that does
not walk the subject, and never takes it for a program the tier cannot emit.
`examples/compilable` reports the count over the benchmark patterns, and
`bench --bin perex-native --crossover` is the sweep the length came from.

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

## What adopting this in a host needs

Stages 1 to 4 are done and the tier still runs nowhere. This is what stands
between it and Perry, in the order it would have to be built, with what each
part is worth.

### What it is worth

Per match, walking every match of `"ab12 cd345;".repeat(100000)` — 1.1 MB, ASCII,
200,000 matches — on the development machine under other load, best of five:

| Per match | `/[0-9]+/g` | `/([a-z]+)([0-9]+)/g` |
|---|---:|---:|
| A `Search` per match, as a host runs one | 139 ns | 197 ns |
| One search restarted per match | 112 ns | 169 ns |
| Synchronous `find` per match | 95 ns | 154 ns |
| Generated code per match | 7.8 ns | 6.6 ns |
| V8's whole `exec` loop, including its result arrays | 27 ns | 42 ns |

So the tier is twelve to twenty times the interpreter on this shape, and below
V8's per-match cost even counting what V8 allocates. That is the case for
building the rest. It is one shape on one machine: the ratio, not the
nanoseconds, is the result.

For scale, Perry's own per-match cost in a callback `replace` is about 3,200 ns
after Perry issue #10225, of which the engine is 140 to 200. Making the engine
free would leave that loop at roughly eighteen times Node. The tier is worth
building for what a host will be once its own costs come down, not for what it
fixes today.

### Stage 5: an x86-64 encoder

CI is x86-64, so today CI cannot execute a single generated instruction, and by
this project's rules an untested code generator is not evidence. The instruction
selection in `emit` is already separate from `a64`'s encoding; the second
backend implements the same operations, its own emulator decodes independently,
and `verify` gains an x86-64 decoder against the same proofs.

Two backends also give a test no oracle can: the same program over the same
subject must produce identical answers on both, so a mistake in one shows up
without asking V8 or the interpreter what the answer is.

### Stage 6: choosing a path

The budget already answers this. Run the generated code with a small allowance;
if it returns `EXHAUSTED`, the interpreter runs the same search from the same
start. Wasted work is bounded by the allowance times the code's length, and
correctness never depends on which path ran. `/needle/` over 256 KiB — where
generated code is nineteen times slower than the interpreter, because it tries
every start where admission skips them — becomes a bounded loss, and the
allowance is chosen by measuring exactly that case.

The rule is then: the program is in the subset, the subject is ASCII storage,
code exists for the target, and an allowance is set. Everything else is the
interpreter, as now.

### Stage 7: the host boundary

Two routes, and they are not equivalent:

- **Ahead of time.** Perry compiles TypeScript to a binary, so a regex literal's
  code can be emitted at build time, linked as ordinary read-only data, and
  checked by `verify` when it is first used — the same check `Program::from_words`
  already performs for a program. No executable memory is mapped at runtime,
  which is what makes it work on platforms that forbid it, iOS among them.
  Dynamic `new RegExp` keeps the interpreter.
- **At runtime.** Map, protect and call, as `bench/src/bin/native.rs` already
  does. It covers dynamic patterns, and it needs W^X mapping, an entitlement
  under the hardened runtime on macOS, and is unavailable on iOS.

Ahead of time first: it covers the literals that dominate real programs, and it
avoids the platform questions entirely.

### Stage 8: coverage

The subset is a sequence of atoms, greedy repeats of atoms, captures, the
anchors and a literal lookbehind, over ASCII storage. What falls back is
alternation, lazy repeats, backreferences, non-ASCII characters and classes, and
case-insensitive classes. `examples/compilable` counts what qualifies over the
benchmark patterns; the same count over Perry's own patterns decides whether
alternation — the one that needs real branching in generated code — is worth
adding before anything else.

### What has to be decided

- Ahead-of-time or runtime first; this note argues ahead of time.
- Whether the x86-64 encoder comes before or after a host integration on
  AArch64. CI argues for before.
- How much coverage is required before adoption, measured over the host's own
  patterns rather than guessed.
- Where the host-side work lives, since emitting, linking, verifying and
  selecting are all the host's, and this crate keeps `#![forbid(unsafe_code)]`.

### What stays true whatever is decided

Generated code is an optimization of one definition: the interpreter. Both paths
run every differential case, disagreement is a bug rather than a fallback, and
the subset is decided from the program before a search, never from how one is
going.
