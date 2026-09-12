# V8: backreference into a folded alternative of a scoped modifier group returns no match

Ready to file at <https://issues.chromium.org/issues/new?component=1456824>
(Blink > JavaScript > Regexp). Not yet submitted.

## Summary

After an unrelated regexp workload in the same isolate, V8 returns `null` for
patterns that match. The affected shape is a scoped modifier group whose
alternation captures, where the winning alternative required case folding, and
a backreference then reads that alternative's capture:

```js
new RegExp("(?i:(a)|(b))\\2", "d").exec("BB")
// expected: ["BB", undefined, "B"] at index 0
// actual after the workload: null
```

The same expression is correct in a fresh isolate, and correct for the rest of
the process if it is compiled once before the workload runs. It is the *first*
compilation of the pattern after the workload that produces wrong code.

## Reproduction

`repro.mjs` beside this file is self-contained and needs no dependencies.

```sh
node repro.mjs                # exits 1; four probes return null
node repro.mjs --no-workload  # exits 0; the same four probes are correct
```

Reproduced 5/5 runs on Node v26.5.1, V8 14.6.202.34-node.24, macOS 15 arm64.

The workload is a differential-test case list of about 124,000 regexps. None of
them uses a probe pattern or a probe subject; the list only has to compile
enough regexps first. Two properties of the reproduction are worth noting
because they made it hard to reduce:

- **The probe must not be compiled before the workload.** Adding a single
  `exec` of the probe pattern at the top of the script makes the probe answer
  correctly for the remainder of the process.
- **The workload resists reduction.** Removing any one of its generator
  dimensions, or replacing it with an equivalent count of other regexps, stops
  the reproduction. A prefix of roughly 108,000 cases is the threshold, and the
  exact threshold moves between runs, so it appears to be a resource or cache
  boundary rather than one poisoned pattern.

## Affected and unaffected expressions

Each row is compiled for the first time after the workload. Expected values are
what a fresh isolate returns, and what the specification requires.

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

## Where the wrong answer comes from

| Configuration | Result |
|---|---|
| default | wrong (3/3 runs) |
| `--regexp-interpret-all` | correct (3/3 runs) |
| `--jitless` | correct (3/3 runs) |
| `--no-regexp-tier-up` | wrong |
| `--no-opt` | wrong |

Both configurations that fix it force the RegExp interpreter, so the
interpreter is correct and the natively compiled code is wrong.

`--no-regexp-optimization` is **not** a reliable mitigation: it makes an earlier
draft of this reproduction pass but not the committed one, so it only moves the
threshold.

## Correctness of the expected answer

For `/(?i:(a)|(b))\2/d` on `"BB"`: inside `(?i: … )` the first alternative `(a)`
does not match `B` under folding; the second alternative `(b)` does, capturing
`B` into group 2. The backreference `\2` then compares that capture against the
following `B` and succeeds, giving a match of `"BB"` at index 0 with group 1
undefined and group 2 `"B"`.

Independently: a fresh V8 isolate returns exactly this, and so does an
unrelated ECMAScript regexp engine developed against Node as its oracle, which
is how the difference was found.

## How it was found

Perex, an independent ECMAScript regexp engine, uses Node as a differential
oracle. Eight cases in its scoped-modifier suite reported Node returning
no-match where Perex returned a match. The cases were re-checked in a clean
process, where Node agreed with Perex, which identified the oracle rather than
the engine as the source of the difference.

## Not tested

Other V8 versions, `d8`, other platforms, and whether the Node 26.8.1 build
used in this project's CI is affected. No upstream issue was found for this
shape.
