#!/usr/bin/env node
// V8 miscompiles a scoped modifier group whose alternation captures, when the
// winning alternative required case folding and a backreference reads it.
//
//   node repro.mjs                 -> exits 1, probes return null
//   node repro.mjs --no-workload   -> exits 0, probes return the right match
//   node --regexp-interpret-all repro.mjs   -> exits 0
//   node --no-regexp-optimization repro.mjs -> exits 0
//   node --jitless repro.mjs                -> exits 0
//
// The workload below is a differential-test case list. Nothing in it uses a
// probe pattern or subject; it only has to compile enough regexps first. The
// probes must not be compiled before it -- compiling one early is enough to
// make it answer correctly for the rest of the process.

const PROBES = [
  // [pattern, flags, subject, expected]
  ["(?i:(a)|(b))\\2", "d", "BB", '0:"BB"'],
  ["(?i:(?<x>a)|(?<x>b))\\k<x>", "d", "BB", '0:"BB"'],
  ["(?i:(?<x>a)|(?<x>b))(?-i:\\k<x>)", "d", "aBBB", '1:"BB"'],
  ["(?i:(?<x>a)|(?<x>b))(?-i:\\k<x>)", "d", "0".repeat(80) + "aBBB", '81:"BB"'],
  // Controls: the same shapes without folding, or taking the first
  // alternative, or without the scoped modifier. These stay correct.
  ["(?i:(a)|(b))\\2", "d", "bb", '0:"bb"'],
  ["(?i:(a)|(b))\\1", "d", "AA", '0:"AA"'],
  ["((a)|(b))\\3", "d", "bb", '0:"bb"'],
  ["(?i:(a))\\1", "d", "aa", '0:"aa"'],
];

function answer([source, flags, subject]) {
  const m = new RegExp(source, flags).exec(subject);
  return m === null ? "null" : `${m.index}:${JSON.stringify(m[0])}`;
}

function runCase(row) {
  let re;
  try { re = new RegExp(row.source, row.flags); } catch { return; }
  re.lastIndex = row.start_utf16;
  const match = re.exec(row.subject);
  if (match === null) return;
  // Materializing the answer is what the differential harness does.
  Array.from(match, (text, i) => text === undefined ? null : Array.from(match.indices[i]));
  if (match.groups !== undefined) Object.keys(match.groups).sort();
}

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

if (!process.argv.includes("--no-workload")) {
  for (const row of cases) runCase(row);
}

let failures = 0;
for (const probe of PROBES) {
  const actual = answer(probe);
  const ok = actual === probe[3];
  if (!ok) failures++;
  console.log(`${ok ? "ok  " : "FAIL"}  /${probe[0]}/${probe[1]} on ${JSON.stringify(probe[2]).slice(0, 24)}  expected ${probe[3]}, got ${actual}`);
}
console.log(`\n${cases.length} regexps in the workload, ${failures} probe(s) wrong`);
console.log(`node ${process.version}, v8 ${process.versions.v8}`);
process.exitCode = failures ? 1 : 0;
