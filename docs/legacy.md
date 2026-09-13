# Legacy escapes and quantified assertions

The same parser and evaluator implement [ECMAScript Annex B pattern grammar](https://tc39.es/ecma262/multipage/additional-ecmascript-features-for-web-browsers.html#sec-regular-expressions-patterns). This is JavaScript's non-Unicode grammar, with explicit Unicode-mode syntax errors where that grammar is disallowed.

## Numeric escapes

A decimal escape beginning with 1–9 is a backreference when its full value identifies a capture anywhere in the pattern. That includes forward references, open self references and named capturing groups. Escaped parentheses, class contents, noncapturing groups and assertion delimiters do not add captures.

If that value cannot identify a capture, legacy parsing consumes an octal escape or the identity-escaped digit 8/9. A leading octal digit 0–3 permits at most three digits; 4–7 permits at most two. The remaining characters are ordinary pattern syntax: `\400+` means a space followed by one or more `0` characters. It must not quantify an invented character numbered 256 or treat the complete decimal text as one atom. A leading zero never becomes a backreference. Inside a character class numeric escapes are characters, never references.

The compiler determines this before constructing and quantifying the atom. It scans the full pattern only when an unresolved reference needs the total capture count, sharing that scan with legacy named-escape detection. Already-declared references use the current count. The scan walks original pattern storage, caches only scalar counts and consumes the compilation budget. Very long decimal text can be known to exceed the representable capture count without integer overflow; legacy parsing then leaves its ordinary suffix intact.

In Unicode mode only a valid numeric reference or `\0` without a following decimal digit is accepted. Out-of-range references and class numeric escapes are syntax errors.

## Control prefixes and lookahead

ASCII letters after `\c` produce their control character. A legacy class additionally accepts decimal digits and `_`. Other legacy `\c` sequences consume only the backslash, leaving `c` as its own character/atom. This preserves behavior such as `\c*`, which matches a literal backslash followed by zero or more `c` characters. Unicode mode rejects invalid control escapes.

Legacy lookahead can be quantified. The existing counted-repeat and assertion instructions implement that behavior, including captures, optional/required repetitions, zero progress and rollback. Quantified lookbehind, and quantified assertions in Unicode mode, are syntax errors. No additional execution engine or instruction format is introduced.

## Evidence and scope

`tools/check-legacy.mjs` compares 112,748 complete answers with Node 26.8.1, with no differences or exception list. Inputs cover 1,047 numeric spellings, octal boundaries, long decimal text, class/range contexts, forward and named counts, raw control-prefix suffixes across 256 byte values, adjacent quantifiers, quantified assertions and capture indices above 254. Expected results come from Node execution, not from Perex's parser or the subject generator.

Three Rust tests pin numeric atom boundaries, capture-count distinctions, control prefixes and quantified assertions. Existing name, core, case-folding, input and resource/lifetime tests remain required. A mutant that consumes a third octal digit after 4–7 fails the atom-boundary witness.

Program format and matching storage are unchanged. This is additional correctness coverage, not CPU/RSS evidence or complete ECMAScript conformance. Unicode-sets operators and string properties under `v`, and structured fuzzing, remain outstanding.
