# Changelog

## 0.1.22 — 2026-11-18

- Less work at startup: when a program is published, its execution hints are derived once, after validation, and written into the existing header words, instead of being derived separately during emission and again during validation. An imported program still re-derives its hints and rejects one whose stored hints disagree. On a 448-pattern startup fixture, perex's exclusive instruction count fell 6.3% against 0.1.20.
- No API, program-format or cache-layout change; answers, complete captures, work limits and declines are unchanged.

## 0.1.21 — 2026-11-18

- Opt-in `selectable-unicode` feature: Unicode property membership is resolved once, when a program, atom or DFA is built or validated, and the compiled structure keeps the chosen function. Nothing is looked up per membership query. The DFA is specialized over its provider, so its membership call stays direct and inlinable: with the feature on, the DFA is within noise of feature-off on `\p{L}+`, `[\p{Script=Greek}\p{Nd}]` and `/\p{Lu}/iu`, and the VM is not slower. A host that leaves the tables out drops them from its binary (perry's chess binary: -131 KB).
- The default engine is unchanged and stays `no_std`; answers, complete captures, work limits and declines are unchanged.
- The release workflow now publishes the crate from the version tag.

## 0.1.20 — 2026-11-16

- Backtrack inside a trial without returning to the outer phase loop: the executor pops a frame and runs the Fail, Rollback and Clear phases in place when no pause or capacity request can fall between them, with the same charges. On the three.js track-name parse the engine-only instruction count fell from 21.8x node to 18x node.
- No API, program-format or cache-layout change; answers, complete captures, work limits and declines are unchanged.

## 0.1.19 — 2026-11-15

- Fewer checks per capture search: `dfa::captures` checks once per walk that every register a mask can name lies inside the cache, so a tagged step writes its registers without a check per write, reads its mask pair as one pair, and copies the groups out as one slice of register pairs. A bound view compares its 11-word program header as one fixed-size array. On a seven-group line pattern over 64-byte lines a host's whole exec costs about 0.6% fewer instructions.
- No API, program-format or cache-layout change; answers, complete captures, work limits and declines are unchanged (checked against 0.1.18 on 105,319 random cases over 22 patterns).

## 0.1.18 — 2026-10-30

- Walk captures in fewer instructions: `dfa::captures` takes plain transitions in a loop of their own over the subject cut at the work limit (one comparison per byte, the column table borrowed once), and applies a thread's tag events inside that loop: a capture state holds one thread, so its tag row sits at a fixed offset, and a program of sixteen groups or fewer reads one word of writes and one of clears. On a seven-group line pattern over 64-byte lines the capture search costs about 36% fewer instructions.
- No API, program-format or cache-layout change; answers, complete captures, work limits and declines are unchanged.

## 0.1.17 — 2026-10-29

- Read capture groups off the lazy automaton: `dfa::captures` returns the match that begins at a position with every group when each transition on the way keeps one path alive, recording each transition's capture effects (writes, and the per-iteration reset of captures inside a repeat) as interned tag records in the cache. A transition that keeps two paths alive is marked ambiguous in the automaton's own state and the search is declined to the evaluator, as before.
- Report the storage a thrashing program needs: `CacheStats::required_words`, derived from the states the cache held when it filled, so a host can allocate automaton storage on demand and grow it.
- Add `Decline::Captures`. The cache layout number advances to 2; older caches are rebuilt.
- A capture search that reaches an ambiguous transition marks the cache (`CacheStats::captures_declined`, `dfa::captures_declined`): every later capture search on it declines before walking, until the cache is reset, so a host asks the evaluator directly.
- Decide searches whose first-character descriptor admits no start before building any state: over wholly ASCII storage they answer no match without a cache, even over empty storage, which otherwise declines with `Decline::Storage`. A host can therefore allocate automaton storage only when a search needs states.
- Add `dfa::initial_words`, the size a host gives a program's first storage: its minimum with a state hash and region for one state per instruction.
- Document that storage need not be zeroed: a host only resets new storage (`dfa::reset`), since the automaton writes every word before reading it.
- No program-format change; complete captures, work limits and resumable searches are unchanged.

## 0.1.16 — 2026-10-28

- Execute adjacent exact literal and capture-save bytecodes directly over original ASCII storage, preserving each opcode work charge and paid SAVE retry after undo growth.
- Reuse a repeat record within an active atom execution chain without retaining pointers across pauses or storage relocation.
- Derive exact quantum endpoints once per evaluator run and compute opcode allowances only when used, preserving finite budgets and every pause boundary.
- Preserve program format 16, complete captures, original-input traversal and the existing evaluator for other storage and reverse searches.

## 0.1.15 — 2026-10-28

- Reduce repeated work when matching character repetitions and reading compiled programs.
- Read ASCII character classes directly from validated program storage.
- Preserve program format 16, complete captures, work limits and resumable searches across storage relocation.

## 0.1.14 — 2026-10-18

- Execute bounded start-anchored literal runs directly from immutable program data, without DFA storage or evaluator scratch.
- Compare ASCII subjects as original bytes, retaining lossless UTF-16/WTF-8 cursor semantics for other storage and Unicode starts between surrogate halves.
- Expose `Program::literal_prefix()` and `LiteralPrefix::find`; DFA entry points share the exact prefix reader and report a zero-word minimum for these programs.
- Advance the relocatable program format to 16 and validate the prefix descriptor against the complete instruction sequence; older program formats are rejected.
