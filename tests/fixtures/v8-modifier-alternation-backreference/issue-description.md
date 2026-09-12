## Summary

V8 returns `null` for a RegExp that matches, when the pattern is compiled for
the first time after an unrelated regexp workload in the same isolate.

```js
new RegExp("(?i:(a)|(b))\\2", "d").exec("BB")
// expected: ["BB", undefined, "B"] at index 0
// actual:   null
```

The affected shape is a scoped modifier group whose alternation captures, where
the winning alternative required case folding, and a backreference then reads
that alternative's capture.

The same expression is correct in a fresh isolate, and correct for the rest of
the process if it is executed once *before* the workload. It is the first
compilation after the workload that produces wrong code.

## Steps to reproduce

The attached `repro.mjs` is self-contained and has no dependencies.

```
node repro.mjs                # exits 1; four probes return null
node repro.mjs --no-workload  # exits 0; the same four probes are correct
```

Reproduced 5 out of 5 runs.

**Environment:** Node v26.5.1, V8 14.6.202.34-node.24, macOS (Darwin 25.5.0), arm64.

## Which path produces the wrong answer

| Configuration | Result |
|---|---|
| default | wrong (3/3 runs) |
| `--regexp-interpret-all` | correct (3/3 runs) |
| `--jitless` | correct (3/3 runs) |
| `--no-regexp-tier-up` | wrong |
| `--no-opt` | wrong |

Both configurations that fix it force the RegExp interpreter, so the interpreter
is correct and the natively compiled code is wrong.

`--no-regexp-optimization` is **not** a reliable mitigation: it makes an earlier
draft of this reproduction pass but not the attached one, so it only moves the
threshold.

## Affected and unaffected expressions

Each row is compiled for the first time after the workload. "Expected" is what a
fresh isolate returns, and what the specification requires.

| Pattern | Subject | Expected | Actual |
|---|---|---|---|
| `/(?i:(a)\|(b))\2/d` | `"BB"` | `0:"BB"` | `null` |
| `/(?i:(?<x>a)\|(?<x>b))\k<x>/d` | `"BB"` | `0:"BB"` | `null` |
| `/(?i:(?<x>a)\|(?<x>b))(?-i:\k<x>)/d` | `"aBBB"` | `1:"BB"` | `null` |
| `/(?i:(?<x>a)\|(?<x>b))(?-i:\k<x>)/d` | `"0"×80 + "aBBB"` | `81:"BB"` | `null` |
| `/(?i:(a)\|(b))\2/d` | `"bb"` | `0:"bb"` | `0:"bb"` |
| `/(?i:(a)\|(b))\1/d` | `"AA"` | `0:"AA"` | `0:"AA"` |
| `/((a)\|(b))\3/d` | `"bb"` | `0:"bb"` | `0:"bb"` |
| `/(?i:(a))\1/d` | `"aa"` | `0:"aa"` | `0:"aa"` |

Every ingredient appears to be required. Removing the scoped modifier, taking
the first alternative instead of a later one, or matching without case folding
all keep the correct answer. Named and numbered capture groups are equally
affected, so this is not specific to duplicate named capture groups. Subject
length is not a factor: a two-character subject fails.

## Why the expected answer is correct

For `/(?i:(a)|(b))\2/d` on `"BB"`: inside `(?i: … )` the first alternative `(a)`
does not match `B` under folding; the second alternative `(b)` does, capturing
`B` into group 2. The backreference `\2` then compares that capture against the
following `B` and succeeds, giving `"BB"` at index 0 with group 1 undefined and
group 2 `"B"`.

A fresh V8 isolate returns exactly this.

## Notes for anyone reproducing this

Two properties made this hard to reduce and are worth knowing up front:

- **The pattern must not be compiled before the workload.** Adding a single
  `exec` of a probe pattern at the top of the script makes that probe answer
  correctly for the remainder of the process.
- **The workload resists reduction.** It is a list of about 124,000 regexps,
  none of which uses a probe pattern or a probe subject; it only has to compile
  enough regexps first. Removing any one of its generator dimensions, or
  substituting an equal count of other regexps, stops the reproduction.
  Automated chunk removal (halves down to 1/32 of the list) removed nothing. A
  prefix of roughly 108,000 cases is the threshold, and the exact threshold
  moves between runs, so this looks like a resource or cache boundary rather
  than one poisoned pattern.

## How it was found

An independent ECMAScript regexp engine uses Node as a differential oracle.
Eight cases in its scoped-modifier suite reported Node returning no-match where
the engine returned a match. Re-checking those cases in a clean Node process
showed Node agreeing with the engine, which identified the oracle rather than
the engine as the source of the difference.

## Not tested

Other V8 versions, `d8`, other platforms, and other Node builds. No existing
upstream issue was found for this shape.
