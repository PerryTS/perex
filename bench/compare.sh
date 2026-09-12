#!/bin/sh
# perex vs V8 on the same cases, same machine, adjacent in time.
# Several passes; the minimum per case is used, which on a contended machine is
# the estimate least polluted by other load.
set -e
DIR=$(cd "$(dirname "$0")" && pwd)
OUT=${1:-/tmp/perex-vs-node}
PASSES=${2:-3}
mkdir -p "$OUT"
cargo build --release --quiet 2>/dev/null || cargo build --release
"$DIR/target/release/perex-bench" --emit-cases > "$OUT/cases.json"
i=0
while [ "$i" -lt "$PASSES" ]; do
  "$DIR/target/release/perex-bench" > "$OUT/perex.$i.txt"
  node "$DIR/node-bench.mjs" "$OUT/cases.json" > "$OUT/node.$i.txt"
  i=$((i + 1))
done
python3 - "$OUT" "$PASSES" <<'PY'
import re, sys, glob
out, passes = sys.argv[1], int(sys.argv[2])
def parse(paths, col):
    best = {}
    for path in paths:
        for line in open(path):
            f = line.split()
            if len(f) > col and re.match(r'^[a-z0-9-]+$', f[0]):
                try: v = float(f[col])
                except ValueError: continue
                if f[0] not in best or v < best[f[0]]: best[f[0]] = v
    return best
perex = parse(sorted(glob.glob(out+"/perex.*.txt")), 1)
test = parse(sorted(glob.glob(out+"/node.*.txt")), 1)
exec_ = parse(sorted(glob.glob(out+"/node.*.txt")), 2)
# `find` always produces captures, which `test` does not and `exec` does; `exec`
# also allocates substrings, which `find` does not. The comparison is therefore
# bracketed by the two rather than being a single number.
rows = sorted(((k, perex[k], test[k], exec_[k]) for k in perex if k in test and k in exec_),
              key=lambda r: -(r[1]/r[2]))
print(f"{'case':<24}{'perex ns':>13}{'V8 test':>13}{'V8 exec':>13}{'vs test':>10}{'vs exec':>10}")
print("-" * 83)
behind_both = []
for k, p, t, e in rows:
    rt, re_ = p / t, p / e
    if rt > 1.0 and re_ > 1.0: behind_both.append(k)
    mark = '  <--' if rt > 1.0 and re_ > 1.0 else ('  ~' if rt > 1.0 else '')
    print(f"{k:<24}{p:13.1f}{t:13.1f}{e:13.1f}{rt:9.2f}x{re_:9.2f}x{mark}")
print()
print(f"behind V8 on both entry points in {len(behind_both)} of {len(rows)} cases")
print("<-- behind both;  ~ behind the boolean test only, at or ahead of the capturing one")
PY
