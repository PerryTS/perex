# Implementation milestones

These milestones remain the full implementation scope. Input-layer progress does not establish matching, host integration or performance.

1. **Exact input and result model — in progress.** Lossless code-unit/code-point cursors, reverse traversal, half-pair positions, no-copy span access and scoped-borrow relocation tests are implemented. Initial cursor CPU/RSS diagnostics have run; efficient repeated validation/resumption under the host's string invariants, and actual GC integration remain outstanding. No converted subject buffer is permitted for production traversal.
2. **Compiler and program format.** Select reusable source material, preserve provenance, pin grammar/Unicode versions, support caller-owned compile scratch and validated relocatable output. Resolve capture count/operand widths consistently.
3. **One evaluator.** Implement ordered JS matching, quantified capture reset, assertions/backreferences, caller-controlled execution state and explicit exhaustion. Add deterministic semantic witnesses as features become available; never silently fall back to another engine.
4. **Perry adapter.** Traced program reference, safe input borrows, collecting/reentrant operations, result materialization and all JS operation paths. This implementation belongs in Perry, not this crate.
5. **Conformance and reliability.** Relevant Test262 coverage, structured differential fuzzing, resource-limit tests and host GC/lifetime witnesses. Reference fixture checks alone do not satisfy this milestone.
6. **Performance and adoption.** Native microbenchmarks, program/scratch accounting and controlled whole-application CPU/RSS comparisons. Meet the per-case CPU and RSS requirements in `performance.md`; aggregate wins cannot conceal losing cases. Replace Perry's old routes only after the integrated candidate passes the gates.
7. **Reusable release and later compilation tiers.** Stabilize an embedding API and an owned-buffer convenience API; consider native/AOT compilation only with explicit memory and compatibility evidence.
