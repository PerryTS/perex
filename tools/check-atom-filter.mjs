#!/usr/bin/env node
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { runCase, parseAnswers, compareAnswers } from './reference.mjs';

const [probe, output, ...probeArgs] = process.argv.slice(2);
assert(output, 'usage: check-atom-filter.mjs PROBE OUTPUT_DIR [PROBE_ARGS...]');
const cases = [];
function add(source, flags, subject, start_utf16 = 0) {
  cases.push({ id: `atom-filter:${cases.length}`, source, flags: `d${flags}`, subject, start_utf16 });
}

// Exercise both retreat directions, complete captures and nonliteral
// continuations that must retain ordinary retry order.
for (const atom of ['.', '[abxſK😀]', '[^z]']) {
  for (const q of ['*', '+', '{2,9}', '*?']) {
    for (const tail of ['ab', 'ſK', '\\ud83d', '\\ude00', '(ab)', '(?=ab)ab']) {
      for (const source of [`^(${atom}${q}${tail})`, `(?<=^(${tail}${atom}${q}))$`]) {
        for (const flags of ['g', 'gi', 'gu', 'giu']) {
          for (const subject of ['', 'ab', 'abxxxxab', 'ab'+'x'.repeat(257),
            'ſK'+'x'.repeat(257), 'sKxxxxſK', '😀xxx😀', '\ud800ab\udc00']) {
            for (const start of new Set([0, 1, subject.length, subject.length+1])) {
              add(source, flags, subject, start);
            }
          }
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
