#!/usr/bin/env node
import assert from 'node:assert/strict';
import {writeFileSync,mkdirSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
import {runCase,parseAnswers,compareAnswers,stableDifferences} from './reference.mjs';
const [probe,output]=process.argv.slice(2);assert(probe,'usage: check-repetition.mjs PROBE [OUTPUT_DIR]');
const cases=[];
function add(source,flags,subject,start_utf16=0){cases.push({id:`repetition:${cases.length}`,source,flags:`d${flags}`,subject,start_utf16});}
const quantifiers=['?','*','+','{0}','{1}','{2}','{0,2}','{1,3}','{2,3}','{3,5}','{0,}','{1,}','{2,}','??','*?','+?','{0,2}?','{1,3}?'];
for(const atom of ['a','[ab]'])for(const inner of quantifiers)for(const outer of quantifiers){
 const nested=`(?:(?:${atom})${inner})${outer}`;
 for(const source of [`^(${nested})([ab]*?)!$`,`^(${nested})([ab]{1,2})!$`,`(?<=^(${nested})([ab]*?))!`,`^(${nested})\\1!$`])
  for(const flags of ['','i'])for(let length=0;length<8;length++)for(const letter of ['a','b'])add(source,flags,letter.repeat(length)+'!');
}
for(const atom of ['a','[ab]','\\w','.','ſ','\\ud83d','\\ude00','\\p{L}'])
 for(const inner of ['?','+','{0,2}','{1,3}','|(?:)','(?:)|','{1,3}?']){
  const body=inner==='|(?:)'?`(?:${atom}|(?:))`:inner==='(?:)|'?`(?:(?:)|${atom})`:`(?:${atom})${inner}`;
  for(const source of [`^((?:${body})+)(.*?)!$`,`(?<=^((?:${body})+)(.*?))!`,`^((?:(${body}))+)(.*?)!$`])
   for(const flags of ['','u','i','ui'])for(const subject of ['!','a!','aaaaa!','ababa!','ſſS!','😀😀!','\ud83d\ud83d!','\ude00\ude00!'])add(source,flags,subject);
 }
// Exact failure witnesses remain in check-admission; these stress captures and
// observable first-end priority beyond the compiler's permitted rewrite.
for(const body of ['(?:a{3,5}){0,2}','(?:a{2})+','(?:(a)|(?:)){1,3}','(?:(?=a)a?)+','(?:ab?)+','(?:a|aa)+'])
 for(const source of [`^(${body})(a*?)!$`,`(?<=^(${body})(a*?))!`])for(let n=0;n<12;n++)add(source,'','a'.repeat(n)+'!');
function encode(s){const b=[];for(const ch of s){const p=ch.codePointAt(0);if(p<128)b.push(p);else if(p<2048)b.push(0xc0|p>>6,0x80|p&63);else if(p<65536)b.push(0xe0|p>>12,0x80|p>>6&63,0x80|p&63);else b.push(0xf0|p>>18,0x80|p>>12&63,0x80|p>>6&63,0x80|p&63);}return Buffer.from(b).toString('hex');}
const expectedText=cases.map(r=>JSON.stringify(runCase(r))).join('\n')+'\n';
const run=spawnSync(resolve(probe),[],{input:cases.map(r=>[r.id,encode(r.source),r.flags,encode(r.subject),r.start_utf16].join('\t')).join('\n')+'\n',encoding:'utf8',maxBuffer:256*1024*1024,timeout:120_000});assert.ifError(run.error);assert.equal(run.status,0,run.stderr);
const {differences,unstable}=stableDifferences(parseAnswers(expectedText),parseAnswers(run.stdout),cases);const report={node:process.version,cases:cases.length,unstable_oracle_answers:unstable,differences:differences.length,first_differences:differences.slice(0,25).map(d=>({...d,case:cases.find(r=>r.id===d.id)}))};
if(output){mkdirSync(output,{recursive:true});writeFileSync(resolve(output,'cases.json'),JSON.stringify({cases}));writeFileSync(resolve(output,'expected.jsonl'),expectedText);writeFileSync(resolve(output,'actual.jsonl'),run.stdout);writeFileSync(resolve(output,'summary.json'),JSON.stringify(report,null,2)+'\n');}console.log(JSON.stringify(report,null,2));process.exitCode=differences.length?1:0;
