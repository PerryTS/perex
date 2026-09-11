#!/usr/bin/env node
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { runCase, parseAnswers, compareAnswers } from './reference.mjs';

const [probe, output, ...probeArgs] = process.argv.slice(2);
assert(output, 'usage: check-classes.mjs PROBE OUTPUT_DIR [PROBE_ARGS...]');
const cases = [];
function add(source, flags, subject, start_utf16 = 0) {
  cases.push({ id: `classes:${cases.length}`, source, flags: `d${flags}`, subject, start_utf16 });
}

// Large literal-only classes exercise normalization, binary membership and
// fallback property unions, independently of the compiler's representation.
const point = c => c <= 0xffff ? `\\u${c.toString(16).padStart(4, '0')}` : String.fromCodePoint(c);
const points = [0, 9, 32, 65, 67, 75, 83, 97, 99, 107, 115, 127, 128, 0x17f,
  0x3a3, 0x3c2, 0x3c3, 0x212a, 0xd800, 0xdc00, 0xdfff, 0xffff,
  0x10000, 0x10400, 0x10428, 0x1f600, 0x10ffff];
const sets = [
  points.map(point).join(''),
  [...points].reverse().map(point).join(''),
  points.flatMap(c => [point(c), point(c)]).join(''),
  'a-zA-Z'.repeat(16),
  Array.from({length:128}, (_, i) => point(0x100 + (127-i)*3)).join(''),
  Array.from({length:64}, (_, i) => point(i*2)).join(''),
  Array.from({length:32}, (_, i) => `${point(0x100+i*3)}-${point(0x104+i*3)}`).reverse().join(''),
  points.map(point).join('') + '\\p{L}',
  points.map(point).join('') + '\\P{ASCII}',
];
for (const members of sets) for (const negate of ['', '^']) {
  const cls = `[${negate}${members}]`;
  for (const source of [cls, `(${cls}+)(${cls}?)`, `(${cls}+?)(${cls}*)`,
    `(?<=(${cls}+))(${cls})`, `(${cls})\\1`, `(?=(${cls}))\\1`,
    `(?<x>${cls})\\k<x>`, `(${cls}|(a))+`]) {
    for (const flags of ['g', 'gi', 'gu', 'giu', 'gy', 'giy', 'guy', 'giuy']) {
      for (const c of points) {
        for (const subject of [String.fromCodePoint(c).repeat(2),
          'x'.repeat(8) + String.fromCodePoint(c) + String.fromCodePoint(Math.max(0, c-1))]) {
          for (const start of [0, 1]) add(source, flags, subject, start);
        }
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
