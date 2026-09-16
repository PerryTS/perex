//! Emission uses only the prepared arena and budget, never an input view.
use super::*;

impl core::fmt::Debug for Prepared<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Prepared")
            .field("required_words", &self.size)
            .field("remaining_work", &self.budget.remaining())
            .finish_non_exhaustive()
    }
}

impl Prepared<'_> {
    /// Exact final storage, including names, ranges and repetition metadata.
    pub fn required_words(&self) -> usize {
        self.size
    }

    pub fn remaining_work(&self) -> usize {
        self.budget.remaining()
    }

    pub(super) fn step(&mut self) -> Result<(), CompileError> {
        self.budget.charge(1).map_err(|_| CompileError::WorkLimit)
    }

    /// Emit once into caller storage and release every parse-scratch borrow.
    /// The returned program borrows only output. Insufficient storage consumes
    /// the plan too; query the exact size before allocating. Every error leaves
    /// the output's first word invalid, so no partial program is published.
    pub fn emit<'p>(self, output: &'p mut [u32]) -> Result<Program<'p>, CompileError> {
        if let Some(first) = output.first_mut() {
            *first = 0;
        }
        let size = self.size;
        if output.len() < size {
            return Err(CompileError::ProgramStorage {
                required_words: size,
            });
        }
        let output = &mut output[..size];
        if let Err(error) = self.emit_into(output) {
            output[0] = 0;
            return Err(error);
        }
        // emit_into has validated these exact immutable words under the same
        // operation budget. No callback or write occurs between that and here.
        Ok(Program { words: output })
    }

    /// The filter an operator left on one group of a property of strings: one
    /// bit per member, in the group's order, set when that member survived.
    /// The members are read from the shared table and compared against the
    /// operands recorded beside the group, never copied into the program.
    fn write_filter(
        &mut self,
        output: &mut [u32],
        base: usize,
        node: Node,
    ) -> Result<(), CompileError> {
        if node.b == 0 {
            return Ok(());
        }
        let (set, group) = sequence_group(node.a);
        let (start, end) = match group {
            None => (0, crate::sequences::count(set)),
            Some(group) => crate::sequences::group(set, group),
        };
        let at = base + node.b as usize - 1;
        output[at..at + (end - start).div_ceil(32)].fill(0);
        for (bit, index) in (start..end).enumerate() {
            self.step()?;
            let member = crate::sequences::member(set, index);
            if super::sets::passes(self.nodes, self.budget, node.c, member, node.flags & I != 0)? {
                output[at + bit / 32] |= 1 << (bit % 32);
            }
        }
        Ok(())
    }

    fn emit_into(mut self, output: &mut [u32]) -> Result<(), CompileError> {
        let (root, count, bits, base_size) = (self.root, self.count, self.flags, self.base_size);
        let parser = &mut self;
        parser.write_names(output, base_size)?;
        // Emitter walks the topologically ordered arena backwards. It does not use
        // the native stack for a long sequence or expand counted repetitions.
        parser.nodes[root as usize].start = 1;
        let filters =
            HEADER + count as usize * 3 + parser.range_used * 2 + parser.repeats as usize * 8;
        let mut repeat_id = 0;
        let write = |out: &mut [u32], pc: u32, op: u32, a: u32, b: u32| {
            let at = HEADER + pc as usize * 3;
            out[at..at + 3].copy_from_slice(&[op, a, b]);
        };
        // Address zero belongs to the leading `SAVE`, so a node still holding
        // it is one no alternative reaches: a member an operator removed, or a
        // set kept only to compare against. It becomes no instruction.
        for i in (0..parser.used).rev() {
            parser.step()?;
            let n = parser.nodes[i];
            let pc = n.start;
            if pc == 0 {
                continue;
            }
            let mut child = |id: u32, start: u32, reverse: bool| {
                parser.nodes[id as usize].start = start;
                parser.nodes[id as usize].reverse = reverse;
            };
            match n.kind {
                EMPTY | NAME_META | NAME_DECL => {}
                WRAP => child(n.a, pc, n.reverse),
                SEQ => {
                    let (a, b) = if n.reverse { (n.b, n.a) } else { (n.a, n.b) };
                    let len = parser.nodes[a as usize].len;
                    parser.nodes[a as usize].start = pc;
                    parser.nodes[a as usize].reverse = n.reverse;
                    parser.nodes[b as usize].start = pc + len;
                    parser.nodes[b as usize].reverse = n.reverse;
                }
                ALT => {
                    let len = parser.nodes[n.a as usize].len;
                    let right = pc + len + 2;
                    parser.nodes[n.a as usize].start = pc + 1;
                    parser.nodes[n.a as usize].reverse = n.reverse;
                    parser.nodes[n.b as usize].start = right;
                    parser.nodes[n.b as usize].reverse = n.reverse;
                    write(output, pc, SPLIT, pc + 1, right);
                    write(output, right - 1, JUMP, pc + n.len, 0);
                }
                GROUP => {
                    child(n.a, pc + 1, n.reverse);
                    write(output, pc, SAVE, n.b * 2 + u32::from(n.reverse), 0);
                    write(
                        output,
                        pc + n.len - 1,
                        SAVE,
                        n.b * 2 + u32::from(!n.reverse),
                        0,
                    );
                }
                ASSERT => {
                    child(n.a, pc + 1, n.flags & 2 != 0);
                    write(output, pc, ASSERT, pc + n.len, n.flags);
                    write(output, pc + n.len - 1, ASSERT_END, 0, 0);
                }
                REPEAT => {
                    child(n.a, pc + 3, n.reverse);
                    for (delta, op) in [
                        (0, REPEAT_INIT),
                        (1, REPEAT_CHOICE),
                        (2, REPEAT_BODY),
                        (n.len - 1, REPEAT_NEXT),
                    ] {
                        write(output, pc + delta, op, repeat_id, 0);
                    }
                    let at = HEADER
                        + count as usize * 3
                        + parser.range_used * 2
                        + repeat_id as usize * 8;
                    output[at..at + 8].copy_from_slice(&[
                        n.b,
                        n.c,
                        n.flags,
                        pc + 2,
                        pc + n.len,
                        n.first,
                        n.end,
                        0,
                    ]);
                    repeat_id += 1;
                }
                SEQSET if n.len != 0 => {
                    write(output, pc, modified(SEQUENCE, n.flags), n.a, n.b);
                    parser.write_filter(output, filters, n)?;
                }
                SEQSET | STRING | FILTER => {}
                op => write(output, pc, modified(op, n.flags), n.a, n.b),
            }
        }
        write(output, 0, SAVE, 0, 0);
        write(output, count - 2, SAVE, 1, 0);
        write(output, count - 1, MATCH, 0, 0);
        let start = HEADER + count as usize * 3;
        for (i, range) in parser.ranges[..parser.range_used].iter().enumerate() {
            parser
                .budget
                .charge(1)
                .map_err(|_| CompileError::WorkLimit)?;
            output[start + i * 2..start + i * 2 + 2].copy_from_slice(&[range.lo, range.hi]);
        }
        output[..HEADER].copy_from_slice(&[
            MAGIC,
            VERSION,
            bits | if parser.name_count != 0 { NAMES } else { 0 },
            parser.captures,
            count,
            parser.range_used as u32,
            parser.repeats,
            0,
            0,
            0,
            parser.filter_words as u32,
        ]);
        // Preserve the existing atom instruction and its admission-hint address.
        // The entry selects a bounded retry record in the same evaluator; keeping
        // the generic body words avoids a second program or an AST relocation pass.
        for i in 0..parser.repeats as usize {
            parser.step()?;
            let p = Program { words: output };
            let r = p.repeat(i);
            let entry = r[3] as usize - 2;
            if r[4] as usize == entry + 5 && r[5] == r[6] && consuming(p.instruction(entry + 3)[0])
            {
                output[HEADER + entry * 3] = ATOM_REPEAT;
                let at = HEADER + count as usize * 3 + parser.range_used * 2 + i * 8;
                output[at + 7] = 1;
            }
        }
        if parser.repeats != 0 {
            let hint = parser.admission(Program { words: output }, root)?;
            output[2] |= hint;
        }
        parser.candidate()?;
        output[7] = parser.candidate_descriptor(root);
        output[8] = parser.end_candidate_descriptor(root);
        // Derived from the emitted instructions with the same function the
        // validator re-runs, so the stored word cannot disagree with the program.
        output[9] = crate::program::derive_run_skip(output, Program { words: output })
            | crate::program::derive_leading(output, count as usize)
            | if parser.forward {
                crate::program::ADMISSION_FORWARD
            } else {
                0
            };
        Program::from_words(output, parser.budget)
            .map(|_| ())
            .map_err(|e| match e {
                ProgramError::WorkLimit => CompileError::WorkLimit,
                ProgramError::Invalid => CompileError::InvalidProgram,
            })
    }
}
