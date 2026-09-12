import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { isDeepStrictEqual } from 'node:util';

const fixturePath = new URL('../tests/fixtures/core-cases.json', import.meta.url);
const expectedPath = new URL('../tests/fixtures/core-expected.jsonl', import.meta.url);
const receiptPath = new URL('../tests/fixtures/reference.json', import.meta.url);
const hash = text => createHash('sha256').update(text).digest('hex');

function units(text) {
  const out = [];
  for (let i = 0; i < text.length; i++) out.push(text.charCodeAt(i));
  return out;
}

export function parseAnswers(text) {
  const answers = new Map();
  for (const line of text.split('\n')) {
    if (!line.trim()) continue;
    const answer = JSON.parse(line);
    if (!answer || typeof answer.id !== 'string' || !answer.id) {
      throw new Error('Every answer needs a nonempty string id');
    }
    if (answers.has(answer.id)) throw new Error(`Duplicate answer: ${answer.id}`);
    answers.set(answer.id, answer);
  }
  if (!answers.size) throw new Error('Empty answer set');
  return answers;
}

export function compareAnswers(expected, actual) {
  const differences = [];
  for (const [id, value] of expected) {
    if (!actual.has(id)) differences.push({ id, reason: 'missing' });
    else if (!isDeepStrictEqual(value, actual.get(id))) {
      differences.push({ id, reason: 'different', expected: value, actual: actual.get(id) });
    }
  }
  for (const id of actual.keys()) {
    if (!expected.has(id)) differences.push({ id, reason: 'extra' });
  }
  return differences;
}

export function runCase(row) {
  let re;
  try {
    re = new RegExp(row.source, row.flags);
  } catch (error) {
    if (!(error instanceof SyntaxError)) throw error;
    return { id: row.id, outcome: 'syntax-error' };
  }
  if (!re.hasIndices) throw new Error(`${row.id}: accepted patterns need d for capture spans`);
  if (row.start_utf16 && !re.global && !re.sticky) {
    throw new Error(`${row.id}: nonzero start needs g or y in the reference adapter`);
  }
  re.lastIndex = row.start_utf16;
  const match = re.exec(row.subject);
  if (match === null) return { id: row.id, outcome: 'no-match' };
  const captures = Array.from(match, (text, i) => text === undefined ? null : {
    span: Array.from(match.indices[i]),
    units: units(text),
  });
  const groups = match.groups === undefined ? null : Object.keys(match.groups).sort().map(name => ({
    name,
    capture: match.groups[name] === undefined ? null : {
      span: Array.from(match.indices.groups[name]),
      units: units(match.groups[name]),
    },
  }));
  return { id: row.id, outcome: 'match', captures, groups };
}

function loadFixture() {
  const text = readFileSync(fixturePath, 'utf8');
  const fixture = JSON.parse(text);
  if (fixture.schema_version !== 1 || !Array.isArray(fixture.cases) || !fixture.cases.length) {
    throw new Error('Invalid or empty fixture');
  }
  const ids = new Set();
  for (const row of fixture.cases) {
    if (!row || typeof row.id !== 'string' || !row.id || ids.has(row.id) ||
        typeof row.source !== 'string' || typeof row.flags !== 'string' ||
        typeof row.subject !== 'string' || !Number.isSafeInteger(row.start_utf16) || row.start_utf16 < 0) {
      throw new Error('Invalid or duplicate fixture case');
    }
    ids.add(row.id);
  }
  return { text, fixture, ids };
}

function readExpected(input) {
  const text = readFileSync(expectedPath, 'utf8');
  const receipt = JSON.parse(readFileSync(receiptPath, 'utf8'));
  if (receipt.fixture_sha256 !== hash(input.text) || receipt.answers_sha256 !== hash(text)) {
    throw new Error('Fixture/reference hashes differ from the committed receipt');
  }
  const expected = parseAnswers(text);
  if (expected.size !== input.ids.size || [...input.ids].some(id => !expected.has(id))) {
    throw new Error('Expected answer coverage differs from the fixture');
  }
  return expected;
}

// The oracle's answers can depend on accumulated V8 state: a pattern that
// matches in a clean process has been observed to report no match after a large
// unrelated regexp workload in the same process. Recomputing a small set of
// cases in a fresh process removes that measurement artifact. It never changes
// or hides an answer; a difference that survives re-verification is reported.
export function freshAnswers(rows) {
  if (!rows.length) return new Map();
  const child = spawnSync(process.execPath, [fileURLToPath(import.meta.url), '--answers'], {
    input: JSON.stringify(rows),
    encoding: 'utf8',
    maxBuffer: 256 * 1024 * 1024,
    timeout: 300_000,
  });
  if (child.error) throw child.error;
  if (child.status !== 0) throw new Error(`reference --answers failed: ${child.stderr}`);
  return parseAnswers(child.stdout);
}

// Compare, then recompute only the disagreeing cases in a clean process and
// compare those again. A difference that survives is reported unchanged; the
// number whose oracle answer was unstable is published alongside it.
export function stableDifferences(expected, actual, cases) {
  const first = compareAnswers(expected, actual);
  if (!first.length) return { differences: first, unstable: 0 };
  const byId = new Map(cases.map(row => [row.id, row]));
  const rows = first.map(d => byId.get(d.id));
  if (rows.some(row => row === undefined)) return { differences: first, unstable: 0 };
  const fresh = freshAnswers(rows);
  const subset = new Map(
    [...fresh.keys()].filter(id => actual.has(id)).map(id => [id, actual.get(id)]),
  );
  const persistent = compareAnswers(fresh, subset);
  return { differences: persistent, unstable: first.length - persistent.length };
}

export function main(args = process.argv.slice(2)) {
  const mode = args[0] ?? '--check';
  if (mode === '--answers') {
    const rows = JSON.parse(readFileSync(0, 'utf8'));
    process.stdout.write(rows.map(row => JSON.stringify(runCase(row))).join('\n') + '\n');
    return 0;
  }
  if (!['--emit', '--write', '--check', '--compare'].includes(mode) ||
      args.length > (mode === '--compare' ? 2 : 1) || (mode === '--compare' && !args[1])) {
    throw new Error('Usage: reference.mjs [--check | --emit | --write | --compare FILE]');
  }
  const input = loadFixture();
  if (mode === '--compare') {
    const differences = compareAnswers(readExpected(input), parseAnswers(readFileSync(args[1], 'utf8')));
    process.stdout.write(JSON.stringify({ cases: input.ids.size, differences }, null, 2) + '\n');
    return differences.length ? 1 : 0;
  }
  const output = input.fixture.cases.map(row => JSON.stringify(runCase(row))).join('\n') + '\n';
  if (mode === '--emit') {
    process.stdout.write(output);
    return 0;
  }
  if (mode === '--write') {
    writeFileSync(expectedPath, output);
    writeFileSync(receiptPath, JSON.stringify({
      schema_version: 1,
      scope: 'Node semantic reference only; Perex matching is not implemented.',
      node: process.version,
      v8: process.versions.v8,
      unicode: process.versions.unicode,
      fixture_sha256: hash(input.text),
      answers_sha256: hash(output),
      cases: input.ids.size,
    }, null, 2) + '\n');
    process.stdout.write(`Wrote ${input.ids.size} reference answers using ${process.version}\n`);
    return 0;
  }
  const differences = compareAnswers(readExpected(input), parseAnswers(output));
  process.stdout.write(JSON.stringify({ node: process.version, cases: input.ids.size, differences }, null, 2) + '\n');
  return differences.length ? 1 : 0;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { process.exitCode = main(); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
