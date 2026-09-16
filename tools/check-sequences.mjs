#!/usr/bin/env node
// Complete-answer differential for the properties of strings.
//
// Their members are sequences of code points, so nothing about them is visible
// in a class table: the engine matches them from shared data, longest member
// first. This check derives the members from the pinned Unicode input rather
// than from the generated tables, so a generator mistake is a difference here
// rather than a shared assumption, and compares every member against every set
// — each set should hold its own members, `RGI_Emoji` should hold all of them,
// and no set should hold another's.
//
// usage: check-sequences.mjs PROBE OUTPUT_DIR [PROBE_ARGS...]
import assert from 'node:assert/strict';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { parseAnswers, stableDifferences, boundedAnswers } from './reference.mjs';

const [probe, output, ...probeArgs] = process.argv.slice(2);
assert(output, 'usage: check-sequences.mjs PROBE OUTPUT_DIR [PROBE_ARGS...]');

const data = new URL('../third_party/unicode/17.0.0/emoji/', import.meta.url);
const SETS = ['Basic_Emoji', 'Emoji_Keycap_Sequence', 'RGI_Emoji', 'RGI_Emoji_Flag_Sequence',
  'RGI_Emoji_Modifier_Sequence', 'RGI_Emoji_Tag_Sequence', 'RGI_Emoji_ZWJ_Sequence'];

const members = new Map(SETS.map(name => [name, []]));
const points = new Map(SETS.map(name => [name, []]));
for (const file of ['emoji-sequences.txt', 'emoji-zwj-sequences.txt']) {
  for (const line of readFileSync(new URL(file, data), 'utf8').split('\n')) {
    const body = line.split('#')[0].trim();
    if (!body) continue;
    const [codes, property] = body.split(';').map(part => part.trim());
    assert(members.has(property), `unexpected property ${property}`);
    const values = codes.split(/\s+/);
    if (values.length === 1) {
      const [lo, hi] = values[0].split('..');
      for (let c = parseInt(lo, 16); c <= parseInt(hi ?? lo, 16); c++) points.get(property).push(c);
    } else {
      members.get(property).push(values.map(c => String.fromCodePoint(parseInt(c, 16))).join(''));
    }
  }
}
for (const name of SETS) {
  if (name === 'RGI_Emoji') continue;
  members.get('RGI_Emoji').push(...members.get(name));
  points.get('RGI_Emoji').push(...points.get(name));
}

const cases = [];
const add = (source, flags, subject) =>
  cases.push({ id: `sequences:${cases.length}`, source, flags: `d${flags}`, subject, start_utf16: 0 });

// Every member of every set, against every set: anchored, and inside text so
// the search reaches it from a start that is not the match.
const all = [...new Set(members.get('RGI_Emoji'))];
for (const name of SETS) {
  for (const member of all) {
    add(`^\\p{${name}}$`, 'v', member);
    add(`\\p{${name}}`, 'v', `x${member}y`);
  }
  // The members of one code point, which are `Emoji_Presentation` without the
  // regional indicators, and a regional indicator that is not one.
  for (const c of [...new Set(points.get(name))].slice(0, 64)) {
    add(`^\\p{${name}}$`, 'v', String.fromCodePoint(c));
  }
  add(`^\\p{${name}}$`, 'v', '\u{1f1e6}');
  // Shapes around a property of strings: unions, operators and quantifiers.
  for (const [source, subject] of [
    [`[\\p{${name}}\\d]`, '5'], [`[\\p{${name}}\\d]`, all[0]],
    [`^[\\p{${name}}--\\q{${all[0]}}]$`, all[0]], [`^[\\p{${name}}--\\q{${all[0]}}]$`, all[1]],
    [`^[\\p{${name}}&&\\p{RGI_Emoji}]$`, all[0]], [`^[\\p{${name}}--\\p{RGI_Emoji}]$`, all[0]],
    [`^[\\p{${name}}&&\\p{Basic_Emoji}]$`, all[0]], [`^\\p{${name}}+$`, all[0] + all[1]],
    [`^\\p{${name}}{2}$`, all[0].repeat(2)], [`(?<=\\p{${name}})x`, `${all[0]}x`],
    [`^[\\p{${name}}\\q{ab}]$`, 'ab'], [`^[^\\p{Emoji_Presentation}]$`, all[0]],
  ]) add(source, 'v', subject);
  for (const flags of ['iv', 'gv', 'u']) add(`\\p{${name}}`, flags, all[0]);
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
const input = cases
  .map(row => [row.id, encode(row.source), row.flags, encode(row.subject), row.start_utf16].join('\t'))
  .join('\n') + '\n';
save('cases.json', JSON.stringify({ cases }));
save('expected.jsonl', cases.map(row => JSON.stringify(expected.get(row.id))).join('\n') + '\n');
save('input.tsv', input);

const run = spawnSync(resolve(probe), probeArgs, {
  input, encoding: 'utf8', maxBuffer: 256 * 1024 * 1024, timeout: 900_000,
});
save('actual.jsonl', run.stdout ?? '');
save('stderr.txt', run.stderr ?? '');
assert.ifError(run.error);
assert.equal(run.status, 0, run.stderr);

const actual = parseAnswers(run.stdout);
assert.equal(actual.size, cases.length);
const unsupported = [...actual.values()].filter(row => row.outcome === 'unsupported');
const { differences, unstable } = stableDifferences(expected, actual, cases);

const report = {
  node: process.version,
  sets: SETS.length,
  members: all.length,
  cases: cases.length,
  cases_unsupported: unsupported.length,
  unstable_oracle_answers: unstable,
  differences: differences.length,
  first_differences: differences.slice(0, 25).map(d => ({
    ...d,
    case: cases.find(row => row.id === d.id),
  })),
};
save('summary.json', JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify(report, null, 2));
process.exitCode = differences.length || unsupported.length ? 1 : 0;
