#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { runCase, parseAnswers, compareAnswers } from './reference.mjs';
const args=process.argv.slice(2);
const probe=args.shift();
assert(probe,'usage: check-engine.mjs PROBE [--allow-listed-unsupported] [--output DIR]');
const reviewed=args.includes('--allow-reviewed-reference-disagreements');
const allow=args.includes('--allow-listed-unsupported');
const outIndex=args.indexOf('--output');
const output=outIndex<0 ? null : args[outIndex+1];
assert(outIndex<0 || output,'--output needs a directory');
const core=JSON.parse(readFileSync(new URL('../tests/fixtures/core-cases.json',import.meta.url))).cases;
const cases=[...core];
const patterns=['','a','ab','a|ab','ab|a','(ab|a)b','(a|(b))+','(a*)*','(a*)+','a{2,4}?a','a{2,4}a','a*?b','a{0}','(a?){2,4}',
 '(a+)?(b*)','((a)|(b))*','(a(b)?)+','(a|ab)*?b','(?:a|)*','(?:|a)*','(a*)*b','a{3,}','^a$','^$','$','^','.', '[^]','[]',
 '[a-c-]+','[^ab]*','[\\d-a]+','[a-\\d]+','[\\d-]','[\\D]+','[\\S\\s]','\\b\\w+\\s\\d+\\b','\\B','\\x61','\\u0061','\\u{61}',
 '\\ud83d','\\ud83d\\ude00','\\u{d83d}\\ude00','\\ud83d\\u{de00}','😀','[😀]','[\\ud83d\\ude00]','[a-z0-9_]+',
 '(?=(a+))a*b\\1','(?!a(b))a(c)','(?<=([ab]+)([bc]+))$','(?<!ab)c','(a)?b\\1','\\1(a)','(a\\1)','(a|b)\\1',
 '(?=(a|ab))\\1b','(?!(a+)+b)c','(?<=(a+))b\\1','(?<=(a)\\1)b','(?<=\\1(a))b','(?=(?!(a))b)b','(?!(?=(a))b)a',
 '(?:(?=(a))a|b)+','(?:(a)|(?!(b))c)+','(?:(?<=a)b|a)+','a??a','(a??)*','(?:a{0})+','(a{0}){2}',
 '(', '[a', '[z-a]', '*a', 'a{3,2}', 'a**', '\\', 'a)', '[a-]', '[--0]'];
const subjects=['','a','b','ab','aba','aaaaa','aaab','baabac','abc','abc c','aabaa','ac','z-abcc','a 12!','\n','x\r\ny','\u2028','😀','a😀b','\ud83d','\ude00','\ud800\ud800\udfff','\0','ſK'];
for (const source of patterns) for (const subject of subjects) for (const mode of ['','u','m','s']) {
  cases.push({id:`matrix:${cases.length}`,source,flags:`d${mode}`,subject,start_utf16:0});
}
for (const source of ['.','(.)','[^]','(?<=.)','(?:)','\\ud83d','\\ude00','😀','a|b']) {
  for (const subject of ['😀','a😀b','\ud800\ud800\udfff','']) for (let start=0;start<=subject.length+1;start++) for (const mode of ['gy','uy','g','ug']) {
    cases.push({id:`start:${cases.length}`,source,flags:`d${mode}`,subject,start_utf16:start});
  }
}
// Bounded generated compositions exercise interactions that a flat matrix
// misses. Keep generation deterministic and inputs short enough for the oracle.
let seed=0x7bc351d2;
function random(n) { seed^=seed<<13; seed^=seed>>>17; seed^=seed<<5; return (seed>>>0)%n; }
function pattern(depth) {
  const atoms=['a','b','.','[ab]','[^b]','\\d','(?:)','😀'];
  if(!depth) return atoms[random(atoms.length)];
  const child=()=>pattern(depth-1);
  switch(random(9)) {
    case 0: return `(?:${child()}|${child()})`;
    case 1: return `(${child()})`;
    case 2: return `(?:${child()})${['*','+','?','*?','{0,2}','{1,3}?'][random(6)]}`;
    case 3: return `(?=${child()})${child()}`;
    case 4: return `(?!${child()})${child()}`;
    case 5: return `(?<=${child()})${child()}`;
    case 6: return `(?<!${child()})${child()}`;
    default: return child()+child();
  }
}
for(let n=0;n<512;n++) {
  let source=pattern(3);
  if(n%4===0) source=`(a)?${source}\\1`;
  for(const subject of ['','ababa','baab','a😀b']) for(const mode of ['','u']) {
    cases.push({id:`generated:${n}:${mode||'legacy'}:${cases.length}`,source,flags:`d${mode}`,subject,start_utf16:0});
  }
}
for(const source of ['(?:^)*','(?:$)+','(?:(?=a))*','(?:(?!b))+','(?:\\b)?','(?:(?<=a)){1,2}','(?r)','(?q:a)']) {
  for(const subject of ['','a','b','ab']) for(const mode of ['','u']) cases.push({id:`grammar:${cases.length}`,source,flags:`d${mode}`,subject,start_utf16:0});
}
function encode(text) {
  const bytes=[];
  for (const ch of text) {
    const p=ch.codePointAt(0);
    if (p<0x80) bytes.push(p);
    else if (p<0x800) bytes.push(0xc0|p>>6,0x80|p&63);
    else if (p<0x10000) bytes.push(0xe0|p>>12,0x80|p>>6&63,0x80|p&63);
    else bytes.push(0xf0|p>>18,0x80|p>>12&63,0x80|p>>6&63,0x80|p&63);
  }
  return Buffer.from(bytes).toString('hex');
}
const expectedText=cases.map(row=>JSON.stringify(runCase(row))).join('\n')+'\n';
const run=spawnSync(resolve(probe),[],{input:cases.map(row=>[row.id,encode(row.source),row.flags,encode(row.subject),row.start_utf16].join('\t')).join('\n')+'\n',encoding:'utf8',maxBuffer:128*1024*1024,timeout:120_000});
assert.ifError(run.error);assert.equal(run.status,0,run.stderr);
const expected=parseAnswers(expectedText),actual=parseAnswers(run.stdout);
assert.equal(actual.size,cases.length,'missing/extra answers');
for (const id of actual.keys()) assert(expected.has(id),`unexpected ${id}`);
const unsupported=[...actual.values()].filter(row=>row.outcome==='unsupported');
let differences=compareAnswers(expected,actual);
const rawDifferences=differences.length;
let referenceDisagreements=[];
if (allow) {
  const listed=JSON.parse(readFileSync(new URL('../tests/fixtures/unsupported.json',import.meta.url)));
  assert.deepEqual(unsupported,listed,'unsupported coverage changed; all newly missing behavior needs review');
  const accepted=new Set(listed.map(row=>row.id));
  differences=differences.filter(row=>!accepted.has(row.id));
}
if (reviewed) {
  referenceDisagreements=JSON.parse(readFileSync(new URL('../tests/fixtures/reference-disagreements.json',import.meta.url)));
  for(const row of referenceDisagreements) {
    assert.deepEqual(cases.find(item=>item.id===row.id),row.case,'reviewed input changed');
    assert.deepEqual(expected.get(row.id),row.node,'Node disagreement changed');
    assert.deepEqual(actual.get(row.id),row.perex,'Perex disagreement changed');
  }
  const ids=new Set(referenceDisagreements.map(row=>row.id));
  differences=differences.filter(row=>!ids.has(row.id));
}
if (output) {
  mkdirSync(output,{recursive:true});
  writeFileSync(resolve(output,'cases.json'),JSON.stringify({cases}));
  writeFileSync(resolve(output,'expected.jsonl'),expectedText);
  writeFileSync(resolve(output,'actual.jsonl'),run.stdout);
}
const report={node:process.version,cases:cases.length,core_cases:core.length,raw_differences:rawDifferences,node_agreements:cases.length-rawDifferences,unsupported,reviewed_reference_disagreements:referenceDisagreements.length,unreviewed_differences: differences.length,first_differences:differences.slice(0,25)};
if(output) writeFileSync(resolve(output,'summary.json'),JSON.stringify(report,null,2)+'\n');
console.log(JSON.stringify(report,null,2));
process.exitCode=differences.length?1:0;
