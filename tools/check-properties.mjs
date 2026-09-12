#!/usr/bin/env node
// All scalar/surrogate membership values from real compiled programs. Separate
// complete-answer cases cover aliases, complements/case folding and bad syntax.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import { runCase, parseAnswers, compareAnswers ,stableDifferences} from './reference.mjs';
const args=process.argv.slice(2);
const allowReviewed=args.includes('--allow-reviewed-reference-disagreements');
const [propertyProbe,engineProbe,output] = args.filter(a=>a!=='--allow-reviewed-reference-disagreements');
assert(engineProbe,'usage: check-properties.mjs PROPERTY_PROBE ENGINE_PROBE [OUTPUT_DIR] [--allow-reviewed-reference-disagreements]');
const fixture=JSON.parse(readFileSync(new URL('../tests/fixtures/properties.json',import.meta.url)));
const expected=[];
if(output) mkdirSync(output,{recursive:true});
const save=(file,text)=>{ if(output) writeFileSync(resolve(output,file),text); };
for(const [index,{name}] of fixture.properties.entries()) {
  let re;
  try { re=new RegExp(`^\\p{${name}}$`,'u'); }
  catch(error) {
    if(!(error instanceof SyntaxError)) throw error;
    expected.push({name,outcome:'syntax-error'});
    continue;
  }
  const boundaries=[];
  let previous=false;
  for(let c=0;c<0x110000;c++) {
    const found=re.test(String.fromCodePoint(c));
    if(found!==previous) { boundaries.push(c); previous=found; }
  }
  if(previous) boundaries.push(0x110000);
  expected.push({name,boundaries});
  if(index%32===0) console.error(`Node membership ${index+1}/${fixture.properties.length}`);
}
save('membership-expected.jsonl',expected.map(r=>JSON.stringify(r)).join('\n')+'\n');
const run=spawnSync(resolve(propertyProbe),[],{
  input:fixture.properties.map(r=>r.name).join('\n')+'\n',encoding:'utf8',
  maxBuffer:64*1024*1024,timeout:600_000,
});
save('membership-actual.jsonl',run.stdout??'');save('membership-stderr.txt',run.stderr??'');
assert.ifError(run.error);assert.equal(run.status,0,run.stderr);
const actual=run.stdout.trim().split('\n').map(line=>JSON.parse(line));
assert.equal(actual.length,expected.length);
const membershipDifferences=expected.flatMap((row,index)=>isDeepStrictEqual(row,actual[index])?[]:[{
  name:row.name,actual_name:actual[index].name,
  expected_outcome:row.outcome??'membership',
  first_different_boundaries:(row.boundaries??[]).filter((b,i)=>actual[index].boundaries[i]!==b).slice(0,8),
  expected_boundaries:row.boundaries?.length??null,actual_boundaries:actual[index].boundaries.length,
}]);
const cases=[];
function add(source,flags,subject) { cases.push({id:`properties:${cases.length}`,source,flags:`d${flags}`,subject,start_utf16:0}); }
const fixed=[0,9,32,48,65,95,97,0x17f,0x3a3,0x212a,0x200d,0x3042,0x30fc,0xd800,0xdfff,0xe000,0x10428,0x1f600,0x10ffff];
for(const [i,row] of fixture.properties.entries()) {
  const boundaries=expected[i].boundaries??[];
  const selected=[...boundaries.slice(0,6),...boundaries.slice(-6)];
  const points=[...new Set([...fixed,...selected.flatMap(c=>[c-1,c,c+1])].filter(c=>c>=0&&c<0x110000))];
  for(const name of new Set([row.name,...row.aliases])) {
    for(const c of points) for(const flag of ['u','iu']) {
      add(`(\\p{${name}})`,flag,String.fromCodePoint(c));
      add(`[\\P{${name}}]`,flag,String.fromCodePoint(c));
    }
  }
}
for(const source of ['\\p{L}','\\P{Lowercase_Letter}','[^\\P{Lowercase_Letter}]','[\\p{L}\\d]+',
  '(\\p{L})\\1','(?<=\\p{L})\\p{N}','[a-\\p{L}]','[\\p{L}-a]','\\p{}','\\p{L',
  '\\p{lowercase_letter}','\\p{Lowercase Letter}','\\p{General_Category=Script}','\\p{sc=Greek}',
  '\\p{Greek}','\\p{gc=Alphabetic}','\\p{Script=latin}','\\p{Alphabetic=True}','\\p{White_Space=Yes}',
  '\\p{Age=17.0}','\\p{Block=Basic_Latin}','\\p{IDS_Unary_Operator}','\\p{Other_Alphabetic}',
  '\\p{RGI_Emoji}','\\p{Basic_Emoji}','\\p{L=}','\\p{=L}','\\p{gc==L}','\\p{Script_Extensions=}',
  `\\p{${'A'.repeat(65)}}`,'\\p','\\P','\\p{Lu}*','[\\p{ASCII}\\P{ASCII}]']) {
  for(const flags of ['','u','iu']) for(const subject of ['','aAſK','aa','1α2','a😀b','\ud800','p{L}']) add(source,flags,subject);
}
function encode(text) {
  const bytes=[];
  for(const ch of text) {
    const c=ch.codePointAt(0);
    if(c<128) bytes.push(c);
    else if(c<2048) bytes.push(0xc0|c>>6,0x80|c&63);
    else if(c<65536) bytes.push(0xe0|c>>12,0x80|c>>6&63,0x80|c&63);
    else bytes.push(0xf0|c>>18,0x80|c>>12&63,0x80|c>>6&63,0x80|c&63);
  }
  return Buffer.from(bytes).toString('hex');
}
const expectedText=cases.map(r=>JSON.stringify(runCase(r))).join('\n')+'\n';
save('cases.json',JSON.stringify({cases}));save('expected.jsonl',expectedText);
const check=spawnSync(resolve(engineProbe),[],{
  input:cases.map(r=>[r.id,encode(r.source),r.flags,encode(r.subject),0].join('\t')).join('\n')+'\n',
  encoding:'utf8',maxBuffer:512*1024*1024,timeout:300_000,
});
save('actual.jsonl',check.stdout??'');save('stderr.txt',check.stderr??'');
assert.ifError(check.error);assert.equal(check.status,0,check.stderr);
const {differences,unstable}=stableDifferences(parseAnswers(expectedText),parseAnswers(check.stdout),cases);
let reviewedCases=0,reviewedMembership=0;
if(allowReviewed) {
  const reviewed=JSON.parse(readFileSync(new URL('../tests/fixtures/property-reference-disagreements.json',import.meta.url)));
  const byId=new Map(cases.map(r=>[r.id,r]));
  const differingMembership=expected.flatMap((row,i)=>isDeepStrictEqual(row,actual[i])?[]:[{node:row,perex:actual[i]}]);
  assert.deepEqual(differingMembership,reviewed.membership,'reviewed property constructor/membership answers changed');
  const differingCases=differences.map(row=>({case:byId.get(row.id),node:row.expected,perex:row.actual}));
  assert.deepEqual(differingCases,reviewed.cases,'reviewed property inputs/complete answers changed');
  reviewedCases=reviewed.cases.length;reviewedMembership=reviewed.membership.length;
}
const report={node:process.version,unicode:process.versions.unicode,properties:expected.length,
  perex_membership_values:0x110000*expected.length,
  node_membership_values:0x110000*expected.filter(r=>!r.outcome).length,membership_differences:membershipDifferences,
  cases:cases.length,unstable_oracle_answers:unstable,differences:differences.length,node_agreements:cases.length-differences.length,
  reviewed_cases:reviewedCases,reviewed_membership:reviewedMembership,
  unreviewed_differences:membershipDifferences.length+differences.length-reviewedCases-reviewedMembership,
  first_differences:differences.slice(0,12)};
save('summary.json',JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report,null,2));
process.exitCode=report.unreviewed_differences?1:0;
