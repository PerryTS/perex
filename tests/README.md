# Semantic reference fixtures

`fixtures/core-cases.json` is a small authored suite of engine-boundary cases. Each case has a unique `id`, exact JS `source`/`flags`/`subject`, and `start_utf16`. JSON strings can contain escaped lone surrogates; do not normalize them through a lossy Unicode conversion.

The Node reference uses RegExp.exec once per case. Accepted patterns include `d` to observe all capture spans. `g` or `y` expresses nonzero-start behavior in this adapter. The engine result schema deliberately excludes lastIndex and JS result-object ownership: those belong to host integration tests.

Each JSONL answer is one of:

```json
{"id":"example","outcome":"syntax-error"}
{"id":"example","outcome":"no-match"}
{"id":"example","outcome":"match","captures":[{"span":[0,1],"units":[97]},null],"groups":null}
```

Capture zero is the entire match. Other captures follow numerical order. `null` means unset; an empty participating capture has equal span endpoints and `units: []`. Spans use UTF-16 units, end-exclusive. Named groups are a name-sorted array of `{name, capture}` objects, with the same capture representation. No-match and syntax error are different outcomes. Error-message wording is not compared.

The initial fixtures cover surrogate modes/positions, captures/assertions, Unicode sets/properties, line terminators, legacy escapes, invalid syntax/flags and capture-capacity boundaries. They are not comprehensive conformance or resource-limit tests. Global iteration, replacements, callbacks, split, object overrides and GC behavior require host tests.

`fixtures/reference.json` binds fixture/answer hashes and records the Node/V8/Unicode versions used to generate the answers. The comparator rejects missing, duplicate, extra or different answers. A future Perex harness must emit this schema; current CI tests the reference and comparator only.
