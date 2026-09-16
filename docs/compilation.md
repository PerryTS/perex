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

## Two targets

x86-64 has fifteen usable registers where AArch64 has thirty-one, and six of
them are the caller's to get back. That shapes the split between what the
generator selects and what a target encodes:

- The generator names thirteen **slots** and states operations as results — a
  byte inside a range, a byte at a distance before a position, the last start
  with room for a match, a bound taken as a minimum, a backward branch that
  spends budget. Each backend reaches those its own way, so neither spends a
  register emulating the other's instruction set.
- Thirteen slots is what x86-64 holds in registers after keeping two back, which
  is why open repeats are capped at two. A third would have to live in memory,
  in the generated code and in everything that verifies it.
- x86-64 saves `RBX`, `RBP` and `R12`-`R15` on entry and restores them before
  each return. The verifier follows that rather than assuming it: what was
  pushed is what may be popped, the stack is where it started at every return,
  and every preserved register holds what it held. A branch that jumps over the
  restores is refused.
- Instructions vary in length there, so the analysis works in byte offsets and
  needs one `Facts` per byte rather than per instruction. A branch into the
  middle of an instruction is followed, and what the processor would decode from
  there is what is checked.

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

**Both targets, one analysis.** The generator emits AArch64 and x86-64, an
emulator for each runs the corpus against the interpreter, and the same
verifier proves both before either is returned. CI executes both through those
emulators on whatever it runs on, and now executes one of them *natively* as
well: `bench --bin perex-native --check` maps what the generator emits, calls
it, and compares every answer with the interpreter at every start, on whichever
architecture the runner is. That driver is the host half — mapping, protecting
and calling — which a runtime would own.

## Skipping starts

Generated code used to look at every start position in turn, which is what the
interpreter's admission scan exists not to do: it searches raw bytes for what a
match must contain and skips almost all of them. That difference was the whole
of why a host ever chose the interpreter.

Where a match must begin with a known byte — a literal first character, or a
repeat of one that must take at least one — generated code now scans for it
eight bytes at a time. One 64-bit load, an exclusive or against that byte
repeated, and the subtract-and-mask that finds a zero byte; the first match's
offset is the count of trailing zero bits over eight. What is left at the end of
the subject, and a subject shorter than eight bytes, is a byte-at-a-time loop
after it. The budget is spent once per eight bytes rather than once per start,
so the pause bound improves with it.

The verifier needed one new thing for this: an eight-byte load is inside the
subject only where the position it reads from has eight bytes of room, which the
comparison against `length - 8` establishes. The arithmetic itself it follows
only as far as which register each instruction writes — the result is data, not
a position or an address, and nothing is refined from its flags.

Measured over subjects that hold no match, which is the case that looks at every
start:

| bytes | `needle` before | `needle` after | `a+!` before | `a+!` after |
|---:|---:|---:|---:|---:|
| 128 | 1.51x | **0.23x** | 1.53x | **0.16x** |
| 512 | 5.48x | **0.85x** | 2.19x | **0.21x** |
| 2048 | 9.62x | **1.39x** | 2.34x | **0.21x** |
| 524288 | 15.09x | **1.88x** | 2.51x | **0.21x** |

Those figures are generated code run to a decision, without the allowance the
rule below adds. `a+!` no longer crosses at all: the interpreter walks the run
its repeat leaves behind, where the scan steps over it. `needle` crosses around
a thousand bytes instead of sixty-four, and its worst case is 1.9x rather than
15x — which the allowance then flattens to 1.05x. A folded literal has no single
byte to scan for, and a class does not either, so those are unchanged, and the
rule below treats them apart from these.

## Choosing the path

The last row of the table above is the warning: over 256 KiB generated code is
nineteen times *slower* than the interpreter, because it tries every start
where the interpreter's admission scan reads raw bytes and skips almost all of
them. So a host has to choose, and it has to choose without running both.

`native::emit::preferred(program, bytes)` is that choice. It is the program's
and the subject's length, never the subject's contents and never how a search
is going: both paths answer identically, so a wrong choice costs time and not
correctness.

Two things it asks. First, whether the generated code walks the subject at all,
or walks it eight bytes to a branch: a program anchored at the beginning has one
start, one whose match must end at the subject's end starts near that end, and
one that scans covers eight bytes per branch — none of the three cares how long
the subject is. Second, for everything else, whether the subject is short enough
that entering a search costs more than the walk.

A program whose generated code scans keeps the tier at **any** length, because
what a wrong choice costs it is bounded: `native::emit::allowance` gives such a
call about one backward branch per eight hundred bytes, which lets its scan look
at roughly one byte in a hundred before it gives the search back and the
interpreter answers instead. A long subject is then a bet rather than a walk.
`(?<=0123456789)abc` over 256 KiB, whose match is at byte 26, takes 7.5 ns that
way against the interpreter's 115.2 and V8's 28.5; `/needle/` over 256 KiB
holding none of it, the case that bet loses, takes 11.4 µs against the
interpreter's 10.9 — under five per cent, for the other case's fifteen times.

That bet only works because a branch covers eight bytes. Without a byte to scan
for, a branch covers one start, and the bet costs what it stands to win, so
those programs keep a length instead. Thirty-two bytes is where it sits, and it
was measured rather than guessed, over subjects that match nothing — the case
that looks at every start. Ratios are the tier's cost, with the allowance above,
over the interpreter's, so below 1.00 the tier wins:

| bytes | `[0-9]+[A-Z]+` | `NeEdLe`/i | `(\w+)@(\w+)\.com` | `needle` | `a+!` |
|---:|---:|---:|---:|---:|---:|
| | *no scan* | *no scan* | *no scan* | *scans* | *scans* |
| 24 | 0.91x | 0.26x | 0.13x | 0.12x | 0.17x |
| 32 | **1.18x** | 0.33x | 0.13x | 0.13x | 0.19x |
| 48 | 1.87x | 0.44x | **1.04x** | 0.16x | 0.23x |
| 64 | 2.89x | 0.59x | 2.17x | 0.17x | 0.16x |
| 96 | 3.71x | **1.58x** | 1.90x | 0.22x | 0.18x |
| 128 | 3.37x | 1.50x | 1.70x | 0.24x | 0.16x |
| 512 | 2.09x | 1.48x | 1.25x | 0.86x | 0.21x |
| 2048 | 1.28x | 1.20x | 1.06x | **1.37x** | 1.07x |
| 524288 | 1.01x | 1.01x | 1.00x | 1.05x | 1.00x |

Thirty-two is the shortest crossing among the three that do not scan, and the
most a program without a byte to scan for keeps. Their worst point is where the
walk is long enough to hurt and short enough that the allowance still covers it:
3.71x at ninety-six bytes.

The two that scan cross nowhere that costs much. `needle` over 2 KiB holding
none of it is the worst of them at 1.37x, where generated code's scan is simply
slower than the interpreter's admission scan; past that the allowance takes over
and the tier converges on the interpreter's own time — 1.05x over 512 KiB, where
before the allowance it was 1.88x. That is what the bet buys: the losing side
flattens while the winning side keeps its fifteen times.

Measuring this found something the ratios could not have hidden. A
start-anchored program's generated code tried every start and failed each at
the same `^`, where the interpreter tries one: 16,665x slower over 512 KiB.
Generated code now stops after the first start when the program is
start-anchored, by the same derivation the interpreter uses, and the same case
is 0.05x — twenty times faster than the interpreter rather than four orders of
magnitude slower.

### What the rule gives up

A length cannot see what a subject holds, and three cases are faster in
generated code for a reason only the subject shows — their match is at the very
start:

| Case | Interpreter | Generated | The rule picks |
|---|---:|---:|---|
| `[a-z]+!` over sixty | 103.0 ns | 29.4 ns | interpreter |
| `[^0-9]+!` over sixty | 125.0 ns | 29.3 ns | interpreter |
| `\w+!` over sixty | 154.6 ns | 29.4 ns | interpreter |

Each begins with a class, so its generated code has no byte to scan for, and
the allowance bet that covers the scanning programs would cost as much as it
wins for these. What the rule refuses to do is look at the subject, because a
decision that changes with the subject is how a fallback stops being an
optimization and becomes a second engine.

Scanning for a *set* of bytes rather than one does not close them either, and
the classes say why. `\w` admits every byte from 48 to 122 with four holes in
it, so a scan for that set would stop at nearly every byte of an alphabetic
subject; `[^0-9]` is a complement and bounds no first byte at all. Only
`[a-z]+!` has a set narrow enough for such a scan to skip anything, and its
subject is all lowercase, so the scan would stop at the first byte there too.
What is left in these three is not code generation but the interpreter's cost
on `<class>+<literal>`: over the same sixty-one characters it takes 103 to 155
ns where V8 takes 32 to 34 and generated code takes 29.

### What a host gets, against V8

Every case both drivers run: twenty, timed on the same machine adjacent in
time, three passes each with the minimum taken. `Host` is the path the rule
picks, called with the allowance the crate recommends — not the better of the
two, which would describe a host that cannot exist. V8's two entry points
bracket what `find` does: `test` produces no captures, `exec` produces them and
allocates substrings besides.

| Case | Host | Interpreter | V8 `test` | V8 `exec` | vs `test` | vs `exec` |
|---|---:|---:|---:|---:|---:|---:|
| `/a/` against `"a"` | 1.7 ns | 49.0 | 17.4 | 31.6 | **0.10x** | **0.05x** |
| One-character literal | 1.7 ns | 49.0 | 17.3 | 31.5 | **0.10x** | **0.05x** |
| `/z/` against `""` | 1.2 ns | 19.7 | 12.0 | 13.8 | **0.10x** | **0.09x** |
| Two-character literal | 1.9 ns | 49.9 | 18.4 | 31.9 | **0.10x** | **0.06x** |
| Short literal | 3.7 ns | 54.2 | 32.1 | 39.8 | **0.12x** | **0.09x** |
| `/z/` against `"a"` | 2.5 ns | 23.7 | 14.6 | 16.3 | **0.17x** | **0.15x** |
| `/needle/` over 256 KiB | 11.4 µs | 10.9 µs | 66.6 µs | 66.4 µs | **0.17x** | **0.17x** |
| Four-character literal | 2.2 ns | 50.8 | 12.2 | 24.7 | **0.18x** | **0.09x** |
| Eight-character literal | 3.0 ns | 52.6 | 13.0 | 25.4 | **0.23x** | **0.12x** |
| End-anchored hit, 256 KiB | 3.1 ns | 56.1 | 13.3 | 30.1 | **0.23x** | **0.10x** |
| `/z/` against ten bytes | 3.7 ns | 24.1 | 14.7 | 16.3 | **0.25x** | **0.23x** |
| Literal lookbehind, 256 KiB | 7.5 ns | 115.2 | 28.5 | 38.0 | **0.26x** | **0.20x** |
| Sixteen-character literal | 4.3 ns | 55.5 | 13.9 | 26.6 | **0.31x** | **0.16x** |
| Captures, 25 characters | 29.1 ns | 416.8 | 57.6 | 82.0 | **0.51x** | **0.35x** |
| Short classes | 40.0 ns | 328.7 | 64.3 | 88.0 | **0.62x** | **0.45x** |
| Folded literal | 13.3 ns | 65.2 | 16.9 | 32.7 | **0.79x** | **0.41x** |
| `a+!` over sixty | 29.9 ns | 95.9 | 31.6 | 43.4 | **0.95x** | **0.69x** |
| `[a-z]+!` over sixty | 103.0 ns | 103.0 | 31.9 | 43.9 | 3.23x | 2.35x |
| `[^0-9]+!` over sixty | 125.0 ns | 125.0 | 32.3 | 44.8 | 3.87x | 2.79x |
| `\w+!` over sixty | 154.6 ns | 154.6 | 34.3 | 45.9 | 4.51x | 3.37x |

Seventeen of the twenty are at or better than V8 at both of its entry points.
The three behind are the shape the section above names, and what is left in
them is interpreter cost rather than anything the tier decides. The
twenty-first case the native driver runs, `[0-9]+[A-Z]+` over 256 KiB, has no
counterpart in the V8 driver, which runs `[A-Z]{4}[0-9]{4}` there instead; the
rule hands it to the interpreter, at 1.40 ms against generated code's 895 µs.

Measured with `bench/target/release/perex-native` and `bench/node-bench.mjs`
against Node 26.5.1, process CPU time, on a machine running other builds, which
inflates absolute numbers more than ratios.

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
6. **Done.** The same encoder, generator and verifier for x86-64.
   `native::x64` encodes the System V convention Linux and macOS use, the
   instruction selection is shared with AArch64 through `native::machine`, and
   one analysis verifies both. `emit_search` takes the target and returns
   verified code for either.
7. **Done for a known first byte.** Generated code scans eight bytes at a time
   for the byte a match must begin with, which is what the interpreter's own
   advantage over it was made of.
8. **Done.** An allowance instead of a second length: a scanning program keeps
   the tier at any length and is called with a budget proportional to the
   subject, so a subject that turns out to hold no match costs that allowance
   rather than the walk. The same scan for a *set* of first bytes was the plan
   here and the measurement refused it — see *What the rule gives up*: two of
   the three cases it was meant to close have no first-byte set narrow enough
   to skip anything, and the third's subject defeats it. What is left in them
   is interpreter cost on `<class>+<literal>`, which is not this document's
   problem to solve.

Each stage is independently useful and independently abandonable. Stage 1 costs
nothing and tells us how much of the benchmark the tier could even apply to,
which is worth knowing before anyone writes an encoder.

## Verification

Stage 1 is covered by `tests/compilation.rs`, which fixes the subset's contract
in both directions, including that a program the analysis refuses still compiles
and is still searched, and stages 5 and 8 by the same file: the path rule takes
the tier while a subject is short, keeps it at any length for a program that
does not walk the subject or that scans for a byte, never takes it for a program
the tier cannot emit, and hands a long subject an allowance proportional to it
rather than an unbounded one. `examples/compilable` reports the count over the
benchmark patterns, and `bench --bin perex-native --crossover` is the sweep the
length and the allowance came from.

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
  safe version of each where there is one. `verify::tests::x86_64` does the
  same for that target, including a return that gives back fewer registers than
  it saved and a branch that jumps over the restores.
- `emit::tests::x86_64_code_agrees_with_the_interpreter` and
  `x86_64_code_is_verifiable` run the same corpus through the x86-64 generator:
  every pattern, subject and start against the interpreter's answer and
  captures, through an emulator that decodes the bytes, refuses anything outside
  the subset, and faults on a load outside the subject, a store outside the
  caller's registers, a preserved register written or a stack left unbalanced.
  `x86_64_code_the_verifier_accepts_keeps_its_promises` mutates that code the
  way its AArch64 twin does, and it earned its place: the first version of the
  save-and-restore check was positional, and this found a mutant that jumped
  over the restores to a return that still had pops in front of it.
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

Every stage above is done and the tier still runs nowhere but its own
benchmark. This is what stands between it and Perry, in the order it would have
to be built, with what each part is worth.

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

### Done: an x86-64 encoder, and choosing a path

Both were on this list and both are above. CI is x86-64, so without the second
backend CI could not execute a single generated instruction, and by this
project's rules an untested code generator is not evidence; `native::x64` and
its emulator closed that, and the two backends give a test no oracle can — the
same program over the same subject must produce identical answers on both.

Choosing a path was expected to be the budget's job, and it is:
`native::emit::allowance` runs the generated code with a bounded allowance and
the interpreter answers when that runs out, so a wrong choice costs the
allowance and never the answer. What the note above did not anticipate is that
a length alone is not enough on either side of it — see *Choosing the path*.

### The host boundary

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

### Coverage

The subset is a sequence of atoms, greedy repeats of atoms, captures, the
anchors and a literal lookbehind, over ASCII storage. What falls back is
alternation, lazy repeats, backreferences, non-ASCII characters and classes, and
case-insensitive classes. `examples/compilable` counts what qualifies over the
benchmark patterns; the same count over Perry's own patterns decides whether
alternation — the one that needs real branching in generated code — is worth
adding before anything else.

### What has to be decided

- Ahead-of-time or runtime first; this note argues ahead of time.
- How much coverage is required before adoption, measured over the host's own
  patterns rather than guessed.
- Where the host-side work lives, since mapping, linking and calling are all the
  host's, and this crate keeps `#![forbid(unsafe_code)]`.

### What stays true whatever is decided

Generated code is an optimization of one definition: the interpreter. Both paths
run every differential case, disagreement is a bug rather than a fallback, and
the subset is decided from the program before a search, never from how one is
going.
