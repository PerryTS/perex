#!/usr/bin/env node
import assert from 'node:assert/strict';
import {writeFileSync,mkdirSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
import {runCase,parseAnswers,compareAnswers} from './reference.mjs';
const [probe,output]=process.argv.slice(2);assert(probe,'usage: check-admission.mjs PROBE [OUTPUT_DIR]');
const cases=[];
function add(source,flags,subject,start_utf16=0){cases.push({id:`admission:${cases.length}`,source,flags:`d${flags}`,subject,start_utf16});}
const patterns=['.*needle','a*missing','[^/]+:token','.*->','\\s+(?=[^()]*\\))','([A-F].*[a-f])|([a-f].*[A-F])',
 '(?<!#.*)root\\s*=','(?<=needle.*)z','(?<=\\w*needle)a+','a*','a*b|b+','(?!a+)b+','(?:a+|b+)',
 '(?:a+)?b+','(?:(?<x>a)|(?<x>b))+','(?:(?<x>ab)|(?<x>ab))+\\k<x>',
 '(?:(ab)|(ab))+','(a+)?b\\1','(?=(a+))a*b\\1','(?!(a+))b+','[a-f]+','[A-F]+','[^a-f]+','[a-fa-f]+',
 '[]+','[^]+','[\\w]+','[\\W]+','[\\s]+','[\\S]+','[\\p{Lu}]+','.*ſK','.*Σσ','.*😀','.*\\ud83d','.*\\ude00',
 '(?:abc|abc)+','(?:abc|ab)+c','(?<=abc+)d','(?<!abc+)d+','(?:foo+|bar+)','(?=needle).*','a*(?=needle)',
 'a*(?!needle)','(?:a+)?','(?:(?=needle))*','(?:ab|ab)c+','(?:a|a)bc+','(?:ab){1,3}bc','(?:ab){0,3}bc'];
for(const source of patterns)for(const flags of ['','u','i','ui'])for(const prefix of ['','x'.repeat(63),'x'.repeat(64),'x'.repeat(255),'0123456789 '.repeat(24)])
 for(const tail of ['','needle','needleaaaaaz','aababa','b'.repeat(80),'Aaaa','root = /','ſKsK','Σσς','😀😀','\ud83d\ud83d','\ude00','\nneedle'])add(source,flags,prefix+tail);
for(const source of ['(?<=needle.*)z+','(?<=\\w*needle)a+','.*needle','(?:(?<x>.)|(?<x>a))+\\k<x>'])
 for(const flags of ['g','gy','ug','uy'])for(const subject of ['needle'+'a'.repeat(100)+'z','x'.repeat(70)+'😀needle'])
  for(const start of [0,5,6,64,71,72,73,subject.length,subject.length+1])add(source,flags,subject,start);
let seed=0x3fca529e;function random(n){seed^=seed<<13;seed^=seed>>>17;seed^=seed<<5;return(seed>>>0)%n;}
function pattern(depth){const atoms=['a','b','[ab]','[A-F]','\\w','ſ','😀','(?:)'];if(!depth)return atoms[random(atoms.length)];const child=()=>pattern(depth-1);switch(random(7)){case 0:return`(?:${child()}|${child()})`;case 1:return`(${child()})`;case 2:return`(?:${child()})${['?','{1,3}','{0,2}'][random(3)]}`;case 3:return`(?=${child()})${child()}`;case 4:return`(?<=${child()})${child()}`;case 5:return`(?!${child()})${child()}`;default:return child()+child();}}
for(let i=0;i<512;i++){const source=`(?:${pattern(2)})+Q`;for(const flags of ['','u','i','ui'])for(const tail of ['','ababaQ','baabQ','ſQ','😀Q'])add(source,flags,'0123456789 '.repeat(8)+tail);}
function encode(s){const b=[];for(const ch of s){const p=ch.codePointAt(0);if(p<128)b.push(p);else if(p<2048)b.push(0xc0|p>>6,0x80|p&63);else if(p<65536)b.push(0xe0|p>>12,0x80|p>>6&63,0x80|p&63);else b.push(0xf0|p>>18,0x80|p>>12&63,0x80|p>>6&63,0x80|p&63);}return Buffer.from(b).toString('hex');}
const expectedText=cases.map(r=>JSON.stringify(runCase(r))).join('\n')+'\n';
const run=spawnSync(resolve(probe),[],{input:cases.map(r=>[r.id,encode(r.source),r.flags,encode(r.subject),r.start_utf16].join('\t')).join('\n')+'\n',encoding:'utf8',maxBuffer:256*1024*1024,timeout:120_000});assert.ifError(run.error);assert.equal(run.status,0,run.stderr);
const differences=compareAnswers(parseAnswers(expectedText),parseAnswers(run.stdout));const report={node:process.version,cases:cases.length,differences:differences.length,first_differences:differences.slice(0,25).map(d=>({...d,case:cases.find(r=>r.id===d.id)}))};
if(output){mkdirSync(output,{recursive:true});writeFileSync(resolve(output,'cases.json'),JSON.stringify({cases}));writeFileSync(resolve(output,'expected.jsonl'),expectedText);writeFileSync(resolve(output,'actual.jsonl'),run.stdout);writeFileSync(resolve(output,'summary.json'),JSON.stringify(report,null,2)+'\n');}console.log(JSON.stringify(report,null,2));process.exitCode=differences.length?1:0;
