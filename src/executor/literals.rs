//! Execute adjacent exact literal and capture-save instructions over original
//! ASCII storage. This view is scoped to one advance; pauses retain offsets.
use super::*;

impl Vm<'_, '_, '_, '_> {
    pub(super) fn literal_run<const PAUSABLE: bool>(
        &mut self,
        mut op: u32,
        mut operand: u32,
        bytes: &[u8],
        quantum_end: usize,
    ) -> Result<Step, ExecError> {
        let mut position = self.cursor.position();
        let mut pc = self.state.pc;
        let before = self.budget.remaining();
        let mut remaining = before;
        let instructions = self.sections.instructions;
        let result = (|| loop {
            // The caller fetched and paid the first opcode. A failed
            // comparison consumes its character; the end consumes none.
            if op == CHAR {
                let Some(&actual) = bytes.get(position) else {
                    return Ok(Step::Fail);
                };
                position += 1;
                if u32::from(actual) != operand {
                    return Ok(Step::Fail);
                }
            } else if let Err(error) = self.store(operand as usize, position) {
                // Retry exactly the paid SAVE after its undo storage grows.
                if error == ExecError::Undo {
                    self.state.phase = Phase::Execute {
                        op: SAVE,
                        a: operand,
                        b: 0,
                    };
                }
                return Err(error);
            }
            // Each opcode in this loop charges exactly one. Its paid entry
            // was inside the trial window, so it reaches (never skips) this
            // boundary; a larger-than-budget quantum uses the unpaused path.
            if PAUSABLE && remaining == quantum_end {
                return Ok(Step::Next);
            }
            let Some(&[next @ (CHAR | SAVE), argument, _]) = instructions.get(pc) else {
                return Ok(Step::Next);
            };
            // Pay each existing opcode before advancing its PC, including an
            // opcode whose comparison or store fails. No host callback occurs
            // in this scoped loop; publish its exact work on every exit.
            if remaining == 0 {
                return Err(ExecError::WorkLimit);
            }
            remaining -= 1;
            pc += 1;
            op = next;
            operand = argument;
        })();
        self.state.pc = pc;
        self.charge(before - remaining)?;
        // An ASCII byte position is a UTF-16 position and needs no prefix walk.
        self.cursor = self
            .input
            .cursor_at(position)
            .ok_or(ExecError::InvalidProgram)?;
        result
    }
}
