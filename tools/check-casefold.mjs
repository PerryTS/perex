#!/usr/bin/env node
// Full answers from the real evaluator versus Node; UCD supplies test inputs,
// never the expected matching answers. Unicode generator verifies every point.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { runCase, parseAnswers, compareAnswers } from './reference.mjs';
const [probe, output] = process.argv.slice(2);
assert(probe, 'usage: check-casefold.mjs PROBE [OUTPUT_DIR]');
const data = new URL('../third_party/unicode/17.0.0/', import.meta.url);
function rows(file) {
  return readFileSync(new URL(file, data), 'utf8').split('\n')
    .map(line => line.split('#')[0].trim()).filter(Boolean)
    .map(line => line.split(';').map(f => f.trim()));
}
const pairs = new Map();
function pair(a, b) { pairs.set(`${a}:${b}`, [a, b]); }
function edge(a, b) { pair(a, b); pair(b, a); pair(a, a); pair(b, b); }
for (const f of rows('CaseFolding.txt')) {
  if (['C', 'S'].includes(f[1])) edge(parseInt(f[0], 16), parseInt(f[2], 16));
}
for (const f of rows('UnicodeData.txt')) {
  if (f[12]) edge(parseInt(f[0], 16), parseInt(f[12], 16));
}
for (const f of rows('SpecialCasing.txt')) {
  for (const value of f[3].split(' ').filter(Boolean)) edge(parseInt(f[0], 16), parseInt(value, 16));
}
for (const a of [...new Set([...pairs.values()].flat())]) if (a < 0x10ffff) pair(a, a + 1);
// Third/fourth equivalent members and deliberately non-equivalent pairs.
for (const group of [[0x49,0x69,0x130,0x131], [0x4b,0x6b,0x212a], [0x53,0x73,0x17f],
  [0x3a3,0x3c3,0x3c2], [0x3a9,0x3c9,0x2126], [0x345,0x399,0x3b9,0x1fbe],
  [0xdf,0x1e9e], [0xd800,0xdc00,0xdfff,0xfffd], [0x10400,0x10428]]) {
  for (const a of group) for (const b of group) pair(a, b);
}
const cases = [];
function add(source, flags, subject) {
  cases.push({ id: `casefold:${cases.length}`, source, flags: `d${flags}`, subject, start_utf16: 0 });
}
function escape(c) {
  return [...String.fromCodePoint(c)].flatMap(ch => {
    const out = [];
    for (let i=0;i<ch.length;i++) out.push(`\\u${ch.charCodeAt(i).toString(16).padStart(4,'0')}`);
    return out;
  }).join('');
}
for (const [a,b] of pairs.values()) for (const flags of ['i', 'iu']) {
  const atom = escape(a), left = String.fromCodePoint(a), right = String.fromCodePoint(b);
  add(`^(${atom})$`, flags, right);
  add(`^([${atom}])$`, flags, right);
  add(`^([^${atom}])$`, flags, right);
  add(`^(${atom})\\1$`, flags, left+right);
  add(`(?<=^\\1(${atom}))$`, flags, right+left);
}
const patterns = ['[a-z]+','[^a-z]+','[A-Z]+','[a-Z]','[A-z]+','[s-ſ]+',
  '[Σ-Ω]+','[^Σ-Ω]+','[\\w]+','[\\W]+','[^\\w]+','[^\\W]+','[\\w\\W]+','[a\\W]+',
  '\\w+','\\W+','\\b\\w+\\b','^\\B','\\B$','(s|k)+','(?<=(s))k\\1','[\\D]+',
  '[\\S]+','(ß)\\1','[\\u{10400}-\\u{10427}]+','(.)\\1','(?<=\\1(.))$',
  '(s|(k))+\\1','(s|k)*?S','(?=(s+))\\1K','^(?:ss|SS)$'];
const subjects = ['','SkſK','ſK','Kſ','Σσς','ωΩΩ','ßẞ','ss','ſkS','ſKs','ιΙιͅ','İiıI',
  '𐐀𐐨','A𐐨B','a😀b','\ud800','\udfff','\ud800\udfff','\u0000','12 !','\n\u2028'];
for (const source of patterns) for (const subject of subjects) for (const flags of ['', 'i', 'u', 'iu']) add(source, flags, subject);
function encode(text) {
  const bytes=[];
  for (const ch of text) {
    const p=ch.codePointAt(0);
    if(p<128) bytes.push(p);
    else if(p<2048) bytes.push(0xc0|p>>6,0x80|p&63);
    else if(p<65536) bytes.push(0xe0|p>>12,0x80|p>>6&63,0x80|p&63);
    else bytes.push(0xf0|p>>18,0x80|p>>12&63,0x80|p>>6&63,0x80|p&63);
  }
  return Buffer.from(bytes).toString('hex');
}
const expectedText = cases.map(row => JSON.stringify(runCase(row))).join('\n')+'\n';
const run = spawnSync(resolve(probe), [], {
  input: cases.map(row => [row.id,encode(row.source),row.flags,encode(row.subject),0].join('\t')).join('\n')+'\n',
  encoding:'utf8', maxBuffer:256*1024*1024, timeout:300_000,
});
if(output) {
  mkdirSync(output,{recursive:true});
  writeFileSync(resolve(output,'cases.json'),JSON.stringify({cases}));
  writeFileSync(resolve(output,'expected.jsonl'),expectedText);
  writeFileSync(resolve(output,'actual.jsonl'),run.stdout ?? '');
  writeFileSync(resolve(output,'stderr.txt'),run.stderr ?? '');
}
assert.ifError(run.error); assert.equal(run.status,0,run.stderr);
const differences=compareAnswers(parseAnswers(expectedText),parseAnswers(run.stdout));
const report={node:process.version,unicode:process.versions.unicode,pairs:pairs.size,cases:cases.length,
  differences:differences.length,first_differences:differences.slice(0,20)};
if(output) writeFileSync(resolve(output,'summary.json'),JSON.stringify(report,null,2)+'\n');
console.log(JSON.stringify(report,null,2));
process.exitCode=differences.length?1:0;
