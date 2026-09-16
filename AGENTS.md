# Perex development constraints

- This repository is the authoritative reusable engine source; it must build independently of Perry.
- Implement one production compiler/matcher. Other engines may be development references, never production fallbacks.
- Follow `docs/memory-contract.md`: explicit storage ownership, relocation-safe programs, scoped scratch, exact string semantics and offset-only results.
- The compiler/matcher, input layer and resumable execution are implemented and are Perry's production regex engine. The `v` flag is implemented in full, properties of strings included. Do not claim matching, compatibility or performance that has not been measured.
- Traverse the host's original subject without copying it or creating a UTF-16 conversion buffer. Integer spans remain the result contract.
- Keep Perry-specific GC/object code and private application workloads in the host project.
- Preserve upstream provenance and licenses when importing code or tests.
- Run the checks in README for changed components. Matching changes need complete-answer witnesses; memory changes need lifetime/relocation tests.
- Do not update oracle answers to conceal a discrepancy. Preserve failed measurements and mismatches.

- Keep strict full-reference failures visible. Development exception lists bind exact cases/answers and evidence; they are not adoption exclusions.
