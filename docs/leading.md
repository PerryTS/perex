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

Program format 12 has ten header words. Word 9's low byte holds the run's
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

A branch qualifies only if every alternative is a run of at least two ASCII
characters. A jump, class, repeat, folded or non-ASCII character anywhere in the
prefix makes it undecidable and disables the claim. At most eight alternatives
are walked, each needing one stack slot and no match state.

## Execution, lifetime and work

The scan borrows the original validated bytes and applies only to ASCII storage,
where byte offsets, character offsets and UTF-16 offsets coincide. Mixed
WTF-8 and native UTF-16 subjects keep the general start search unchanged.

Within one scan step the existing word-parallel search locates the next position
the word 7 descriptor admits. When word 9 is set, the remaining characters are
compared against the program's instructions before that position becomes a
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
