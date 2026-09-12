#!/usr/bin/env node
// Harvest regular-expression patterns from a Test262 checkout.
//
// Perex is a pattern engine, not a JavaScript runtime, so it cannot execute
// Test262 directly. What it can use is the corpus: the patterns and flags the
// suite exercises, and the suite's own statement about which of them are
// syntax errors. This module only extracts; comparison lives in
// `check-test262.mjs`.
//
// Extraction is deliberately conservative. A pattern is harvested only from a
// form whose bounds are unambiguous, and every harvested pattern is reported
// with the file it came from, so coverage is a counted fact rather than an
// assumption. Forms that are skipped are counted too.
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const FLAG_CHARS = 'dgimsuvy';

export function walk(root) {
  const out = [];
  (function recurse(dir) {
    for (const entry of readdirSync(dir)) {
      const path = join(dir, entry);
      if (statSync(path).isDirectory()) recurse(path);
      else if (entry.endsWith('.js') && !entry.includes('_FIXTURE')) out.push(path);
    }
  })(root);
  return out.sort();
}

// The YAML-ish frontmatter Test262 puts in every file. Only the fields this
// harvest depends on are read, and only in the shapes the suite actually uses.
export function frontmatter(source) {
  const start = source.indexOf('/*---');
  const end = source.indexOf('---*/');
  if (start < 0 || end < 0) return { negative: null, features: [], flags: [] };
  const block = source.slice(start + 5, end);
  const negativeType = /^\s*type:\s*(\w+)/m.exec(block);
  const negativePhase = /^\s*phase:\s*(\w+)/m.exec(block);
  const featureLine = /^features:\s*\[([^\]]*)\]/m.exec(block);
  const flagLine = /^flags:\s*\[([^\]]*)\]/m.exec(block);
  const split = m => m ? m[1].split(',').map(s => s.trim()).filter(Boolean) : [];
  return {
    negative: /^negative:/m.test(block) && negativeType
      ? { type: negativeType[1], phase: negativePhase ? negativePhase[1] : null }
      : null,
    features: split(featureLine),
    flags: split(flagLine),
  };
}

// Read a single-, double- or backtick-quoted literal starting at `i`, and
// return its *source* text plus the position after it. A template with a
// substitution is rejected: its value is not statically known.
function readStringLiteral(source, i) {
  const quote = source[i];
  if (quote !== '"' && quote !== "'" && quote !== '`') return null;
  let out = '';
  for (let j = i + 1; j < source.length; j++) {
    const ch = source[j];
    if (ch === '\\') { out += ch + source[j + 1]; j++; continue; }
    if (ch === '\n' && quote !== '`') return null;
    if (quote === '`' && ch === '$' && source[j + 1] === '{') return null;
    if (ch === quote) return { raw: out, end: j + 1 };
  }
  return null;
}

// Evaluate a string literal's escapes the way the JavaScript grammar does.
// `JSON.parse` cannot be used: these literals use \x, \u{...}, \0 and single
// quotes that JSON does not accept.
function cook(raw) {
  let out = '';
  for (let i = 0; i < raw.length; i++) {
    if (raw[i] !== '\\') { out += raw[i]; continue; }
    const next = raw[++i];
    switch (next) {
      case 'n': out += '\n'; break;
      case 't': out += '\t'; break;
      case 'r': out += '\r'; break;
      case 'b': out += '\b'; break;
      case 'f': out += '\f'; break;
      case 'v': out += '\v'; break;
      case '0': if (!/[0-9]/.test(raw[i + 1] ?? '')) { out += '\0'; break; } return null;
      case 'x': {
        const hex = raw.slice(i + 1, i + 3);
        if (!/^[0-9a-fA-F]{2}$/.test(hex)) return null;
        out += String.fromCharCode(parseInt(hex, 16)); i += 2; break;
      }
      case 'u': {
        if (raw[i + 1] === '{') {
          const close = raw.indexOf('}', i + 2);
          if (close < 0) return null;
          const hex = raw.slice(i + 2, close);
          if (!/^[0-9a-fA-F]{1,6}$/.test(hex)) return null;
          out += String.fromCodePoint(parseInt(hex, 16)); i = close; break;
        }
        const hex = raw.slice(i + 1, i + 5);
        if (!/^[0-9a-fA-F]{4}$/.test(hex)) return null;
        out += String.fromCharCode(parseInt(hex, 16)); i += 4; break;
      }
      case '\n': break;
      default: out += next;
    }
  }
  return out;
}

// A regular-expression literal, scanned with the grammar's own rules for
// classes and escapes. The caller decides whether a `/` may start one.
function readRegExpLiteral(source, i) {
  let inClass = false;
  for (let j = i + 1; j < source.length; j++) {
    const ch = source[j];
    if (ch === '\\') { j++; continue; }
    if (ch === '\n') return null;
    if (ch === '[') inClass = true;
    else if (ch === ']') inClass = false;
    else if (ch === '/' && !inClass) {
      let k = j + 1;
      while (k < source.length && FLAG_CHARS.includes(source[k])) k++;
      // A following identifier character means this was not a literal.
      if (/[A-Za-z0-9_$]/.test(source[k] ?? '')) return null;
      return { pattern: source.slice(i + 1, j), flags: source.slice(j + 1, k), end: k };
    }
  }
  return null;
}

// Whether a `/` at `i` can begin a regular-expression literal, decided from the
// previous significant token. This is the standard lexical heuristic; the cases
// it refuses are counted rather than guessed at.
function regexAllowed(source, i) {
  let j = i - 1;
  while (j >= 0 && /\s/.test(source[j])) j--;
  if (j < 0) return true;
  const ch = source[j];
  if (/[A-Za-z0-9_$)\]]/.test(ch)) {
    // `return /re/` and `typeof /re/` are the common exceptions.
    const word = /([A-Za-z_$][A-Za-z0-9_$]*)$/.exec(source.slice(0, j + 1));
    return !!word && ['return', 'typeof', 'case', 'in', 'of', 'do', 'else', 'void', 'delete', 'instanceof', 'new', 'throw']
      .includes(word[1]);
  }
  return true;
}

export function extract(source) {
  const found = [];
  const skipped = { dynamicString: 0, ambiguousSlash: 0, uncookable: 0 };
  // `new RegExp(<string>, <string>)` and `RegExp(<string>)`.
  const call = /(?:new\s+)?RegExp\s*\(/g;
  let m;
  while ((m = call.exec(source)) !== null) {
    let i = m.index + m[0].length;
    while (/\s/.test(source[i])) i++;
    const first = readStringLiteral(source, i);
    if (!first) { skipped.dynamicString++; continue; }
    const pattern = cook(first.raw);
    if (pattern === null) { skipped.uncookable++; continue; }
    let j = first.end;
    while (/\s/.test(source[j])) j++;
    let flags = '';
    if (source[j] === ',') {
      j++;
      while (/\s/.test(source[j])) j++;
      const second = readStringLiteral(source, j);
      if (second) {
        const cooked = cook(second.raw);
        if (cooked === null) { skipped.uncookable++; continue; }
        flags = cooked;
      } else if (source[j] !== ')') { skipped.dynamicString++; continue; }
    }
    if (!/^[dgimsuvy]*$/.test(flags)) { skipped.dynamicString++; continue; }
    found.push({ pattern, flags, form: 'constructor' });
  }
  // Regular-expression literals.
  for (let i = 0; i < source.length; i++) {
    const ch = source[i];
    if (ch === '"' || ch === "'" || ch === '`') {
      const str = readStringLiteral(source, i);
      if (str) { i = str.end - 1; continue; }
    }
    if (ch === '/' && source[i + 1] === '/') { i = source.indexOf('\n', i); if (i < 0) break; continue; }
    if (ch === '/' && source[i + 1] === '*') { i = source.indexOf('*/', i) + 1; if (i < 1) break; continue; }
    if (ch !== '/') continue;
    if (!regexAllowed(source, i)) { skipped.ambiguousSlash++; continue; }
    const literal = readRegExpLiteral(source, i);
    if (!literal) { skipped.ambiguousSlash++; continue; }
    found.push({ pattern: literal.pattern, flags: literal.flags, form: 'literal' });
    i = literal.end - 1;
  }
  return { found, skipped };
}

export function harvest(roots, base) {
  const cases = [];
  const totals = { files: 0, dynamicString: 0, ambiguousSlash: 0, uncookable: 0 };
  const seen = new Set();
  for (const root of roots) {
    for (const path of walk(root)) {
      totals.files++;
      const source = readFileSync(path, 'utf8');
      const meta = frontmatter(source);
      const { found, skipped } = extract(source);
      for (const key of Object.keys(skipped)) totals[key] += skipped[key];
      for (const item of found) {
        const key = `${item.pattern} ${item.flags}`;
        if (seen.has(key)) continue;
        seen.add(key);
        cases.push({
          ...item,
          file: relative(base, path),
          negative: meta.negative,
          features: meta.features,
        });
      }
    }
  }
  return { cases, totals };
}
