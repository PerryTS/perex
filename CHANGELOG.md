# Changelog

## 0.1.14 — 2026-10-18

- Execute bounded start-anchored literal runs directly from immutable program data, without DFA storage or evaluator scratch.
- Compare ASCII subjects as original bytes, retaining lossless UTF-16/WTF-8 cursor semantics for other storage and Unicode starts between surrogate halves.
- Expose `Program::literal_prefix()` and `LiteralPrefix::find`; DFA entry points share the exact prefix reader and report a zero-word minimum for these programs.
- Advance the relocatable program format to 16 and validate the prefix descriptor against the complete instruction sequence; older program formats are rejected.
