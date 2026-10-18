# Start-anchored literal execution

Program format 16 adds word 7 bit 26. The compiler derives it only for the
complete instruction sequence `SAVE 0; START; CHAR+; SAVE 1; MATCH`, with
one capture (group zero) and at most 32 characters. Validation re-derives
it and rejects either a forged or a missing claim. The instructions remain
the program data; there is no second program, native code or retained state.

`Program::literal_prefix()` obtains an exact executor. Its `find` compares
those literal characters with the original input and returns group zero's
span. ASCII byte storage is compared directly without constructing cursor
state; other storage uses the lossless cursor. Multiline assertions, case folding, captures and other suffixes
use the ordinary evaluator or automaton. Unicode starts between a leading
surrogate pair's halves normalize exactly as the evaluator does. Work is
charged for the check and each compared character; the 32-character bound
keeps synchronous work bounded. No scratch or cache storage is needed.
The DFA entry points dispatch these programs to the same executor before
reading cache storage, and `minimum_words` returns zero for them.

## Verification

`cargo test --test literal_prefix` compares complete evaluator answers for
ASCII, non-ASCII, astral and lone-surrogate inputs in original UTF-16 and
WTF-8 storage, all starts, global/sticky/Unicode flags, relocation, work
exhaustion and forged descriptors. `cargo test --test dfa` checks both
bounds and Boolean entry points against the evaluator.
