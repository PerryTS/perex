# Perex development constraints

- This repository is the authoritative reusable engine source; it must build independently of Perry.
- Implement one production compiler/matcher. Other engines may be development references, never production fallbacks.
- Follow `docs/memory-contract.md`: explicit storage ownership, relocation-safe programs, scoped scratch, exact string semantics and offset-only results.
- The initial crate is a scaffold. Do not claim implemented matching, compatibility or performance that has not been measured.
- Keep Perry-specific GC/object code and private application workloads in the host project.
- Preserve upstream provenance and licenses when importing code or tests.
- Run the checks in README for changed components. Matching changes need complete-answer witnesses; memory changes need lifetime/relocation tests.
- Do not update oracle answers to conceal a discrepancy. Preserve failed measurements and mismatches.
