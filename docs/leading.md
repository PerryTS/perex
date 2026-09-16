# Leading literal starts

The word 7 descriptor proves which ASCII character a match's first consumed
position may hold. That admits one byte, so a subject containing many copies of
that byte still initializes registers, begins a trial and enters the instruction
loop at every one of them. Program word 9 extends the same idea from one
character to a short run, so an impossible start is rejected by a byte
comparison instead of a trial.

This is an optimization inside one matcher. It never supplies captures, chooses
between matches, or provides a second engine or fallback.

## Derivation and format

Execution begins at instruction zero. A straight-line run of entry `SAVE`
bookkeeping followed by ASCII `CHAR` instructions therefore consumes exactly
those characters on every successful path, whatever follows them. The run ends
at the first instruction that is anything else: a branch, a jump, an assertion,
a class, a repeat, a folded `CHAR_I`, a non-ASCII character, or the interior
`SAVE` of a capture group. A run shorter than two characters is not recorded,
because word 7 already carries that claim.

ASCII bytes cannot occur inside a multibyte UTF-8/WTF-8 encoding, so a run of
ASCII characters is also a claim about original bytes.

Program format 13 has eleven header words. Word 9's low byte holds the run's
character count, or zero when disabled; its top bit is the separate
forward-admission claim described in [admission](admission.md). The word costs
four used program bytes, and the two claims share it rather than adding another.

Unlike the word 7 and 8 descriptors, the run count is **re-derived** from the
instructions by `Program::from_words` and rejected when it disagrees. A
format-valid edited count is therefore not executable, and no program can
disagree with its own instructions here. The derivation inspects a bounded
number of entry instructions and charges the work budget. The validator also
rejects any reserved bit and a forward claim without its condition.

## Folded runs

A case-insensitive literal has a worse word 7 descriptor than a sensitive one:
`/NeEdLe/i` admits every character from `N` to `n`, thirty-three of them, so
ordinary lowercase text stops the scan constantly.

Word 9's low byte sets bit 7 when the run is case-insensitive, and the
comparison then ignores ASCII case. This is exact rather than approximate: the
scan runs only on wholly ASCII storage, and the two non-ASCII characters that
fold into ASCII — U+017F and U+212A — cannot appear there at all, so no folded
match can be missed and none can be invented.

The same exactness is what lets the scan itself name the bytes a folded run
admits, rather than the interval between them: either case of each of the first
two characters, at most four pairs.

A run is wholly folded or wholly not. A change of kind ends it like any other
opcode, so `(?i:ab)cd` claims only `ab`.

## An alternation of runs

A pattern whose entry is a branch — `(?:needle|thimble|haystack)` — has no
single leading run, and its word 7 descriptor is worse than a single literal's:
a union of first characters is widened into one interval, so `{h, n, t}` admits
every character from `h` to `t`. On ordinary text that stops the scan at a large
fraction of positions, each of which previously entered a trial.

Word 9's low byte value 1 records that the entry is a branch whose every
alternative begins with at least two ASCII characters. The branches themselves
are not stored: they are already in the instructions, so the check walks the
`SPLIT` structure and compares each alternative's characters against original
bytes, succeeding as soon as one matches. The claim costs no program storage at
all, and like the single-run count it is re-derived during validation.

The scan itself also changes for an alternation. A union of first characters is
widened into one interval for the word 7 descriptor, so `{h, n, t}` is scanned
as `h` through `t`; the alternation instead scans the exact pairs its branches
begin with.

A branch qualifies only if every alternative is a run of at least two ASCII
characters. A jump, class, repeat, folded or non-ASCII character anywhere in the
prefix makes it undecidable and disables the claim. At most eight alternatives
are walked, each needing one stack slot and no match state.

## Scanning for two characters

Deciding a position on its first byte alone admits far more of them than can
match. `/needle/` over the 256 KiB benchmark subject stops at every `n`, 7,281
of them; `/NeEdLe/i` stops wherever the interval from `N` to `n` holds, which
ordinary lowercase text does at a third of its positions; the three-way
alternation stops at 21,843. Each of those was then compared against the prefix,
and that comparison, not the scan, was where the time went.

Both claims already prove a second character: a recorded run is at least two
characters, and every branch of an admitted alternation is too. A position is
therefore decided on the pair. The second byte is as much a necessary condition
of a match as the first, so this skips nothing a match could have started at,
and on ordinary text it admits almost nothing: none of `ne`, `th` or `ha` occurs
in the benchmark subject's repeating alphabet at all.

Both bytes of every pair are compared against a block of thirty-two lanes at a
time, in the shape [performance](performance.md) records as widening into the
target's vector comparisons, and against one word below a block's length, so a
short subject reaches a widened form too. A scan split into rounds by the
quantum reads past its own chunk to decide that chunk's last position, and
bounds only the positions it decides, so a pair is never split between rounds.

The pairs are derived once per round, and only for a program carrying a claim:
for a short subject, describing the scan costs more than the scan itself.

## Execution, lifetime and work

The scan borrows the original validated bytes and applies only to ASCII storage,
where byte offsets, character offsets and UTF-16 offsets coincide. Mixed
WTF-8 and native UTF-16 subjects keep the general start search unchanged.

Within one scan step a word-parallel search locates the next position that can
begin a match: the pair search above when word 9 is set, and the word 7
descriptor's interval otherwise. When word 9 is set, the remaining characters
are compared against the program's instructions before that position becomes a
start. A mismatch resumes the search at the following byte inside the same
bounded step; a match publishes the start exactly as before. Every compared
position is charged, so the comparison cannot outrun its allowance, and the
bounded step remains resumable across relocation and scratch replacement.

A position whose remaining bytes cannot hold the run ends the search: no later
start has more room. Sticky matching still checks only its requested position.
No subject address survives the scoped borrow, and the scan adds no frame, undo
entry or match-scratch buffer.

## Checks and measurement

`tests/leading.rs` covers the derivation across branches, assertions, classes,
folding, non-ASCII characters and interior capture bookkeeping; the rejection of
a disagreeing claim; complete answers at every start offset; subjects too short
to hold the run; sticky starts and captures; and unchanged answers on mixed and
UTF-16 storage. `tools/check-candidate.mjs` compares complete answers against
Node in ordinary and one-/seventeen-work-unit advances with relocation and
scratch growth, which exercises this path together with the word 7 descriptor.

Measured against the previous revision by alternating the identical driver
between both binaries on one 256 KiB ASCII subject with a six-character
pattern, so the same machine load applies to both. The searching case was
2.75x faster at the median of six rounds (range 2.17x to 6.15x), and the
equivalent long miss about 3.6x over three rounds (3.38x to 4.86x). The host
machine carried a load average above 20 throughout, so absolute times from it
are not reportable; the alternating ratio is.

Charged work is essentially unchanged (305,817 to 305,823 units on the
searching case), because rejecting a candidate costs roughly what the trial it
avoids was charged. The saving is in real cycles: a byte comparison replaces
register initialization, a trial and instruction dispatch. Work accounting
therefore cannot show this improvement, and neither figure alone describes it.

These are single-case standalone figures; they are not a whole-application
result and do not establish a win for patterns without a leading run. Cases
whose first characters usually do continue into the run pay the comparison
without removing a trial, and must stay visible in comparisons.

Deciding a position on two characters rather than one was measured against V8
on the same machine, both drivers reporting process CPU time, with the case
list and raw results under `bench/`:

| Case | One character | Two | V8 |
|---|---:|---:|---:|
| `/needle/` over 256 KiB | 57.7 µs | 11.4 µs | 70.8 µs |
| The same, missing | 57.7 µs | 11.4 µs | 128 µs |
| `(?:needle\|thimble\|haystack)` over 256 KiB | 186 µs | 21.8 µs | 42.7 µs |
| `/NeEdLe/i` over 29 characters | 87.2 ns | 66.3 ns | 21.7 ns |
| `/needle/` over 29 characters | 50.2 ns | 52.0 ns | 43.1 ns |

The last row is the case the paragraph above warns about: a short subject whose
scan ends at the match anyway, where describing the pairs is not repaid. It is
within this machine's noise either way, and it is why the description is built
only for a program that carries a claim, and only once per round.

## Not executing what the scan already compared

A start published by the leading-run comparison has had each of those
characters compared against the subject. Executing their instructions repeats
that comparison, and cannot fail: the run is unconditional, so the trial begins
after it instead.

Only the contiguous-run claim does this. An alternation records nothing, because
which branch matched decides how far to skip and that is not what the claim
carries.

The entry `SAVE` instructions before the run still run, since they record where
the match began, and the skipped instructions are charged exactly as executing
them would charge. A search's total work is therefore unchanged, which is what
lets a paused search still reach an unpaused one's total.

Measured on literals of increasing length, always matching at position zero,
process CPU time per search:

| Pattern | Before | After |
|---|---:|---:|
| `/a/` | 45.6 ns | 44.1 ns |
| `/aa/` | 52.3 ns | 43.0 ns |
| `/aaaa/` | 61.3 ns | 43.8 ns |
| `/aaaaaaaa/` | 91.3 ns | 45.0 ns |
| `/a{16}/` written out | 143.6 ns | 47.9 ns |

The cost per additional character fell from about 6.5 ns to about 0.25 ns, so a
literal's length now barely affects its search. The authored short literal case
fell from 69 ns to 45 ns, which matches V8's reading in the same window.
