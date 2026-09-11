# Allocate final storage after parsing

`compiler::prepare` parses and sizes a pattern using caller-owned node/range
scratch and an operation budget. It returns `Prepared`, which retains only those
scratch and budget borrows plus integer metadata. It has no pattern or flag view.
The host can close its pattern borrow before allocating final program storage.
`required_words()` gives the exact output size; `emit` consumes the plan and
returns a program borrowing only output. Scratch can then be released.

The existing `compile` API uses these same preparation and emission steps. A
shared internal continuation allows it to emit directly without returning a
separate plan to the caller. There is one parser, emitter, program format and
matcher. The emitted format remains 9. Static and dynamic compilation use the
same pipeline; this API does not implement automatic Perry AOT emission.

The plan is not cloneable. It retains the original mutable budget borrow, and
emission accepts no replacement budget. Dropping a plan cancels it and releases
the borrows without an allocation callback. Output storage failure also consumes
the plan: allocate the reported size before calling `emit`. Every emission error
invalidates the first output word, including a late validation/work error.
Successful emission validates the exact returned words before publishing a
`Program`. An oversized caller buffer is only written through the required size.

Scratch must remain stable while borrowed by the plan. Its elements contain only
compiler values and offsets, never host object references. Allocating output may
trigger host GC only after all original pattern views have ended; the API makes
that possible but does not close unrelated views retained by the caller. Perry
must still trace its own pattern/program owners and manage their lifetimes.

This separates preparation from final storage allocation; it is not a resumable
parser. Insufficient node/range scratch still requires a fresh preparation with
adequate storage and the remaining operation budget. The parser, preparation
checks and emission still run synchronously within finite work limits. Bounded
moving-GC pauses, growable parsed state and actual Perry host adoption remain
unfinished. The small-buffer diagnostic is not a production allocation policy.

Tests poison and free original pattern/flag storage before emission, free scratch
while the emitted program remains live, exercise original lone-surrogate input,
and check the full budget range through late validation failure. Compile-fail
examples prevent budget reset or scratch mutation while a plan is live.
