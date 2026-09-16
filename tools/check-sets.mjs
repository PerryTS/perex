#!/usr/bin/env node
// Complete-answer differential for Unicode-sets (`v`) character classes.
//
// The `v` grammar reserves punctuation, requires more escaping, nests classes
// and complements a set after closing it under case folding. Each of those is a
// place a new parser can accept what the grammar rejects, so syntax agreement
// is compared as carefully as matching. A member Perex reports as unsupported
// is counted on its own; a pattern it rejects that Node accepts, or accepts
// that Node rejects, is a difference.
import assert from 'node:assert/strict';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { parseAnswers, stableDifferences, boundedAnswers } from './reference.mjs';

const argv = process.argv.slice(2);
const reviewed = argv.includes('--allow-reviewed-reference-disagreements');
const [probe, output, ...probeArgs] = argv.filter(arg => arg !== '--allow-reviewed-reference-disagreements');
assert(output, 'usage: check-sets.mjs PROBE OUTPUT_DIR [--allow-reviewed-reference-disagreements] [PROBE_ARGS...]');

const R = String.raw;
const bodies = [
  // Ordinary members and ranges.
  'a', 'abc', 'a-z', 'a-zA-Z0-9', '^a-z', '^abc', '', '^',
  'z-a', 'a-', '-a', 'a-b-c',
  // Class escapes, including the complement whose folding order differs.
  R`\d`, R`\D`, R`\w`, R`\W`, R`\s`, R`\S`, R`\d\w`, R`^\d`, R`^\D`,
  R`\p{L}`, R`\P{L}`, R`\p{Ll}`, R`\P{Ll}`, R`^\p{Ll}`, R`^\P{Ll}`,
  R`\p{Script=Greek}`, R`\P{Script=Greek}`, R`\p{Lu}\p{Ll}`, R`\P{Lu}x`,
  R`\p{Basic_Emoji}`, R`\P{Basic_Emoji}`,
  // Nesting and the operators that are not implemented yet.
  '[a-z]', '[a-z][0-9]', 'a[b]c', '[[a]]', '[^a]', '[a-z]x',
  'a--b', '[a-z]--[aeiou]', 'a&&b', '[a-z]&&[aeiou]', R`[0-9]--\d`,
  // Punctuation the grammar reserves, singly and doubled.
  '&', '&&', '!', '!!', '#', '##', '%', '%%', ',', ',,', ':', '::',
  ';', ';;', '<', '<<', '=', '==', '>', '>>', '?', '??', '@', '@@',
  '~', '~~', '^^', '``', '$$', '**', '++', '..',
  // Characters that must be escaped, and their escaped forms.
  '(', ')', '{', '}', '/', '|', '-', '[', ']',
  R`\(`, R`\)`, R`\{`, R`\}`, R`\/`, R`\|`, R`\-`, R`\[`, R`\]`, R`\\`,
  R`\&`, R`\!`, R`\#`, R`\%`, R`\,`, R`\:`, R`\;`, R`\<`, R`\=`, R`\>`,
  R`\@`, R`\~`, R`\``, R`\b`, R`\q`,
  // Character escapes and astral members.
  R`\x41`, R`\x41-\x5a`, R`A`, R`\u{1f600}`, R`\u{1f600}-\u{1f610}`,
  R`\n\t\r`, R`\0`, R`\cA`, '\u{1f600}', '\u{1f600}-\u{1f610}',
  // String members: their own grammar, their order against shorter members,
  // and the operators over them.
  R`\q{abc}`, R`\q{a|b}`, R`\q{}`, R`\q{abc|d}x`, R`[\q{ab}]`,
  R`\q{a}`, R`\q{ab}`, R`\q{ab|a}`, R`\q{a|ab}`, R`\q{abc|ab}`,
  R`\q{ab}a`, R`a\q{ab}`, R`\q{ab}\q{ab}`, R`\q{ab}\q{cd}`,
  R`\q{|a}`, R`\q{a|}`, R`\q{A|B}`, R`\q{AB}`, R`\q{\u{1f600}a}`,
  R`\q{a-b}`, R`\q{\d}`, R`\q{\q{a}}`, R`\q{a`, R`\q{a}}`, R`a-\q{b}`,
  R`\q{ab}-`, R`^\q{ab}`, R`^\q{a}`, R`^[\q{ab}]`,
  R`[\q{ab}]--[\q{ab}]`, R`[\q{ab|cd}]--[\q{ab}]`, R`\q{ab}--\q{ab}`,
  R`\q{ab|cd}--\q{ab}--\q{cd}`, R`\q{a}--[a]`, R`[a-z]--\q{ab}`,
  R`[a-z]--\q{a}`, R`\q{ab}--[a-z]`, R`\q{ab}&&\q{ab|cd}`,
  R`\q{ab}&&\q{cd}`, R`\q{a}&&[a-z]`, R`[a-z]&&\q{a}`,
  R`\q{ab}&&[a-z]`, R`[\q{ab}]&&[\q{ab}]&&[a]`, R`^[\q{ab}]--[\q{ab}]`,
];

const SUBJECTS = [
  '', 'a', 'A', 'z', '0', '-', '&', '!', '~', '^', '(', ')', '|', '/',
  '\u{1f600}', 'K', 'K', 'ſ', 'é', 'α', 'Α', 'abc', 'ab', '\\', ']', '[',
];

const cases = [];
function add(source, flags, subject) {
  cases.push({ id: `sets:${cases.length}`, source, flags: `d${flags}`, subject, start_utf16: 0 });
}
for (const body of bodies) {
  for (const flags of ['v', 'iv', 'gv', 'u', 'iu']) {
    for (const subject of SUBJECTS) add(`[${body}]`, flags, subject);
  }
}
// The same complement rule applies outside a class.
for (const source of [R`\p{Ll}`, R`\P{Ll}`, R`\p{L}`, R`\P{L}`, R`\p{Basic_Emoji}`]) {
  for (const flags of ['v', 'iv', 'u', 'iu']) {
    for (const subject of SUBJECTS) add(source, flags, subject);
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

const unsupported = new Map();
const compared = new Map();
for (const [id, answer] of actual) {
  if (answer.outcome === 'unsupported') unsupported.set(id, answer);
  else compared.set(id, answer);
}
const comparable = cases.filter(row => compared.has(row.id));
const expectedSubset = new Map([...compared.keys()].map(id => [id, expected.get(id)]));
let { differences, unstable } = stableDifferences(expectedSubset, compared, comparable);
const rawDifferences = differences.length;

// Where the two engines disagree about the specification rather than about
// this implementation, the exact input and both answers are bound in a
// reviewed fixture and the development option permits only those. Anything
// new, or any listed answer that has changed on either side, still fails.
let referenceDisagreements = [];
if (reviewed) {
  referenceDisagreements = JSON.parse(
    readFileSync(new URL('../tests/fixtures/sets-reference-disagreements.json', import.meta.url)),
  );
  for (const row of referenceDisagreements) {
    assert.deepEqual(cases.find(item => item.id === row.id), row.case, 'reviewed input changed');
    assert.deepEqual(expected.get(row.id), row.node, 'Node disagreement changed');
    assert.deepEqual(actual.get(row.id), row.perex, 'Perex disagreement changed');
  }
  const ids = new Set(referenceDisagreements.map(row => row.id));
  differences = differences.filter(row => !ids.has(row.id));
}

// A pattern reported unsupported must still be a pattern the grammar accepts:
// declaring a gap on an invalid pattern would hide a missing syntax error.
const unsupportedOnInvalid = [...unsupported.keys()].filter(
  id => expected.get(id)?.outcome === 'syntax-error',
);
const unsupportedSources = new Set(
  [...unsupported.keys()].map(id => {
    const row = cases.find(c => c.id === id);
    return `/${row.source}/${row.flags}`;
  }),
);

const syntax = { bothReject: 0, perexOnly: 0, nodeOnly: 0 };
for (const [id, answer] of compared) {
  const perexBad = answer.outcome === 'syntax-error';
  const nodeBad = expected.get(id)?.outcome === 'syntax-error';
  if (perexBad && nodeBad) syntax.bothReject++;
  else if (perexBad) syntax.perexOnly++;
  else if (nodeBad) syntax.nodeOnly++;
}

const report = {
  node: process.version,
  cases: cases.length,
  cases_compared: compared.size,
  cases_unsupported: unsupported.size,
  unsupported_sources: unsupportedSources.size,
  unsupported_on_invalid_patterns: unsupportedOnInvalid.length,
  syntax,
  unstable_oracle_answers: unstable,
  raw_differences: rawDifferences,
  reviewed_reference_disagreements: referenceDisagreements.length,
  differences: differences.length,
  first_differences: differences.slice(0, 25).map(d => ({
    ...d,
    case: cases.find(row => row.id === d.id),
  })),
};
save('summary.json', JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify(report, null, 2));
process.exitCode = differences.length || unsupportedOnInvalid.length ? 1 : 0;
