# Changelog

## Unreleased

- Chain repeated-atom phases within bytecode trials while retaining every work and quantum boundary.
- Borrow ASCII atom class predicates from validated record storage instead of constructing a maximum-sized temporary array on each scan.

- Borrow the immutable instruction, class-range and repeat records once per evaluator view, avoiding repeated layout decoding and whole-word bounds checks in dispatch and atom execution.
- Preserve program format 16, complete captures, work accounting, resource rebinding and offset-only resumable state. Record views do not survive an advance.

## 0.1.14 — 2026-10-18

- Execute bounded start-anchored literal runs directly from immutable program data, without DFA storage or evaluator scratch.
- Compare ASCII subjects as original bytes, retaining lossless UTF-16/WTF-8 cursor semantics for other storage and Unicode starts between surrogate halves.
- Expose `Program::literal_prefix()` and `LiteralPrefix::find`; DFA entry points share the exact prefix reader and report a zero-word minimum for these programs.
- Advance the relocatable program format to 16 and validate the prefix descriptor against the complete instruction sequence; older program formats are rejected.
