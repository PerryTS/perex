#!/usr/bin/env node
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { runCase, parseAnswers, compareAnswers } from './reference.mjs';

const [probe, output, ...probeArgs] = process.argv.slice(2);
assert(output, 'usage: check-candidate.mjs PROBE OUTPUT_DIR [PROBE_ARGS...]');
const cases = [];
function add(source, flags, subject, start_utf16 = 0) {
  cases.push({ id: `candidate:${cases.length}`, source, flags: `d${flags}`, subject, start_utf16 });
}

// Starts straddle scalar and word/chunk boundaries. Captures, empty results and
// errors are compared in full; a plausible first character never proves a hit.
const patterns = [
  'needle', '(a)?(b+)', '(?:a|)b', '(?:|a)b', '(a*)b', '(a{0})b',
  '(a{0,2})b', '(?:a{0})+b', '(?:a{0,0})b', '(a|bc)d',
  '(?=a)(a)', '(?!a)(b)', '(?<=x{3})(needle)', '(?<!x)(a)',
  '(?=(a?))\\1b', '(a)?\\1b', '\\1(a)', '(?<x>a)?\\k<x>b',
  '(?:(?<x>a)|(?<x>b))\\k<x>', '(?:(?=a)a|b)+',
  '^a', '$', 'a*', '(?:)', '[]', '[^]', '[^a]', '[a-f]+',
  '[^a-f]+', '(?:#|[0-9])z', '\\b\\w+', '\\W', '(?i:a)(?-i:b)',
];
for (const source of patterns) {
  for (const flags of ['g', 'gy', 'gu', 'guy', 'giu']) {
    for (const size of [0, 7, 8, 9, 255, 256, 257]) {
      for (const tail of ['', 'needle', 'abbb', 'bcdb', 'Aa', '12z', '\na', '\0\x7f']) {
        const subject = 'x'.repeat(size) + tail;
        for (const start of new Set([0, size, subject.length, subject.length + 1])) {
          add(source, flags, subject, start);
        }
      }
    }
  }
}

// Complement and case-equivalence ordering matters, including equivalences
// whose other members are non-ASCII. Non-ASCII subjects take the existing path.
for (const source of [
  '\\p{ASCII}', '\\P{ASCII}', '[^\\x00-\\x7f]', '\\p{L}', '\\p{N}',
  '\\p{Lowercase_Letter}', '\\P{Lowercase_Letter}', '[^\\P{Lowercase_Letter}]',
  '[\\p{L}0-9]', '[^\\p{L}0-9]', '[a-\\u212a]', '[^a-\\u212a]',
  '\\u212a', '\\u017f', '[\\u212a\\u017f]', '[^\\u212a\\u017f]',
  '\\s', '\\S', '\\w', '\\W', '\\d', '\\D',
]) {
  for (const flags of ['gu', 'giu', 'guy', 'giuy']) {
    for (const tail of ['a', 'A', 'K', 'S', '0', '\0', '\x7f', 'ſK', '😀', '\ud800']) {
      for (const size of [0, 8, 257]) {
        const subject = 'x'.repeat(size) + tail;
        for (const start of new Set([0, size, subject.length])) add(source, flags, subject, start);
      }
    }
  }
}

// Every ASCII value appears in the oracle matrix. Vary interval endpoints and
// alignment independently from the executor's word-at-a-time implementation.
for (let c = 0; c < 128; c++) {
  const escape = value => `\\x${value.toString(16).padStart(2, '0')}`;
  for (const [lo, hi] of [[c, c], [0, c], [c, 127], [Math.max(0, c - 1), Math.min(127, c + 1)]]) {
    for (const negate of ['', '^']) {
      const source = `([${negate}${escape(lo)}-${escape(hi)}])`;
      for (const flags of ['g', 'gi', 'gu', 'giu']) {
        for (const size of [7, 8, 9]) add(source, flags, 'x'.repeat(size) + String.fromCharCode(c));
      }
    }
  }
}

function encode(text) {
  const bytes = [];
  for (const ch of text) {
    const c = ch.codePointAt(0);
    if (c < 128) bytes.push(c);
    else if (c < 2048) bytes.push(0xc0 | c >> 6, 0x80 | c & 63);
    else if (c < 65536) bytes.push(0xe0 | c >> 12, 0x80 | c >> 6 & 63, 0x80 | c & 63);
    else bytes.push(0xf0 | c >> 18, 0x80 | c >> 12 & 63, 0x80 | c >> 6 & 63, 0x80 | c & 63);
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
const differences = compareAnswers(parseAnswers(expectedText), actual);
const report = {
  node: process.version, cases: cases.length, probe_args: probeArgs,
  differences: differences.length,
  first_differences: differences.slice(0, 25).map(d => ({ ...d, case: cases.find(row => row.id === d.id) })),
};
save('summary.json', JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify(report, null, 2));
process.exitCode = differences.length ? 1 : 0;
