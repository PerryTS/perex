#!/usr/bin/env node
// Development reference only. Compare every capture's original units and offsets,
// every named result (including unset), syntax errors and failed searches.
import assert from 'node:assert/strict';
import {readFileSync,writeFileSync,mkdirSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
import {runCase,parseAnswers,compareAnswers} from './reference.mjs';
const args=process.argv.slice(2),probe=args.shift();
assert(probe,'usage: check-names.mjs PROBE [--output DIR]');
const outIndex=args.indexOf('--output'),output=outIndex<0?null:args[outIndex+1];
assert(outIndex<0||output,'--output needs a directory');
const cases=[];
function add(source,flags,subject,start_utf16=0){cases.push({id:`names:${cases.length}`,source,flags:`d${flags}`,subject,start_utf16});}
const patterns=[
 '(?<x>a)','(?<x>a)?b\\k<x>','\\k<x>(?<x>a)','(?<x>\\k<x>a)','(?<x>ab|a)\\k<x>b',
 '(?<x>a)|(?<x>b)','(?:(?<x>a)|(?<x>b))+\\k<x>','(?:(?<x>a)|(?<x>b)){1,3}?\\k<x>',
 '(?:(?<x>a)|(?<x>b))?\\k<x>','(?:(?<x>a)|(?<x>b)){0}\\k<x>',
 '(?:(?<x>a)|(?<x>b))\\1\\2','(?<x>a)|(?<y>b)|(?<x>c)',
 '(?:(?<x>a)|b)(?:(?<x>b)|a)','(?<x>a)?(?<x>b)?','(?<x>a(?<x>b))',
 '(?:(?<x>a)|(?:(?<x>b)|(?<x>c)))\\k<x>',
 '(?<x>(?<y>a))|(?<x>(?<y>b))','(?:(?<x>)|(?<x>a))+\\k<x>',
 '(?=(?<x>a+))a*b\\k<x>','(?!(?<x>a))b\\k<x>',
 '(?=(?<x>a))(?<x>a)','(?!(?<x>a))(?<x>b)',
 '(?<=\\k<x>(?<x>a))b','(?<=(?<x>a)\\k<x>)b',
 '(?<=\\k<x>(?:(?<x>a)|(?<x>b)))c','(?<=(?:(?<x>a)|(?<x>b))\\k<x>)c',
 '(?:(?=(?<x>a))a|(?<x>b))+\\k<x>','(?:(?!(?<x>b))a|(?<x>b))+\\k<x>',
 '(?<x>.)\\k<x>','(?<x>😀)\\k<x>','(?<x>ſ)\\k<x>',
 '(?<x>k)\\k<x>','(?<X>a)\\k<x>','(?<x>a)\\k<X>',
 '(?<__proto__>a)(?<constructor>b)','\\k<b>(?<a>a)(?<b>b)',
 '(?<𐐀>a)(?<ﬀ>b)','(?<a\\u200c>b)\\k<a\\u200c>',
 '(?<\\u0061>a)\\k<a>','(?<a>a)\\k<\\u0061>',
 '(?<a>a)|(?<\\u0061>b)','(?<a>a)(?<\\u0061>b)',
 '(?<é>a)(?<é>b)','(?<á>a)\\k<á>',
 '(?<>a)','(?<1x>a)','(?<a-b>a)','(?<\\x61>a)','(?<\\uZZZZ>a)',
 '(?<\\u{110000}>a)','(?<\\uD800>a)','(?<\\u{D801}\\u{DC00}>a)',
 '(?<\\uD801\\uDC00>a)\\k<𐐀>','(?<\\u{10400}>a)\\k<𐐀>',
 '\\k','\\k<x>','\\k<bad-name>','[\\k]','[\\k<a>]','\\k<','\\k<>',
 '\\k<x>[(?<x>)]','\\k<a>\\(?<a>','\\k<a>(?<a>b)',
 '[\\k](?<x>a)','(?<x>a)[\\k]','(?<x>a)\\k','(?<x>a)\\k<>',
 '(?<x>a)\\k<missing>','\\k<missing>(?<x>a)','[\\k](?<=a)',
 '\\k<name>\\\\(?<name>a)','\\k<name>\\\\\\(?<name>',
];
const subjects=['','a','b','ab','aa','bb','aba','abb','bba','abc','bbc','aaab','baab','aabaa','k<x>','k<bad-name>','k','😀😀','\ud83d\ud83d','a😀b','sſ','Kk'];
for(const source of patterns)for(const flags of ['','u','i','ui'])for(const subject of subjects)add(source,flags,subject);
for(const source of ['(?<x>.)','(?:(?<x>.)|(?<x>a))\\k<x>','(?<x>😀)','(?<x>)'])
 for(const subject of ['','a😀b','😀😀','\ud83d\ud83d'])for(let start=0;start<=subject.length+1;start++)for(const flags of ['g','gy','ug','uy'])add(source,flags,subject,start);
// Unicode identifier boundary values come from the pinned UCD. The oracle
// still compiles each complete expression independently; no expected answers
// or acceptance lists are derived from Perex tables.
const points=new Set([0,36,95,0x200c,0x200d,0xd800,0xdfff,0x10ffff]);
for(const line of readFileSync(new URL('../third_party/unicode/17.0.0/DerivedCoreProperties.txt',import.meta.url),'utf8').split('\n')){
 const m=/^([0-9A-F]+)(?:\.\.([0-9A-F]+))?\s*; (ID_Start|ID_Continue)\b/.exec(line);
 if(m)for(const n of [parseInt(m[1],16),parseInt(m[2]??m[1],16)])for(const delta of [-1,0,1])if(n+delta>=0&&n+delta<=0x10ffff)points.add(n+delta);
}
for(const cp of [...points].sort((a,b)=>a-b)){
 const raw=String.fromCodePoint(cp),escaped=`\\u{${cp.toString(16)}}`;
 for(const name of [raw,`a${raw}`,escaped,`a${escaped}`])for(const flags of ['','u'])add(`(?<${name}>a)\\k<${name}>`,flags,'aa');
}
let seed=0x596eab73;
function random(n){seed^=seed<<13;seed^=seed>>>17;seed^=seed<<5;return(seed>>>0)%n;}
function pattern(depth){
 if(!depth)return ['a','b','(?<x>a)','(?<x>b)','(?<y>a)','\\k<x>','(?:)'][random(7)];
 const child=()=>pattern(depth-1);
 switch(random(8)){
  case 0:return`(?:${child()}|${child()})`;
  case 1:return`(?<x>${child()})`;
  case 2:return`(?:${child()})${['*','+','?','{1,2}?'][random(4)]}`;
  case 3:return`(?=${child()})${child()}`;
  case 4:return`(?<=${child()})${child()}`;
  case 5:return`(?!${child()})${child()}`;
  default:return child()+child();
 }
}
for(let i=0;i<1024;i++){
 const source=pattern(3);
 for(const subject of ['','ababa','baab'])for(const flags of ['','u'])add(source,flags,subject);
}
const wide=Array.from({length:260},(_,i)=>`(?<n${i}>a)`).join('')+'\\k<n259>';
add(wide,'','a'.repeat(261));
function encode(s){
 const bytes=[];
 for(const ch of s){const p=ch.codePointAt(0);if(p<128)bytes.push(p);else if(p<2048)bytes.push(0xc0|p>>6,0x80|p&63);else if(p<65536)bytes.push(0xe0|p>>12,0x80|p>>6&63,0x80|p&63);else bytes.push(0xf0|p>>18,0x80|p>>12&63,0x80|p>>6&63,0x80|p&63);}
 return Buffer.from(bytes).toString('hex');
}
const expectedText=cases.map(row=>JSON.stringify(runCase(row))).join('\n')+'\n';
const run=spawnSync(resolve(probe),[],{input:cases.map(r=>[r.id,encode(r.source),r.flags,encode(r.subject),r.start_utf16].join('\t')).join('\n')+'\n',encoding:'utf8',maxBuffer:256*1024*1024,timeout:120_000});
assert.ifError(run.error);assert.equal(run.status,0,run.stderr);
const differences=compareAnswers(parseAnswers(expectedText),parseAnswers(run.stdout));
const summary={node:process.version,unicode:process.versions.unicode,cases:cases.length,identifier_boundary_values:points.size,differences:differences.length,first_differences:differences.slice(0,25).map(d=>({...d,case:cases.find(c=>c.id===d.id)}))};
if(output){mkdirSync(output,{recursive:true});writeFileSync(resolve(output,'cases.json'),JSON.stringify({cases}));writeFileSync(resolve(output,'expected.jsonl'),expectedText);writeFileSync(resolve(output,'actual.jsonl'),run.stdout);writeFileSync(resolve(output,'summary.json'),JSON.stringify(summary,null,2)+'\n');}
console.log(JSON.stringify(summary,null,2));
process.exitCode=differences.length?1:0;
