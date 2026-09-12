#!/usr/bin/env node
// Complete-answer differential for the forward-admission bound. Every case
// pairs a pattern whose admission condition is consumed after the match start
// with a subject that places that condition before, after, around or across
// the positions a match could begin at. The bound must never change an answer.
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { runCase, parseAnswers, stableDifferences } from './reference.mjs';

const [probe, output, ...probeArgs] = process.argv.slice(2);
assert(output, 'usage: check-bound.mjs PROBE OUTPUT_DIR [PROBE_ARGS...]');
const cases = [];
function add(source, flags, subject, start_utf16 = 0) {
  cases.push({ id: `bound:${cases.length}`, source, flags: `d${flags}`, subject, start_utf16 });
}

// Patterns whose condition follows a repetition, plus controls whose condition
// an assertion can satisfy before the start, and which must keep every answer.
const patterns = [
  'a+END', 'a*END', 'a+?END', 'a{2,}END', '(a+)(END)', 'a+END+',
  '(?:a+END|b+END)', '(?:a|b)+END', '[ab]+END', 'a+[XY]', 'a+E.D',
  '(?i:a+end)', 'a+END$', '^a+END', '\\ba+END', 'a+(?=END)END',
  // Conditions an assertion reaches; these must not be bounded.
  '(?<=END)a+', '(?=.*END)a+', '(?<=END)a+END', 'a+(?<=DNE|END)',
  // No repetition at all, so no condition is inferred.
  'END', '(END)',
];

// The condition placed before every possible start, after it, repeatedly, at a
// chunk boundary, and split so that a partial copy cannot be mistaken for one.
const fillers = [40, 63, 64, 65, 255, 256, 257];
for (const source of patterns) {
  for (const flags of ['', 'g', 'y', 'gy', 'u', 'i', 'm']) {
    for (const n of fillers) {
      const a = 'a'.repeat(n);
      const q = 'q'.repeat(n);
      for (const subject of [
        `END${a}`,              // Only before every start.
        `${a}END`,              // Only after.
        `END${a}END`,           // Before and after.
        `END${q}aaaEND${q}`,    // A match that ignores the earlier occurrence.
        `${a}`,                 // Absent.
        `EN${a}D`,              // Split, so no complete occurrence exists.
        `${a}EN`,               // A partial copy at the very end.
        `ENDEND${a}`,           // Adjacent occurrences before every start.
        `${a}ENDEND`,           // Adjacent occurrences after.
        `${q}aENDa${q}aaEND`,   // Several usable occurrences.
        `end${a}END`,           // Case matters unless the pattern folds.
        `${a}\u{1f600}END`,     // An astral character between the two.
        `\u{1f600}${a}END`,     // Non-ASCII storage keeps the general search.
      ]) {
        for (const start of new Set([0, 1, n, subject.length, subject.length + 1])) {
          if (start > 0 && !flags.includes('g') && !flags.includes('y')) continue;
          add(source, flags, subject, start);
        }
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
mkdirSync(output, { recursive: false });
const save = (name, text) => writeFileSync(resolve(output, name), text);
const expectedText = cases.map(row => JSON.stringify(runCase(row))).join('\n') + '\n';
const input = cases.map(row => [row.id, encode(row.source), row.flags, encode(row.subject), row.start_utf16].join('\t')).join('\n') + '\n';
save('cases.json', JSON.stringify({ cases }));
save('expected.jsonl', expectedText);
save('input.tsv', input);
save('command.json', JSON.stringify([resolve(probe), ...probeArgs]) + '\n');
const run = spawnSync(resolve(probe), probeArgs, {
  input, encoding: 'utf8', maxBuffer: 256 * 1024 * 1024, timeout: 300_000,
});
save('actual.jsonl', run.stdout ?? '');
save('stderr.txt', run.stderr ?? '');
save('exit.json', JSON.stringify({ status: run.status, signal: run.signal, error: run.error?.message }) + '\n');
assert.ifError(run.error);
assert.equal(run.status, 0, run.stderr);
const actual = parseAnswers(run.stdout);
assert.equal(actual.size, cases.length);
const { differences, unstable } = stableDifferences(parseAnswers(expectedText), actual, cases);
const report = {
  node: process.version, cases: cases.length, probe_args: probeArgs,
  unstable_oracle_answers: unstable,
  differences: differences.length,
  first_differences: differences.slice(0, 25).map(d => ({ ...d, case: cases.find(row => row.id === d.id) })),
};
save('summary.json', JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify(report, null, 2));
process.exitCode = differences.length ? 1 : 0;
