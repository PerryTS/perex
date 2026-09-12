#!/usr/bin/env node
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { parseAnswers, compareAnswers ,stableDifferences} from './reference.mjs';

const [probe, output, ...probeArgs] = process.argv.slice(2);
assert(output, 'usage: check-end-candidate.mjs PROBE OUTPUT_DIR [PROBE_ARGS...]');
assert.equal(/a$/.test('a\n'), false);
assert.equal(/a$/m.test('a\n'), true);
const cases = [];
function add(source, flags, subject, start_utf16 = 0) {
  cases.push({ id: `end-candidate:${cases.length}`, source, flags: `d${flags}`, subject, start_utf16 });
}
const patterns = ["^(a|aa)+b$", "^(a+)+$", "(a+)$", "(a)?$", "(?:a$|b$)", "(?:a$|b)", "a$(?:b?)", "a$(?:)", "a$(?=)", "a$(?!b)", "(?:a$)+", "(?:a$)*", "a{0}$", "(?:a{0})+$", "a(b)?$", "(a)?b$", "(a)?\\1$", "(?<x>a)?\\k<x>$", "(?=(a))\\1$", "(?<=a)(b)$", "(?m:a$)", "(?-m:a$)", "(?i:\u212a)$", "(?i:[\u212a\u017f])$", "[^a-z]$", "[]$", "[^]$", "[^\\x00-\\x7f]$", "(?:a$|)$", "(?:a|)$", "\\uDC00$", "\\uD800$", "(\ud83d\ude00)$", "(a?){2}$", "(?:a$|bc$)(?=)", "(?<=a$)", "(?!(a$))b", "(a$(?m:b?))", "(?m:a$)(?-m:b?)"];
const subjects = ["aaaaaa", "", "a", "b", "aa", "ab", "aab", "aa!", "a\n", "a\r\n", "a\r", "a\u2028", "a\u2029", "A", "K", "\u017f", "\u212a", "\ud83d\ude00", "a\ud83d\ude00", "\ud800", "\udc00", "a\ud800", "\ud800\udc00"];
for (const source of patterns) {
  for (const flags of ['g', 'gm', 'gi', 'gu', 'gui', 'gy', 'guy']) {
    for (const subject of subjects) {
      for (const start of new Set([0, 1, subject.length - 1, subject.length, subject.length + 1])) {
        if (start >= 0) add(source, flags, subject, start);
      }
    }
  }
}
// Independently exercise every ASCII member at interval and complement edges.
for (let c = 0; c < 128; c++) {
  const hex = n => `\\x${n.toString(16).padStart(2, '0')}`;
  for (const source of [`(${hex(c)})$`, `([${hex(0)}-${hex(c)}])$`,
    `([^${hex(c)}-${hex(127)}])$`]) {
    for (const flags of ['g', 'gi', 'gu', 'gui']) {
      for (const value of new Set([Math.max(0, c - 1), c, Math.min(127, c + 1)])) {
        add(source, flags, 'é😀' + String.fromCharCode(value));
      }
    }
  }
}
// Keep rejecting oracle inputs small; the forty-character host failure is
// covered by the explicitly work-bounded Rust witness. A longer successful
// match still compares its nested capture against Node.
for (const flags of ['g', 'gu', 'gi', 'gy']) {
  for (const subject of ['a'.repeat(8) + '!', 'a'.repeat(40), 'a'.repeat(8) + '\n']) {
    add('^(a+)+$', flags, subject);
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
save('cases.json', JSON.stringify({ cases }));
// Isolate the reference as well as the candidate so neither can strand CI.
const oracleCode = `import { readFileSync } from 'node:fs';
import { runCase } from ${JSON.stringify(new URL('./reference.mjs', import.meta.url).href)};
for (const row of JSON.parse(readFileSync(0, 'utf8'))) {
  process.stdout.write(JSON.stringify(runCase(row)) + '\\n');
}`;
const oracleCommand = [process.execPath, '--input-type=module', '-e', oracleCode];
save('oracle-command.json', JSON.stringify(oracleCommand) + '\n');
const oracle = spawnSync(oracleCommand[0], oracleCommand.slice(1), {
  input: JSON.stringify(cases), encoding: 'utf8', maxBuffer: 256 * 1024 * 1024,
  timeout: 30_000,
});
save('expected.jsonl', oracle.stdout ?? '');
save('oracle-stderr.txt', oracle.stderr ?? '');
save('oracle-exit.json', JSON.stringify({ status: oracle.status, signal: oracle.signal,
  error: oracle.error?.message }) + '\n');
assert.ifError(oracle.error);
assert.equal(oracle.status, 0, oracle.stderr);
const expectedText = oracle.stdout;
assert.equal(parseAnswers(expectedText).size, cases.length);
const input = cases.map(row => [row.id, encode(row.source), row.flags, encode(row.subject), row.start_utf16].join('\t')).join('\n') + '\n';
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
const {differences,unstable}=stableDifferences(parseAnswers(expectedText),actual,cases);
const report = {
  node: process.version, cases: cases.length, probe_args: probeArgs,
  unstable_oracle_answers: unstable,
  differences: differences.length,
  first_differences: differences.slice(0, 25).map(d => ({ ...d, case: cases.find(row => row.id === d.id) })),
};
save('summary.json', JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify(report, null, 2));
process.exitCode = differences.length ? 1 : 0;
