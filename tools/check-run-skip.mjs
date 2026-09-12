#!/usr/bin/env node
// Complete-answer differential for skipping a failed start's atom run.
//
// When a pattern begins with an unbounded repeat of one atom and an attempt
// fails, every later start inside the run that repeat walked must fail too, so
// the search resumes at the run's end. This is the riskiest kind of
// optimization in the engine: it does not change how a start is tried, it
// removes starts entirely, so a wrong claim hides a real match.
//
// The cases below put matches at every position relative to a run — inside it,
// at its end, immediately after it, in the next run — and include the shapes
// that must disable the claim: a bounded repeat, whose reachable positions from
// a later start are not a subset, and a backreference, which makes the rest of
// the pattern depend on what the run captured.
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { parseAnswers, stableDifferences, boundedAnswers } from './reference.mjs';

const [probe, output, ...probeArgs] = process.argv.slice(2);
assert(output, 'usage: check-run-skip.mjs PROBE OUTPUT_DIR [PROBE_ARGS...]');

const R = String.raw;
const patterns = [
  // Unbounded leading atom repeats: the claim applies.
  R`[a-z]+[0-9]+`, R`[a-z]*[0-9]+`, R`[a-z]+?[0-9]+`, R`[a-z]{2,}[0-9]+`,
  R`\w+@\w+`, R`(\w+)@(\w+)\.com`, R`([a-z]+)([0-9]+)`, R`[a-z]+$`,
  R`a+b`, R`a*b`, R`a+?b`, R`.+z`, R`[^0-9]+[0-9]`, R`[a-z]+(?=[0-9])`,
  R`[a-z]+(?![a-z])`, R`[a-z]+\b[0-9]`, R`[a-z]+(?:[0-9]|!)`,
  R`[a-z]+[0-9]*!`, R`(?:[a-z]+)[0-9]+`, R`[a-z]+[0-9]+$`, R`^[a-z]+[0-9]+`,
  // Bounded repeats and backreferences: the claim must be absent.
  R`[a-z]{1,3}[0-9]+`, R`[a-z]{2,4}[0-9]`, R`a{1,2}b`,
  R`([a-z]+)\1`, R`([a-z])\1[0-9]`, R`(?<g>[a-z]+)\k<g>`,
  // Astral and folded atoms.
  R`[\u{1f600}-\u{1f610}]+x`, R`[a-z]+X`, R`\S+\d`,
];

// Subjects placing a match inside a run, at its end, just past it, in a later
// run, and nowhere, with runs of every small length.
const subjects = [];
for (const run of ['', 'a', 'ab', 'abc', 'abcd', 'abcde']) {
  for (const tail of ['', '1', '12', '!', 'z', '@ab', 'X', '\u{1f600}']) {
    subjects.push(run + tail);
    subjects.push(run + tail + run + '1');
    subjects.push(run + ' ' + run + tail);
  }
}
subjects.push(
  'value abc123 end', 'mail user@example.com now', 'aaaa1', 'aaaab',
  'abcabc', 'abab1', 'zzz999zzz', 'a'.repeat(40) + '1',
  'a'.repeat(40), 'ab'.repeat(20) + '9', '\u{1f600}'.repeat(4) + 'x',
  'AAAbbb111', '  abc123', 'abc\n123',
);

const cases = [];
for (const source of patterns) {
  for (const flags of ['', 'i', 'u', 'g', 'y', 'm']) {
    for (const subject of new Set(subjects)) {
      const start = flags.includes('g') || flags.includes('y') ? 1 : 0;
      cases.push({
        id: `run-skip:${cases.length}`,
        source,
        flags: `d${flags}`,
        subject,
        start_utf16: Math.min(start, subject.length),
      });
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
  subjects: new Set(subjects).size,
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
