#!/usr/bin/env node
import assert from 'node:assert/strict';
import {writeFileSync,mkdirSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
import {runCase,parseAnswers,compareAnswers,stableDifferences} from './reference.mjs';
const [probe,output]=process.argv.slice(2);
assert(probe,'usage: check-legacy.mjs PROBE [OUTPUT_DIR]');
const cases=[];
function add(source,flags,subject){cases.push({id:`legacy:${cases.length}`,source,flags:`d${flags}`,subject,start_utf16:0});}
function fallback(digits){
 const match=/^[0-3][0-7]{0,2}|^[4-7][0-7]?/.exec(digits);
 return match?String.fromCharCode(parseInt(match[0],8))+digits.slice(match[0].length):digits;
}
const numbers=new Set(Array.from({length:1024},(_,i)=>String(i)));
for(const n of ['00','01','000','001','007','008','009','010','077','099','0123','0000','0001','0008','0789','1234','3777','4000','7777','8888','9999','1000000000000000000000000','9999999999999999999999999'])numbers.add(n);
for(const digits of numbers){
 const escaped=`\\${digits}`,literal=fallback(digits);
 for(const flags of ['','u']){
  for(const source of [escaped,`${escaped}+`,`${escaped}{0,2}`,`[${escaped}]+`,`[^${escaped}]`,`[${escaped}-z]`])
   for(const subject of ['',literal,literal+literal,'a'])add(source,flags,subject);
  for(const count of [1,2,9,10,12]){
   const groups='(a)'.repeat(count),base='a'.repeat(count);
   for(const subject of [base,base+'a',base+literal])add(groups+escaped,flags,subject);
   for(const subject of [base,literal+base])add(escaped+groups,flags,subject);
  }
 }
}
for(let c=0;c<256;c++){
 const ch=String.fromCharCode(c),control=String.fromCharCode(c%32);
 for(const source of [`\\c${ch}`,`[\\c${ch}]`,`\\c${ch}*`,`[\\c${ch}-z]`])
  for(const flags of ['','u'])for(const subject of [control,`\\c${ch}`,ch,'\\'])add(source,flags,subject);
}
const patterns=[
 '\\c','\\c*','\\c+','\\c?','\\c{2}','[\\c]','[\\c-]','[a-\\c]','\\c(a)\\1',
 '\\1[(](?:a)(?=b)','\\2(a)(?<n>b)','\\2\\k<n>(a)(?<n>b)','\\k<n>\\2(a)(?<n>b)',
 '\\2\\(a\\)(b)','\\2[()](a)','\\1\\\\(a)','\\1\\\\\\(a\\)',
 '(?=(a))*a','(?=(a))+a','(?=(a)){0,2}a','(?=(a)){2}a','(?=(a))??a','(?!a)*b','(?!a)+b',
 '(?!(a))?b','(?:(?=(a))?a|b)+','(?=a)*','(?<=a)*','(?<!a)?','(?<=a){0}',
 '\\x','\\xZ','\\x1','\\x123','\\u','\\u0','\\u12X4','\\u{61}',
 '\\x(a)\\1','\\u(a)\\1','\\c(?<n>a)\\k<n>','[\\c](?<n>a)\\k<n>',
];
for(const source of patterns)for(const flags of ['','u','i'])for(const subject of ['','a','b','aa','ab','ba','bbb','aaa','\\','\\c','\\ccc','\u0001(a)','\u0002a','😀','\ud800'])add(source,flags,subject);
for(const count of [99,100,123,255,256]){
 const groups='(a)'.repeat(count),subject='a'.repeat(count);
 for(const n of [count-1,count,count+1])for(const flags of ['','u']){
  add(`${groups}\\${n}`,flags,subject+'a');add(`\\${n}${groups}`,flags,subject);
 }
}
function encode(s){const bytes=[];for(const ch of s){const p=ch.codePointAt(0);if(p<128)bytes.push(p);else if(p<2048)bytes.push(0xc0|p>>6,0x80|p&63);else if(p<65536)bytes.push(0xe0|p>>12,0x80|p>>6&63,0x80|p&63);else bytes.push(0xf0|p>>18,0x80|p>>12&63,0x80|p>>6&63,0x80|p&63);}return Buffer.from(bytes).toString('hex');}
const expectedText=cases.map(r=>JSON.stringify(runCase(r))).join('\n')+'\n';
const run=spawnSync(resolve(probe),[],{input:cases.map(r=>[r.id,encode(r.source),r.flags,encode(r.subject),0].join('\t')).join('\n')+'\n',encoding:'utf8',maxBuffer:512*1024*1024,timeout:120_000});
assert.ifError(run.error);assert.equal(run.status,0,run.stderr);
const {differences,unstable}=stableDifferences(parseAnswers(expectedText),parseAnswers(run.stdout),cases);
const report={node:process.version,cases:cases.length,numeric_spellings:numbers.size,unstable_oracle_answers:unstable,differences:differences.length,first_differences:differences.slice(0,25).map(d=>({...d,case:cases.find(r=>r.id===d.id)}))};
if(output){mkdirSync(output,{recursive:true});writeFileSync(resolve(output,'cases.json'),JSON.stringify({cases}));writeFileSync(resolve(output,'expected.jsonl'),expectedText);writeFileSync(resolve(output,'actual.jsonl'),run.stdout);writeFileSync(resolve(output,'summary.json'),JSON.stringify(report,null,2)+'\n');}
console.log(JSON.stringify(report,null,2));process.exitCode=differences.length?1:0;
