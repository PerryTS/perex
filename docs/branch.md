# Branches

An alternation that fails on its first alternative used to pay for every
other one in turn: a frame popped, a rollback, a dispatcher round trip, and the
alternative's first instruction executed only to fail. Claude Code's emoji
pattern, an alternation of eleven top-level alternatives and about two hundred
nested ones, cost 6,664 instructions per search against the one-character
subject `"1"`; V8's whole `test` call costs about 240. Format 14 changes three
things, each a property of the program alone, and keeps the one evaluator.

## Flat layout

`a|b|c` parses as `(a|b)|c`. Emitted as parsed, the first alternative was
entered under one live frame per alternative, and a success left through one
jump per nesting level. Ordered choice is associative, so the emitter now lays
a left-nested chain out flat:

```
SPLIT  → a, R1     a   JUMP end
R1: SPLIT → b, R2  b   JUMP end
R2: c
end:
```

Each alternative but the last sits behind its own branch, whose frame resumes
at the next alternative; each jumps straight to the end. The size is the same
— two instructions per alternation either way — and the order in which
alternatives are tried, and so every answer, is unchanged. The left spine's
inner alternation nodes become no instruction.

## First-character sets

A new instruction, `BRANCH` (opcode 31), is a `SPLIT` whose first arm begins
at the next instruction and whose operand `a` is the set of characters that arm
can consume first. When the character the arm would read — in the direction the
code runs, so backwards inside a lookbehind — is absent or outside the set, the
arm cannot match: trying it would fail and resume at `b` with nothing changed.
The evaluator goes to `b` directly, without a frame. When `b` is itself a
`BRANCH`, it is decided against the same character in the same step, so a
chain of alternatives that cannot begin here costs one read and one comparison
each. Each branch passed over is charged one work unit, exactly as executing it
would be, and the chain stops at the quantum the next instruction would have
been given, so totals do not depend on how a search is paused.

The set is a 32-bit mask of character buckets. ASCII characters fall in 24
buckets by their low five bits, so both cases of a letter share one; every
other character falls in one of 8 buckets by its high bits, which separates
surrogate halves, the symbol blocks and the CJK blocks. It is derived from the
emitted instructions by `derive_first_mask`: from the arm's first instruction,
capture `SAVE`s and position assertions are passed over (dropping a condition
only widens the set), jumps are followed, and both arms of a nested branch are
taken; each path ends at its first consuming instruction, which contributes
its characters. A path that can succeed without consuming, or that reaches
anything else — an assertion body, a repeat, a backreference, a property, a
complement, a folded class, a folded non-ASCII character — makes the set
unknown, and the branch stays a `SPLIT`. A folded ASCII letter under `u` also
admits every non-ASCII bucket, for U+212A and U+017F. The walk inspects at most
32 instructions and eight pending paths.

`Program::from_words` derives every `BRANCH` mask again and rejects a program
whose operand differs, or whose mask admits everything. So, like the leading
run, a branch's set cannot claim less than its arm can consume.

## Start-anchored claim and start registers

The claim that every match begins at the subject's start was derived at the
start of every search; on the nested emoji alternation that walk was 313 of the
654 instructions a miss on a letter cost. It is now stored as word 7's bit 24
and re-derived on validation; see [candidate starts](candidate.md#start-anchored-starts).

A trial cleared every register before it began: two per capture and two per
repeat. The emoji pattern has 194 repeats, one per `?`, so every start cleared
390 registers and charged 390 work units for it. A repeat's registers are
written by its `REPEAT_INIT`, the only way into the repeat, before anything in
it reads them, and a frame or undo entry that restores an older value restores
it to a point before that write. A trial now clears only the capture registers.

## Measurement

Instructions per search (`instructions:u`, difference between 200,000 searches
and none) of `Search::run` over a counted binding at a quantum of 4096, as a
host calls it per `test`, against 0.1.11 in the same harness. "cc mix" cycles
the subjects `t o k e n 1 7` and a space, the graphemes string-width tests in
Claude Code's streamed output.

| Case | 0.1.11 | 0.1.12 | Change |
|---|---:|---:|---:|
| `/^\p{DI}$/u`, cc mix | 432 | 391 | −9.5% |
| `/^\p{DI}$/u` on 中 | 1,505 | 1,459 | −3.1% |
| emoji `/g`, cc mix | 2,212 | 946 | −57.2% |
| emoji on `t` | 728 | 406 | −44.2% |
| emoji on `1` | 6,664 | 2,569 | −61.4% |
| emoji on 中 | 6,741 | 1,637 | −75.7% |
| emoji on 😀, a match | 11,014 | 5,638 | −48.8% |
| `/ab\|cd\|ef\|gh/` on `xxgh` | 3,183 | 2,124 | −33.3% |
| `/(foo\|bar\|baz)=(\d+)/` on `a=1&baz=42` | 4,241 | 3,819 | −10.0% |
| `/a/` on `t` | 446 | 406 | −9.0% |
| `/a/` on `a`, a match | 1,206 | 1,162 | −3.6% |
| `/qq_[0-9]+/` on `record_12345` | 498 | 458 | −8.0% |
| `/\w+/g` on `foo bar` | 2,231 | 2,189 | −1.9% |
| `/^[a-z]+_[0-9]+$/`, a match | 3,041 | 2,993 | −1.6% |
| `/\s+$/` on a trailing run | 4,143 | 4,095 | −1.2% |

No case measured slower. What remains is mostly the fixed cost of a search
that enters the evaluator: about 1,100 instructions to match `/a/` against
`"a"`, spread over a dozen phase round trips (admission, the start scan, the
seek, clearing, the trial, validation) of 30 to 100 instructions each.

## Verification

`tests/branch.rs` compares 36 patterns against the same patterns with every
alternative, at every depth, written between empty lookaheads — whose arms
begin with an assertion, so no branch carries a set and every arm is tried —
from every start including past the end, over ASCII, folded, non-ASCII,
astral and keycap subjects: alternations of literals, classes, captures and
named groups, lookbehind and negative lookbehind alternatives, folding with
U+212A and U+017F under `i` and `iu`, empty and end-asserting alternatives,
backreferences, repeated alternations and sticky searches. It checks the flat
layout and source order, that repeat registers holding arbitrary values do not
change any answer, and that validation rejects every single-bit change to a
mask, a mask admitting everything, either flip of the anchored bit and a
reserved bit above it. Six injected faults are each caught by the suite:
non-ASCII characters bucketed as ASCII, `REPEAT_INIT` leaving its position
register unwritten, a folded non-ASCII character claiming only its own bucket,
an arm that can match empty contributing nothing, a chain resuming one
instruction past the next alternative, and a trial clearing no capture
register. The engine differential (plain, one- and seventeen-unit resumption
with relocation and growth, from positions, decided in one call), the Test262
pattern corpus, and the input, candidate, classes, names, legacy, admission,
lookbehind, run-skip, repetition, modifiers, bound, end-candidate, sets,
sequences, Unicode-sets, casefold, properties and atom-filter harnesses, and
the native tier's check, report no differences.
