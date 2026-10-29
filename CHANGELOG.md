# Changelog

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
