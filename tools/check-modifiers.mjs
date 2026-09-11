#!/usr/bin/env node
import assert from 'node:assert/strict';
import {writeFileSync,mkdirSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
import {runCase,parseAnswers,compareAnswers} from './reference.mjs';
const [probe,output]=process.argv.slice(2);assert(probe,'usage: check-modifiers.mjs PROBE [OUTPUT_DIR]');
const cases=[];
function add(source,flags,subject,start_utf16=0){cases.push({id:`modifiers:${cases.length}`,source,flags:`d${start_utf16 ? "g" : ""}${flags}`,subject,start_utf16});}

const flagsList=['','i','m','s','ims','u','iu','imsu'];
const modifiers=[];
for(let i=0;i<3;i++)for(let m=0;m<3;m++)for(let s=0;s<3;s++){
 let add='',remove='';for(const [c,v] of [['i',i],['m',m],['s',s]]){if(v===1)add+=c;if(v===2)remove+=c;}
 if(add||remove)modifiers.push(add+(remove?'-'+remove:''));
}
const bodies=['a','[a-z]+','[^a]+','\\w+','\\W+','\\bſ\\b','(.)\\1','(?<x>a)\\k<x>','^(.+)$','(a|b)+','(?<=([a-z]+))b','(?=(a+))\\1','(?!(a+))b'];
const subjects=['','a','A','aA','Aa','AA','bB','abc','AbC','A\nb','x\na\ny','\r\n','\u2028','ſK','Σςσ','😀😀','\ud800A\udfff','a'.repeat(64)+'B'];
for(const flags of flagsList)for(const modifier of modifiers)for(const body of bodies)
 for(const subject of subjects)for(const start of [0,1])add(`(?${modifier}:${body})`,flags,subject,start);
for(const outer of modifiers)for(const inner of modifiers){
 for(const source of [
  `^(?${outer}:(a)(?${inner}:\\1))$`,
  `(?${outer}:a(?${inner}:b)c)d`,
  `(?${outer}:^(?${inner}:(.))$)`,
 ])for(const flags of ['','imsu'])for(const subject of ['aA','Aa','AbCd','ABCD','x\nA\ny','\n'])add(source,flags,subject);
}
// Admission must distinguish lexical flags across runs and alternatives.
for(const source of [
 '(?i:a)b+','(?i:ab)c+','(?i:a)(?-i:b)c+',
 '(?i:a)+b|a+c','(?i:a)b+|aB+',
 '(?i:[a-z])+Q','(?-i:[a-z])+Q',
 '(?<=(?i:a)b+)Q','(?<=(?i:ab)c+)Q',
 '(?i:\\p{Lowercase_Letter})+Q','(?-i:[^\\P{Lowercase_Letter}])+Q',
 '(?i:(?<x>a)|(?<x>b))(?-i:\\k<x>)',
 '(?i:(?<x>a)|(?<x>b))(?i:\\k<x>)',
])for(const flags of flagsList)for(const subject of ['A'.repeat(80)+'Q','a'.repeat(80)+'Q','ab'.repeat(48)+'Q','AB'.repeat(48)+'Q','0'.repeat(80)+'Abbb','0'.repeat(80)+'aBBB','ſ'.repeat(80)+'Q','😀'.repeat(80)+'Q','aA','Aa','AA','bb'])add(source,flags,subject);
// A trailing dash with nonempty additions is valid. Empty sides together,
// repeated flags, overlap, foreign flags, escapes and unscoped forms are not.
const words=[''];for(let n=1;n<=4;n++){const prev=words.filter(w=>w.length===n-1);for(const w of prev)for(const c of 'ims')words.push(w+c);}
for(const word of words)for(const flags of ['','u','ims']){
 for(const source of [`(?${word}:a)`,`(?${word}-:a)`,`(?-${word}:a)`])add(source,flags,'A');
}
const short=words.filter(w=>w.length<=2);
for(const a of short)for(const b of short)for(const flags of ['','u'])add(`(?${a}-${b}:a)`,flags,'A');
for(const modifier of ['g','d','y','u','v','x','I','i-m-s','i--m','-','--','i i','i\\m','\\u0069','i=','!i'])
 for(const flags of ['','u'])for(const tail of [':a)',')'])add('(?'+modifier+tail,flags,'a');
function encode(s){const b=[];for(const ch of s){const p=ch.codePointAt(0);if(p<128)b.push(p);else if(p<2048)b.push(0xc0|p>>6,0x80|p&63);else if(p<65536)b.push(0xe0|p>>12,0x80|p>>6&63,0x80|p&63);else b.push(0xf0|p>>18,0x80|p>>12&63,0x80|p>>6&63,0x80|p&63);}return Buffer.from(b).toString('hex');}
const expectedText=cases.map(r=>JSON.stringify(runCase(r))).join('\n')+'\n';
const run=spawnSync(resolve(probe),[],{input:cases.map(r=>[r.id,encode(r.source),r.flags,encode(r.subject),r.start_utf16].join('\t')).join('\n')+'\n',encoding:'utf8',maxBuffer:256*1024*1024,timeout:300_000});assert.ifError(run.error);assert.equal(run.status,0,run.stderr);
const differences=compareAnswers(parseAnswers(expectedText),parseAnswers(run.stdout));const report={node:process.version,cases:cases.length,differences:differences.length,first_differences:differences.slice(0,25).map(d=>({...d,case:cases.find(r=>r.id===d.id)}))};
if(output){mkdirSync(output,{recursive:true});writeFileSync(resolve(output,'cases.json'),JSON.stringify({cases}));writeFileSync(resolve(output,'expected.jsonl'),expectedText);writeFileSync(resolve(output,'actual.jsonl'),run.stdout);writeFileSync(resolve(output,'summary.json'),JSON.stringify(report,null,2)+'\n');}console.log(JSON.stringify(report,null,2));process.exitCode=differences.length?1:0;
