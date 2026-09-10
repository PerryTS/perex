import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { compareAnswers, parseAnswers } from './reference.mjs';

const expectedText = readFileSync(new URL('../tests/fixtures/core-expected.jsonl', import.meta.url), 'utf8');
const expected = parseAnswers(expectedText);

test('comparison accepts reordered answers and object properties', () => {
  const actual = new Map([...expected].reverse().map(([id, row]) => [id, Object.fromEntries(Object.entries(row).reverse())]));
  assert.deepEqual(compareAnswers(expected, actual), []);
});

test('comparison catches a wrong code unit in a half-surrogate capture', () => {
  const actual = parseAnswers(expectedText);
  const row = actual.get('dot-nonu-astral');
  assert.equal(row.captures[0].units[0], 0xd83d);
  row.captures[0].units[0] = 0xfffd;
  assert.deepEqual(compareAnswers(expected, actual).map(d => [d.id, d.reason]), [['dot-nonu-astral', 'different']]);
});

test('comparison catches missing and extra cases', () => {
  const actual = parseAnswers(expectedText);
  actual.delete('sticky-low-u');
  actual.set('invented', { id: 'invented', outcome: 'no-match' });
  assert.deepEqual(compareAnswers(expected, actual).map(d => [d.id, d.reason]), [['sticky-low-u', 'missing'], ['invented', 'extra']]);
});

test('duplicate and empty answer streams are rejected', () => {
  const first = expectedText.split('\n')[0];
  assert.throws(() => parseAnswers(first + '\n' + first), /Duplicate answer/);
  assert.throws(() => parseAnswers('\n'), /Empty answer/);
});
