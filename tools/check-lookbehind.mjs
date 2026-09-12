#!/usr/bin/env node
// Complete-answer differential for lookbehind assertions whose body is a run of
// characters. Such a body is compared against the bytes before the position
// directly, rather than by reversing direction and reading each character, so
// the cases here put the position at and near the subject's start, mix the
// positive and negative forms, and include the storage and flags that must keep
// the general path: non-ASCII subjects, folding, and bodies that are anything
// other than a plain run.
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { parseAnswers, stableDifferences, boundedAnswers } from './reference.mjs';

const [probe, output, ...probeArgs] = process.argv.slice(2);
assert(output, 'usage: check-lookbehind.mjs PROBE OUTPUT_DIR [PROBE_ARGS...]');

const R = String.raw;
const patterns = [
  // Plain runs, positive and negative, of every small length.
  R`(?<=a)b`, R`(?<!a)b`, R`(?<=ab)c`, R`(?<!ab)c`, R`(?<=abc)d`, R`(?<!abc)d`,
  R`(?<=0123456789)a`, R`(?<!0123456789)a`,
  // The body alone, matching nothing, at every position.
  R`(?<=a)`, R`(?<!a)`, R`(?<=ab)`, R`(?<!ab)`,
  // Runs that must keep the general path.
  R`(?<=[ab])c`, R`(?<=a+)b`, R`(?<=a|b)c`, R`(?<=(a))b`, R`(?<=\w)b`,
  R`(?<=a.)c`, R`(?<=\u{e9})b`, R`(?<=😀)b`, R`(?<=a\b)b`, R`(?<=^a)b`,
  // Nested and repeated, where a frame really is needed.
  R`(?<=(?<=a)b)c`, R`(?<=a)(?<=ab)c`, R`(?:(?<=a)b)+`, R`(?<=a)b|(?<=c)d`,
  // A lookbehind after something else consumed, so the position is not the
  // match start.
  R`x(?<=x)y`, R`ab(?<=ab)c`, R`a+(?<=aa)b`,
  // Lookahead, which this must not touch.
  R`(?=ab)a`, R`(?!ab)a`,
];

const subjects = [
  '', 'a', 'b', 'ab', 'abc', 'abcd', 'ba', 'xb', 'aab', 'a'.repeat(12) + 'b',
  '0123456789a', 'x0123456789a', '0123456789', 'A', 'AB', 'Ab', 'aB',
  '\u{e9}b', '😀b', 'a😀b', '\ud800b', 'b\ud800', 'a\nb', 'xy', 'xxy',
  'abab c', 'cd', 'acd', '0123456789' + '0123456789' + 'a',
];

const cases = [];
for (const source of patterns) {
  for (const flags of ['', 'i', 'u', 'm', 'g', 'y', 'iu']) {
    for (const subject of subjects) {
      const anchored = flags.includes('g') || flags.includes('y');
      for (const start of new Set(anchored ? [0, 1, Math.max(0, subject.length - 1)] : [0])) {
        cases.push({
          id: `lookbehind:${cases.length}`,
          source,
          flags: `d${flags}`,
          subject,
          start_utf16: Math.min(start, subject.length),
        });
      }
    }
  }
}

function encode(text) {
  const bytes = [];
  for (const ch of text) {
    const p = ch.codePointAt(0);
    if (p < 128) bytes.push(p);
    else if (p < 2048) bytes.push(0xc0 | (p >> 6), 0x80 | (p & 63));
    else if (p < 65536) bytes.push(0xe0 | (p >> 12), 0x80 | ((p >> 6) & 63), 0x80 | (p & 63));
    else bytes.push(0xf0 | (p >> 18), 0x80 | ((p >> 12) & 63), 0x80 | ((p >> 6) & 63), 0x80 | (p & 63));
  }
  return Buffer.from(bytes).toString('hex');
}

mkdirSync(dirname(resolve(output)), { recursive: true });
mkdirSync(output, { recursive: true });
const save = (name, text) => writeFileSync(resolve(output, name), text);

const { answers: expected } = boundedAnswers(cases);
const expectedText = cases.map(row => JSON.stringify(expected.get(row.id))).join('\n') + '\n';
const input = cases
  .map(row => [row.id, encode(row.source), row.flags, encode(row.subject), row.start_utf16].join('\t'))
  .join('\n') + '\n';
save('cases.json', JSON.stringify({ cases }));
save('expected.jsonl', expectedText);
save('input.tsv', input);

const run = spawnSync(resolve(probe), probeArgs, {
  input, encoding: 'utf8', maxBuffer: 256 * 1024 * 1024, timeout: 600_000,
});
save('actual.jsonl', run.stdout ?? '');
save('stderr.txt', run.stderr ?? '');
assert.ifError(run.error);
assert.equal(run.status, 0, run.stderr);

const actual = parseAnswers(run.stdout);
assert.equal(actual.size, cases.length);
const { differences, unstable } = stableDifferences(expected, actual, cases);

const report = {
  node: process.version,
  cases: cases.length,
  patterns: patterns.length,
  subjects: subjects.length,
  probe_args: probeArgs,
  unstable_oracle_answers: unstable,
  differences: differences.length,
  first_differences: differences.slice(0, 25).map(d => ({
    ...d,
    case: cases.find(row => row.id === d.id),
  })),
};
save('summary.json', JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify(report, null, 2));
process.exitCode = differences.length ? 1 : 0;
