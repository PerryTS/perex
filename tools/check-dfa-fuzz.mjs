#!/usr/bin/env node
// Random patterns the lazy automaton accepts, against Node. The probe runs
// with PEREX_DFA set (default `1`), so every eligible case is answered by the
// automaton as well and any disagreement with the evaluator is an outcome of
// its own; the answers are compared with V8's. See docs/dfa.md.
//
// usage: check-dfa-fuzz.mjs PROBE OUTPUT_DIR [--seed N] [--cases N]
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { boundedAnswers, parseAnswers, stableDifferences } from './reference.mjs';

const args = process.argv.slice(2);
const probe = args.shift();
const output = args.shift();
assert(probe && output, 'usage: check-dfa-fuzz.mjs PROBE OUTPUT_DIR [--seed N] [--cases N]');
const option = (name, fallback) => {
  const at = args.indexOf(name);
  return at < 0 ? fallback : Number(args[at + 1]);
};
const seed = option('--seed', 1);
const total = option('--cases', 20000);

let state = seed >>> 0 || 1;
function random(n) {
  state = (state + 0x6d2b79f5) >>> 0;
  let t = state;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return (((t ^ (t >>> 14)) >>> 0) % n);
}
const pick = list => list[random(list.length)];

// Characters chosen for the corners: case pairs, the two non-ASCII letters that
// fold into ASCII under `u`, line terminators, word boundaries, a pair and its
// halves, and the start of a property.
const characters = ['a', 'b', 'c', 'A', 'B', 'k', 'K', 's', 'S', 'ſ', 'K', 'é', 'É', 'ß',
  '_', '0', '9', ' ', '\n', '\r', ' ', '😀', '😂', '\ud83d', '\ude00', '­', '​', 'Ω', 'ω'];
const literal = ['a', 'b', 'c', 'A', 'k', 's', 'é', 'ß', '_', '0', ' ', '\\n', '\\u017f', '\\u212a',
  '😀', '\\ud83d', '\\ude00', 'ω', '\\.', '\\u{1F600}'];
const classes = ['.', '\\w', '\\W', '\\d', '\\D', '\\s', '\\S', '[ab]', '[^a-c]', '[a-zé]', '[\\w-]',
  '[k-s]', '[A-Z]', '[^\\s]', '[😀-😂]', '[\\ud83d\\ude00]', '[^😀]', '[\\u017f\\u212a]', '[ω-ϊ]'];
const unicodeOnly = ['\\p{L}', '\\p{Lu}', '\\P{Ll}', '\\p{Emoji}', '\\p{Default_Ignorable_Code_Point}',
  '\\p{Script=Greek}', '[\\p{Lu}\\d]', '[^\\p{L}]', '\\P{Any}'];
const assertions = ['^', '$', '\\b', '\\B'];
const quantifiers = ['*', '+', '?', '*?', '+?', '??', '{2}', '{1,3}', '{0,2}?', '{2,}', '{0}', '{3,4}?'];

function atom(depth, unicode) {
  const roll = random(20);
  if (roll < 6) return pick(literal);
  if (roll < 11) return pick(classes);
  if (roll < 13) return unicode ? pick(unicodeOnly) : pick(classes);
  if (roll < 15) return pick(assertions);
  if (depth < 3) {
    const open = pick(['(', '(?:', '(?:', '(?<n' + random(1000) + '>']);
    return open + alternation(depth + 1, unicode) + ')';
  }
  return pick(literal);
}

function sequence(depth, unicode) {
  let text = '';
  for (let i = random(4) + (depth ? 0 : 1); i > 0; i--) {
    const a = atom(depth, unicode);
    text += a;
    if (!assertions.includes(a) && random(3) === 0) text += pick(quantifiers);
  }
  return text;
}

function alternation(depth, unicode) {
  const parts = [sequence(depth, unicode)];
  while (random(3) === 0) parts.push(sequence(depth, unicode));
  return parts.join('|');
}

// Most subjects are short; one in ten is long enough for the steady path's
// skip to a byte a match can begin with.
function subject() {
  let text = '';
  for (let i = random(10) === 0 ? 40 + random(60) : random(14); i > 0; i--) text += pick(characters);
  return text;
}

const cases = [];
for (let n = 0; n < total; n++) {
  const unicode = random(2) === 0;
  const flags = ['d', unicode ? 'u' : '', ...['i', 'm', 's'].filter(() => random(3) === 0)];
  const start = random(3);
  if (start === 1) flags.push('g');
  if (start === 2) flags.push('y');
  const source = alternation(0, unicode);
  const text = subject();
  cases.push({
    id: `dfa-fuzz:${seed}:${n}`,
    source,
    flags: flags.join(''),
    subject: text,
    start_utf16: start === 0 ? 0 : random(text.length + 2),
  });
}

function encode(text) {
  const bytes = [];
  for (let i = 0; i < text.length; i++) {
    let p = text.charCodeAt(i);
    if (p >= 0xd800 && p < 0xdc00 && i + 1 < text.length) {
      const next = text.charCodeAt(i + 1);
      if (next >= 0xdc00 && next < 0xe000) {
        p = 0x10000 + ((p - 0xd800) << 10) + (next - 0xdc00);
        i++;
      }
    }
    if (p < 0x80) bytes.push(p);
    else if (p < 0x800) bytes.push(0xc0 | p >> 6, 0x80 | p & 63);
    else if (p < 0x10000) bytes.push(0xe0 | p >> 12, 0x80 | p >> 6 & 63, 0x80 | p & 63);
    else bytes.push(0xf0 | p >> 18, 0x80 | p >> 12 & 63, 0x80 | p >> 6 & 63, 0x80 | p & 63);
  }
  return Buffer.from(bytes).toString('hex');
}

const { answers: expected, timedOut } = boundedAnswers(cases);
const run = spawnSync(resolve(probe), [], {
  input: cases.map(row => [row.id, encode(row.source), row.flags, encode(row.subject), row.start_utf16].join('\t')).join('\n') + '\n',
  encoding: 'utf8',
  maxBuffer: 512 * 1024 * 1024,
  timeout: 600_000,
  env: { ...process.env, PEREX_DFA: process.env.PEREX_DFA ?? '1' },
});
assert.ifError(run.error);
assert.equal(run.status, 0, run.stderr);
const actual = parseAnswers(run.stdout);
// A dfa-mismatch is Perex against itself; a Node timeout does not void it.
const mismatches = [...actual.values()].filter(answer => answer.outcome === 'dfa-mismatch');
for (const row of timedOut) actual.delete(row.id), expected.delete(row.id);
const { differences: all, unstable } = stableDifferences(expected, actual, cases);
// The reviewed Node disagreement in docs/reference-disagreements.md: under `u`,
// V8 lets `\B` match between the halves of a surrogate pair. Perex's
// evaluator does not, and the automaton agrees with the evaluator, so such a
// case differs from Node for a reason already on record. Only a difference
// whose Node match begins between two halves, in a `u` pattern holding `\B`,
// is set aside; every other difference fails.
const byId = new Map(cases.map(row => [row.id, row]));
const betweenHalves = (text, at) => at > 0 && at < text.length &&
  /[\ud800-\udbff]/.test(text[at - 1]) && /[\udc00-\udfff]/.test(text[at]);
const reviewed = all.filter(d => {
  const row = byId.get(d.id);
  const spans = (d.expected?.captures ?? []).filter(Boolean).map(c => c.span).flat();
  return row.flags.includes('u') && row.source.includes('\\B') && d.expected?.outcome === 'match' &&
    spans.some(at => betweenHalves(row.subject, at));
});
// The evaluator can exhaust the probe's allowance on a pathological pattern.
// The automaton then answers no-match itself, but a match still needs the
// evaluator for its captures, so such a case stays an execution error. It is
// reported, not counted as a wrong answer.
const exhausted = all.filter(d => !reviewed.includes(d) &&
  d.actual?.outcome === 'execution-error' && d.actual?.error === 'WorkLimit');
const differences = all.filter(d => !reviewed.includes(d) && !exhausted.includes(d));
const outcomes = {};
for (const answer of actual.values()) outcomes[answer.outcome] = (outcomes[answer.outcome] ?? 0) + 1;
const report = {
  node: process.version,
  seed,
  cases: cases.length,
  dfa: process.env.PEREX_DFA ?? '1',
  probe: run.stderr.trim().split('\n').pop(),
  outcomes,
  reference_timeouts: timedOut.length,
  unstable_reference_answers: unstable,
  dfa_mismatches: mismatches.length,
  reviewed_word_boundary_disagreements: reviewed.length,
  evaluator_work_limit: exhausted.map(d => d.id),
  differences: differences.length,
  first_differences: differences.slice(0, 20).map(d => ({ ...d, case: cases.find(row => row.id === d.id) })),
};
mkdirSync(output, { recursive: true });
writeFileSync(resolve(output, 'summary.json'), JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify(report, null, 2));
process.exitCode = differences.length || mismatches.length ? 1 : 0;
