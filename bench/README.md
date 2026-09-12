# Benchmarks

Two drivers, both development-only. Neither is part of the published crate, and
neither is adoption evidence on its own: `docs/performance.md` states what
evidence this project requires and what these figures do and do not show.

## `perex-bench` — cross-engine comparison

Authored, self-contained cases. Every engine gets the same protocol: compile
once outside the timer, then run a warm search loop with reused scratch. The
match outcome is reported alongside the time, so a fast wrong answer shows up as
a correctness failure rather than a speed result.

```sh
cargo run --release --bin perex-bench            # all cases
cargo run --release --bin perex-bench short      # cases whose id contains "short"
cargo run --release --bin perex-bench -- --emit-cases > cases.json
```

The comparators are `regress`, another ECMAScript engine; `regex`, a linear-time
automaton with SIMD prefilters, which answers a different question and cannot
express backreferences or lookbehind; and `fancy-regex`, which can.

## Against V8

V8 is the engine Perry replaces, so it is the comparison that decides adoption.
`compare.sh` alternates both drivers on the same machine so the same load
applies to each, and reports the minimum of several passes.

```sh
./compare.sh /tmp/out 3
```

`node-bench.mjs` times **both** of V8's entry points, because neither is a
like-for-like comparison on its own: `perex::executor::find` always produces
captures, `RegExp.prototype.test` produces none, and `exec` produces them and
also allocates a result array of substrings that `find` never builds. The work
`find` does sits between the two, so every case is reported as a bracket.

Both drivers measure process CPU time rather than wall clock. On a loaded
machine wall clock measures how often the scheduler ran a process, not how much
work it did.

## `perex-native` — the compilation tier, executed

Perex emits machine code into a caller-owned buffer and never maps, protects or
calls anything: those are the host's, which is what keeps
`#![forbid(unsafe_code)]` on the engine. This driver is that host half. It maps
the buffer, makes it executable, calls it, and times it against the interpreter.

Every answer is checked against the interpreter at every start position before
anything is timed. See `docs/compilation.md`.

```sh
cargo run --release --bin perex-native
```

AArch64 only so far.
