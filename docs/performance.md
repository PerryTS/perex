# Performance and adoption requirements

The target is one production regex engine that is correct for the host's entire regex workload and beats the compatible alternatives, or reaches CPU and RSS parity within the owner's accepted 10% variation. The tolerance applies to both measures across the covered cases; an aggregate improvement alone does not satisfy that target. Input-layer tests, a faster subset, an upstream engine's results, or a passing CI run are not adoption evidence.

## Comparisons that must remain visible

- Construction/validation, first execution and warmed execution, including short inputs where setup dominates.
- Positive and negative searches, sticky starts, complete captures and boolean tests that still require internal captures.
- Long misses, alternatives, assertions in both directions, backreferences, quantified capture reset and Unicode properties/sets.
- ASCII, multibyte text, astral characters, lone surrogates and positions between surrogate halves.
- Program storage, shared table footprint, compile scratch, live/peak/retained match scratch and host caches, as well as whole-process peak and settled RSS.
- Cancellation, resource exhaustion and allocation failure as explicit outcomes. A fast false no-match is a correctness failure.
- Moving collection, nested operations and result materialization in the actual host. The engine must traverse original subject storage without a converted copy.

Reference engines are development-only competitors. Record their exact revisions, flags, build configuration and complete answers. A competitor that rejects a pattern cannot provide a successful-match performance baseline for it; keep the rejection visible and use the engines that support the case. Do not install a production fallback to make coverage pass.

The host owns private workload capture, source/binary receipts and application measurements. Core performance cases should also be reproducible independently with authored or correctly attributed inputs. Maintain a per-case inventory: merging results into one median must not conceal a slower or higher-memory case. New real workload cases expand the inventory.

## Evidence before adoption

Run clean alternating comparisons on the same machine, preserve all clean outcomes and every failure, and account for measurement noise with repeated samples. Keep cold/warm and construction/execution results distinct. For each covered case, identify the best compatible measured competitor for CPU and for memory, even when these are different competitors. Losses outside the owner's 10% tolerance remain work to do; uncertain observations require stronger evidence. The old host campaign's aggregate thresholds or memory allowance do not override this project's objective.

Then repeat complete host-operation checks and whole-application CPU/RSS comparisons against the retained binary. Traceability must connect the measured binary to the exact Perex and host revisions. Engine microbenchmarks cannot prove a host improvement, and host totals cannot prove every engine case is faster.

No finite benchmark suite proves fastest behavior for all possible patterns, inputs and future competitors. The claim must state the actual tested workload, competitors and uncertainty; missing evidence remains incomplete work. The goal is not achieved until the real host cases, correctness, ownership and per-case performance requirements have verified evidence.

## Current measurement tool

`examples/input_cost.rs` decomposes the implemented input layer only. It creates one roughly 256 KiB subject and repeatedly validates or traverses that same allocation; seeking targets the middle. Cases cover ASCII, multibyte BMP, astral characters, separately encoded surrogate pairs, lone surrogates, original UTF-16 ASCII and original UTF-16 astral storage. Each constructs its original representation directly; the UTF-16 cases do not convert a byte subject for traversal.

Modes are `validate`, `forward`, `backward` and `seek`. Output includes a checksum, subject byte/unit lengths, iteration count, input/cursor state sizes and `loop_ns`. Subject construction and the initial input borrow precede that internal timer; process CPU and peak RSS measured externally also include setup. Keep both costs explicit and use the identical driver on both engines or revisions being compared. This is an input-cost diagnostic, not a whole-engine comparison or an adoption result:

```sh
cargo build --locked --release --example input_cost
/usr/bin/time -v target/release/examples/input_cost astral forward 2048
```

## Measured against V8, 2026-09-12

V8 is the engine Perry replaces, so it is the comparison that decides adoption.
Earlier comparisons in this project used `regress` and Rust `regex`; the latter
is a linear-time automaton with SIMD prefilters and answers a different
question. Both drivers measure process CPU time, because this host is shared and
its load average ranged from 19 to 108 during these runs, where wall clock
measures how often the scheduler ran a process rather than how much work it did.
The figure for each case is the minimum of three passes, with the two engines
alternating so the same load applies to both.

V8 is timed at two entry points, because neither is a like-for-like comparison
on its own. `find` always produces captures. `test` does not produce them, so it
is timed against a Perex doing work it skips; `exec` does produce them, and also
allocates a result array of substrings that `find` never builds. The work `find`
does sits between the two, so every case below is reported as a bracket rather
than a single ratio. This was measured only after several revisions had been
compared against `test` alone, which overstated the gap on every capturing case:
the worst reads 6.41x against `test` and 4.31x against `exec`.

The harness, cases and raw results are the host's, under
`bench/`.

### Ahead

| Case | Perex | V8 |
|---|---:|---:|
| Required text before its prefix | 1.80 µs | 23.1 ms |
| Required text absent, 4 KiB of `a` | 1.60 µs | 23.1 ms |
| Class miss over 256 KiB | 18.5 µs | 886 µs |
| Long literal miss, 256 KiB | 11.1 µs | 123 µs |
| Long literal, 256 KiB | 11.2 µs | 67.7 µs |
| Three-way alternation, 256 KiB | 21.2 µs | 40.9 µs |

The first two are the catastrophic-backtracking shapes. Bounded work and the
admission bound are not a curiosity against a backtracking competitor: they are
the largest single advantage Perex has.

The last four are the scan-bound cases, and they are here because of one change
rather than a class of them: every start scan used to decide a position on its
first byte alone and compare the prefix at each position that admitted, and
[leading](leading.md) records why deciding on two bytes was available for free.
The alternation moved from 4.5x behind to 0.52x ahead, and the long literal from
0.83x to 0.16x.

### Behind

Nineteen of twenty-five cases, on both entry points: between 1.24x and 6.41x
against the boolean test, and between 1.09x and 4.31x against the capturing one.

| Case | Perex | V8 `test` | V8 `exec` | vs test | vs exec |
|---|---:|---:|---:|---:|---:|
| Captures, 25-character subject | 385 ns | 60.1 ns | 89.3 ns | 6.41x | 4.31x |
| Class repeat, short subject | 306 ns | 68.2 ns | 93.1 ns | 4.48x | 3.28x |
| `\w+` over a 60-character run | 144 ns | 36.4 ns | 49.8 ns | 3.96x | 2.90x |
| End-anchored hit, 256 KiB | 53.4 ns | 13.8 ns | 32.8 ns | 3.87x | 1.63x |
| Literal lookbehind, 256 KiB | 115 ns | 30.0 ns | 41.0 ns | 3.82x | 2.79x |
| Folded literal, 29 characters | 66.0 ns | 17.3 ns | 35.8 ns | 3.82x | 1.84x |
| Short literal, 29 characters | 51.9 ns | 38.8 ns | 43.2 ns | 1.34x | 1.20x |

Most of them are searches short enough that starting one dominates. The literal
ladder makes that explicit: a one-character literal costs 43.6 ns and a
sixteen-character one 52.8 ns, so length is worth about half a nanosecond per
character while the search itself costs forty-four. The two long-subject entries
are the same thing — both find their match almost immediately and then pay the
same flat cost.

The first two are not that: they leave far more above the flat cost than it
accounts for.

### The two worst cases

What separates them is ablation rather than profiling — removing the captures
from the pattern, then removing the failed start from the subject, and reading
the difference. For `(\w+)@(\w+)\.com`, before the changes this found:

| Subject | Perex | Charged work |
|---|---:|---:|
| `mail user@example.com now` | 470 ns | 159 |
| the same, pattern without captures | 400 ns | 134 |
| `user@example.com`, no failed start | 235 ns | 97 |

Half the case was one failed start, and captures were 16 ns of the whole. The
70 ns between the first two rows was therefore not capture bookkeeping: it was a
capture group's closing `SAVE` standing between the repeat and what consumes
next, which hid the continuation from the filter that decides which endpoints of
an over-consumed repeat are worth a retry frame. `\w+@` had the filter and
`(\w+)@` did not. [repetition](repetition.md) records that, and the walk itself,
which decoded every endpoint through the cursor where a byte comparison would do.

What is left is the flat cost once per start attempt, plus the instructions
between — the same thing the ladder below measures, paid twice.

### The floor, and what it is

A diagnostic ladder separates fixed cost from marginal cost:

| Case | Perex | V8 `test` | V8 `exec` |
|---|---:|---:|---:|
| `/z/` against `""` | 15.5 ns | 12.5 ns | 14.2 ns |
| `/z/` against `"a"` | 20.5 ns | 15.1 ns | 16.9 ns |
| `/a/` against `"a"` | 44.8 ns | 18.2 ns | 33.7 ns |

An empty search is 1.24x against the boolean test and 1.09x against the
capturing one: that is the cost of a bytecode pausable at every
instruction so the host's collector can run, which is why this engine exists.
The step from a miss to a hit is 24 ns, against V8's 3 for the boolean test and
17 for the capturing one — and the capturing one is the comparison that matches
what a hit here produces. It buys register initialization, four instructions,
capture validation and copying the result out, about 2 to 3 ns for each
primitive operation, which is what a bytecode interpreter with bounds-checked
scratch costs and several times what compiled code costs. V8 emits native code
and owes a collector nothing.

There is no remaining hot spot behind that number, and it was priced rather
than assumed. Removing one piece of the successful path at a time, and measuring
what each removal recovers from `/a/` against `"a"`:

| Piece removed | Recovered |
|---|---:|
| Checking the registers a match reports | 3.3 ns |
| Copying the result out | 3.0 ns |
| Clearing the registers a trial starts from | 3.9 ns |

The last of those is an upper bound rather than a price — a search that does not
clear its registers is not the same search, and removing the write lets the
compiler drop the slice around it too. The first two are exact. What is left of
the twenty-four is four instructions, entering the trial, and the rounds between.
Nothing in the list is large, and putting any of them where the round before it
ends was measured and made things worse.

This was also checked from the other direction, by counting the rounds rather
than pricing their contents. `/a/` against `"a"` takes seven dispatcher rounds
and the capture case forty-seven. Removing eight of those forty-seven recovered
three percent; running three of a start's seven in one round recovered four
percent on the shortest hits and nothing elsewhere; and removing the round every
match spends checking its registers cost five to ten percent on the two cases
furthest behind. Three measurements agree that the dispatcher is not where the
remaining gap is.

The cost is distributed across what the phases do, in the shape the ladder
shows, so no further tuning of this design removes it.

The scan is not where the remaining gap is, and it is no longer where any of
it is. Two claims recorded here earlier were wrong, and the measurements that
refuted them are in the repository.

*Auto-vectorisation does not substitute for SIMD intrinsics.* It does. LLVM
widens a loop written the way its vectoriser wants — a fixed-size array, a
fixed trip count, no iterators, no exit inside the block. Per byte scanned, on
this machine: a three-member set at 0.050 ns in block form against 0.107 for
lane arithmetic and V8's 0.158.

*The gap is scanning several literals in one pass.* It was not the scan. Every
scan decided a position on one byte and compared the prefix at each position
that admitted, and the comparison was the cost. Both claims Perex carries
already prove a second character, so a position is now decided on the pair,
which on ordinary text admits almost nothing. Every scan-bound case is ahead of
V8 with `#![forbid(unsafe_code)]` intact.

### Attempts that did not work

Recorded because a measurement that refuted a plausible idea is worth as much
as one that confirmed it, and because each of these looked obviously right.

- **Inline class test in the `CLASS` instruction arm.** No change: a repeated
  class is dispatched from the atom scan and never reaches that arm. Moving the
  test to the scan afterwards gave 1.6x.
- **Completing a pure literal without the trial loop.** Within noise; the
  remaining instructions cost about 1 ns each, not the 6.5 ns they cost before
  the leading-run skip landed.
- **A skip table over the leading literals.** 1.08x. Its dependent load chain
  is latency-bound where the existing lane arithmetic is not.
- **Caching the leading characters per scan step.** 1.5x *worse*, and worse
  again after the step was enlarged sixteenfold and the first-byte filter
  restored. Reading an instruction per character is not the cost that the
  arithmetic suggested it was.
- **Beginning the trial after a single verified character.** A run of two or
  more that the start scan compared is skipped rather than executed. A
  descriptor admitting exactly one byte decides one character the same way when
  the instruction that consumes it wants that byte, which is a sound claim and
  was implemented as one. It recovered 1.5 ns of the two cases it applies to and
  cost 1.0 to 2.6 ns on five cases that never reach the branch it added,
  including the two furthest behind. The loss is code layout around the phase,
  not work: this is the third change in a row whose effect on the hot paths was
  larger than what it removed.
- **Checking a match's registers in the round that produced it.** That check
  is its own phase, and ablating it showed it costs 3.3 to 4.1 ns of every
  successful search — seven to eight percent of the shortest ones — almost all
  of it the round trip rather than the two loads it performs. Running it where
  the trial ends recovered about a nanosecond on those, and cost five to ten
  percent on the two worst cases, consistently across three alternating passes.
  Testing the phase after every trial return perturbs the hottest path in the
  engine by more than a whole round trip is worth. The round trip is real and
  still not where the gap is.
- **Lowering the length at which a repeated class uses block comparisons.**
  The 256-byte threshold was chosen when the block form ended with a scalar
  pass over all thirty-two lanes to find the one it stopped at. That pass is now
  a word-wise search, so the threshold looked worth revisiting; it is not. At 64
  bytes nothing moved, and at 32 a sixty-character `\w+` went from 146 to 194 ns
  and a 256 KiB class miss from 21.1 to 24.0 µs. The original choice survives the
  change that prompted the retest.
- **A fixed count of range comparisons for a repeated class.** A class of two
  to four ranges is decided by a loop that exits at the range that matches, and
  a run shorter than a word reaches only that loop. Padding the unused slots
  with a range nothing is inside, so the count is fixed and the compiler can
  unroll it without a branch, was slower on all five cases measured, by two to
  four percent each. Reverting restored them, so it was the change and not the
  machine. Branching out of two comparisons beats not branching out of eight.
- **Hoisting the atom scan's byte predicate out of its entry.** A profile
  taken with the hot functions forced out of line put twelve percent of the
  worst case in rebuilding that predicate. Recovering it gained nothing: the
  attribution was an artifact of the measurement. Counting how often each phase
  ran found the real distribution instead, and the round trip through the phase
  dispatcher turned out to cost about two nanoseconds — enough to be worth
  batching a seek and a rollback, and nowhere near enough to explain the gap.

### What closing the rest took

One route remained, and it was not the scan. Every case still behind is a search
short enough that the fixed cost of starting one dominates it, and that cost is
interpretation: a bytecode pausable at every instruction, which is why this
engine exists. Closing it meant compiling rather than interpreting, which is
[issue #2](https://github.com/PerryTS/perex/issues/2)'s decision, and the answer
was a compilation tier rather than SIMD or `unsafe`.

The crate stays `#![forbid(unsafe_code)]`. `native::emit` writes machine code
into a caller-owned `&mut [u8]` and returns its length; a verifier proves the
memory, control-flow, calling-convention and budget properties of those bytes
before they are returned. Mapping the buffer executable and calling it is the
host's, which is where the `unsafe` lives and where it is auditable — the same
boundary as every other buffer this engine writes into. See
[compilation](compilation.md).

Measured against V8 on the twenty cases both drivers run, with the path the
crate's own rule picks: seventeen are at or better than V8 at both entry points,
including six of the seven in the table above. The three still behind are
`[a-z]+!`, `[^0-9]+!` and `\w+!` — a class repeat followed by a literal, one of
which is that table's seventh row. There the rule hands the search to the
interpreter, because generated code has no first byte to scan for and only the
subject would decide, which the rule will not look at. The per-case table is in
[compilation](compilation.md#what-a-host-gets-against-v8).

The SIMD route that issue also offered is withdrawn: it was there to close the
scan-bound cases, and those are closed without it.

## The interpreter alone, 2026-09-16

The compilation tier is not in any host yet, so what a host runs today is the
interpreter. Against V8 on all twenty-five cases, measured with
`bench/compare.sh` before the changes below, it was behind at both entry points
on nineteen, between 1.48x and 7.43x against `test`, and ahead on the six long
subjects by between 1.9x and four orders of magnitude.

Three changes were tried to see how much of that the interpreter can close
without compiling, each measured against the previous engine in one process,
interleaved per round.

- **Deciding a short search before it seeks.** Kept. An ASCII remainder under
  64 bytes is scanned once on entering admission: no byte a match can begin
  with decides the search, and the first one is where it starts. The three
  short misses went from behind V8 at both entry points to ahead of both —
  `/z/` against `"a"` from 23.6 ns to 12.5, against V8's 14.9 and 16.8 — and
  the short literal hits gained 2 to 7 percent. See
  [candidate](candidate.md#short-remainders).
- **A boolean entry.** Kept. `executor::is_match` and
  `Search::without_captures` end a search at its match instead of checking the
  capture registers `find` copies out. Short matches gain up to 17 percent —
  `/a/` against `"a"` from 50.0 ns to 41.5 — and nothing is slower. It moves
  none of the matches ahead of V8's `test`. See [engine](engine.md).
