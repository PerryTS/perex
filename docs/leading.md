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

Program format 11 has ten header words. Word 9 holds the run's character count,
or zero when disabled. The word costs four used program bytes.

Unlike the word 7 and 8 descriptors, word 9 is **re-derived** from the
instructions by `Program::from_words` and rejected when it disagrees. A
format-valid edited claim is therefore not executable, and no program can
disagree with its own instructions here. The derivation inspects a bounded
number of entry instructions and charges the work budget.

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

Measured against the previous revision on one 256 KiB ASCII subject with a
six-character pattern, a search that previously entered a trial at every copy of
the first character fell from about 225 µs to about 57 µs, and the equivalent
long miss from about 226 µs to about 69 µs. A scan with no admitted position at
all was already about 24 µs, so a remaining gap to that floor is the per-position
comparison rather than the byte scan. These are single-case standalone figures
on one machine; they are not a whole-application result and do not establish a
win for patterns without a leading run. Cases whose first characters usually do
continue into the run pay the comparison without removing a trial, and must stay
visible in comparisons.
