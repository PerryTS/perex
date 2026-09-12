#!/usr/bin/env node
// Time the same cases in V8, the engine Perex actually replaces in the host.
// Same protocol as the Rust driver: compile once, warm up, median of five
// timed rounds, and report the answer so a fast wrong result is visible. Both
// of V8's entry points are timed; see below for why one of them alone is not a
// like-for-like comparison.
import { readFileSync } from 'node:fs';

const cases = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const filter = process.argv[3];
const ROUNDS = 5;

// Two entry points, because neither alone compares like for like with the Rust
// driver. `test` skips producing captures, which `find` always produces; `exec`
// produces them and also allocates a result array of substrings, which `find`
// never does. The work `find` does sits between them, so both are reported and
// the true comparison is bracketed rather than asserted.
console.log(`${'case'.padEnd(26)}${'test ns'.padStart(14)}${'exec ns'.padStart(14)}   answer`);
console.log('-'.repeat(70));
for (const c of cases) {
  if (filter && !c.id.includes(filter)) continue;
  let re;
  try { re = new RegExp(c.pattern, c.flags); } catch { console.log(`${c.id.padEnd(26)}${'-'.padStart(14)}   (rejected)`); continue; }
  const subject = c.subject;
  let last = false;
  for (let i = 0; i < Math.max(1, c.iters / 20); i++) last = re.test(subject);
  // Process CPU time, for the same reason the Rust driver uses it: a loaded
  // machine makes wall clock measure scheduling rather than work.
  const median = (run) => {
    const rounds = [];
    for (let r = 0; r < ROUNDS; r++) {
      const t0 = process.cpuUsage();
      for (let i = 0; i < c.iters; i++) run();
      const d = process.cpuUsage(t0);
      rounds.push(((d.user + d.system) * 1000) / c.iters);
    }
    return rounds.sort((a, b) => a - b)[ROUNDS >> 1];
  };
  const tested = median(() => { last = re.test(subject); });
  let found = null;
  for (let i = 0; i < Math.max(1, c.iters / 20); i++) found = re.exec(subject);
  const execed = median(() => { found = re.exec(subject); });
  const ok = last === c.expect && (found !== null) === c.expect;
  console.log(`${c.id.padEnd(26)}${tested.toFixed(1).padStart(14)}${execed.toFixed(1).padStart(14)}   ${ok ? 'ok' : `WRONG (${last}, expected ${c.expect})`}`);
}
