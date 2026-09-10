#!/usr/bin/env node
// Differential test of real Perex cursors against JS string operations and
// Unicode sticky RegExp initialization. No third-party packages or engine deps.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';

const probe = process.argv[2];
assert(probe, 'usage: node tools/check-input.mjs /path/to/input_probe');
const fixtures = JSON.parse(readFileSync(new URL('../tests/fixtures/core-cases.json', import.meta.url)));
const subjects = ['', 'ascii\0\x7f', '\u0080\u07ff\u0800\uffff', 'a😀b', '\ud800x\udfff', '\ud800\ud800\udfff\udfff'];
for (const item of fixtures.cases) subjects.push(item.source, item.subject);
let seed = 0x58a4fd12;
const boundary = [0, 0x7f, 0x80, 0x7ff, 0x800, 0xd7ff, 0xd800, 0xdbff, 0xdc00, 0xdfff, 0xe000, 0xffff];
function random() {
  seed ^= seed << 13;
  seed ^= seed >>> 17;
  seed ^= seed << 5;
  return seed >>> 0;
}
for (let n = 0; n < 512; n++) {
  const units = Array.from({ length: random() % 33 }, () => random() % 2 ? random() & 0xffff : boundary[random() % boundary.length]);
  subjects.push(String.fromCharCode(...units));
}

function encodePoint(point) {
  if (point < 0x80) return [point];
  if (point < 0x800) return [0xc0 | point >> 6, 0x80 | point & 63];
  if (point < 0x10000) return [0xe0 | point >> 12, 0x80 | point >> 6 & 63, 0x80 | point & 63];
  return [0xf0 | point >> 18, 0x80 | point >> 12 & 63, 0x80 | point >> 6 & 63, 0x80 | point & 63];
}
const frames = [];
const expected = [];
const value = item => item === undefined ? '-' : item;
const normalizedStart = new RegExp('(?:)', 'duy');
for (const [index, subject] of subjects.entries()) {
  for (const combined of [true, false]) {
    const id = `${index}-${Number(combined)}`;
    const units = Array.from({ length: subject.length }, (_, at) => subject.charCodeAt(at));
    const points = combined ? Array.from(subject, ch => ch.codePointAt(0)) : units;
    const bytes = points.flatMap(encodePoint);
    frames.push(`${id}\t${Buffer.from(bytes).toString('hex')}\t${units.map(unit => unit.toString(16)).join(',')}`);
    const encodings = ['bytes', 'units'];
    // str is available precisely when this physical byte sequence has no
    // surrogate-valued code point. Noncanonical six-byte pairs are not Rust str.
    if (!points.some(point => point >= 0xd800 && point <= 0xdfff)) encodings.push('str');
    for (const encoding of encodings) {
      for (let at = 0; at <= subject.length; at++) {
        const nextUnit = at < subject.length ? subject.charCodeAt(at) : undefined;
        const previousUnit = at > 0 ? subject.charCodeAt(at - 1) : undefined;
        const nextPoint = subject.codePointAt(at);
        const previousPoint = Array.from(subject.slice(0, at)).at(-1)?.codePointAt(0);
        normalizedStart.lastIndex = at;
        const start = normalizedStart.exec(subject).index;
        expected.push([id, encoding, subject.length, at,
          value(nextUnit), Math.min(at + 1, subject.length),
          value(previousUnit), Math.max(at - 1, 0),
          value(nextPoint), at + (nextPoint === undefined ? 0 : nextPoint > 0xffff ? 2 : 1),
          value(previousPoint), at - (previousPoint === undefined ? 0 : previousPoint > 0xffff ? 2 : 1),
          start].join('\t'));
      }
    }
  }
}
const run = spawnSync(resolve(probe), [], {
  input: frames.join('\n') + '\n', encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, timeout: 60_000,
});
assert.ifError(run.error);
assert.equal(run.status, 0, run.stderr);
const actual = run.stdout.trimEnd().split('\n');
assert.equal(actual.length, expected.length, 'missing/extra cursor observations');
for (let index = 0; index < expected.length; index++) {
  assert.equal(actual[index], expected[index], `cursor observation ${index + 1}`);
}
console.log(`Perex input: ${expected.length} complete cursor observations agree with Node ${process.version} across ${frames.length} encoded inputs (${subjects.length} subjects).`);
