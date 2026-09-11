//! Resumable necessary-condition search through original subject storage.
use super::*;

impl Vm<'_, '_, '_, '_> {
    fn needle(&self) -> &[u8] {
        match &self.state.work {
            Work::Needle { bytes, len, .. } => &bytes[..*len as usize],
            _ => unreachable!("admission literal buffer is not active"),
        }
    }
    fn needle_fold(&self) -> bool {
        match self.state.work {
            Work::Needle { fold, .. } => fold,
            _ => unreachable!("admission literal buffer is not active"),
        }
    }
    fn required(&self, index: usize) -> u32 {
        let (pc, reverse) = self.program.admission().unwrap();
        self.program.instruction(
            pc + if reverse {
                self.needle().len() - 1 - index
            } else {
                index
            },
        )[1]
    }
    pub(super) fn admit_step(&mut self, available: usize) -> Result<(), ExecError> {
        match self.state.phase {
            Phase::Admission => {
                if self.input.len_utf16() < 64 || self.program.words[2] & ADMISSION == 0 {
                    self.state.phase = Phase::Start;
                    return Ok(());
                }
                let (pc, _) = self.program.admission().ok_or(ExecError::InvalidProgram)?;
                let [op, a, b] = self.program.instruction(pc);
                if matches!(op, CLASS | CLASS_I | CLASS_SORTED | CLASS_SORTED_I) {
                    if b == 1 && op == CLASS {
                        let [lo, hi] = self.program.range(a as usize);
                        if hi < 128 && lo <= hi && self.input.original_bytes().is_some() {
                            self.state.phase = Phase::AdmitByteClass {
                                offset: 0,
                                lo: lo as u8,
                                hi: hi as u8,
                            };
                            return Ok(());
                        }
                    }
                    self.cursor = self.input.cursor();
                    self.state.phase = Phase::AdmitClass;
                    return Ok(());
                }
                let len = self.program.literal_run(pc);
                self.charge(len)?;
                self.state.work = Work::Needle {
                    bytes: [0; ADMISSION_MAX],
                    len: len as u8,
                    fold: op == CHAR_I,
                };
                let mut ascii = true;
                for i in 0..len {
                    let c = self.required(i);
                    if c >= 128 {
                        ascii = false;
                        break;
                    }
                    let Work::Needle { bytes, .. } = &mut self.state.work else {
                        unreachable!()
                    };
                    bytes[i] = c as u8;
                }
                let bytes = if op == CHAR_I {
                    self.input.ascii_bytes()
                } else {
                    self.input.original_bytes()
                };
                if ascii && bytes.is_some() {
                    self.state.phase = Phase::AdmitBytes { offset: 0 };
                } else {
                    self.cursor = self.input.cursor_at(self.input.len_utf16()).unwrap();
                    self.state.phase = Phase::AdmitSuffix(len);
                }
            }
            Phase::AdmitByteClass { offset, lo, hi } => {
                let bytes = self
                    .input
                    .original_bytes()
                    .ok_or(ExecError::ChangedResources)?;
                if offset == bytes.len() {
                    self.state.phase = Phase::Finished(false);
                } else {
                    let end = bytes.len().min(offset + 256);
                    self.charge(end - offset)?;
                    self.state.phase = if bytes[offset..end].iter().any(|&b| b >= lo && b <= hi) {
                        Phase::Start
                    } else {
                        Phase::AdmitByteClass {
                            offset: end,
                            lo,
                            hi,
                        }
                    };
                }
            }
            Phase::AdmitBytes { offset } => {
                let fold = self.needle_fold();
                let bytes = if fold {
                    self.input.ascii_bytes()
                } else {
                    self.input.original_bytes()
                }
                .ok_or(ExecError::ChangedResources)?;
                if offset == bytes.len() {
                    self.state.phase = Phase::Finished(false);
                } else {
                    let end = bytes.len().min(offset + 256);
                    self.charge(end - offset)?;
                    let len = self.needle().len();
                    let mut at = offset;
                    while let Some(hit) = bytes[at..end].iter().position(|&b| {
                        if fold {
                            b.eq_ignore_ascii_case(&self.needle()[0])
                        } else {
                            b == self.needle()[0]
                        }
                    }) {
                        at += hit;
                        self.charge(len)?;
                        let needle = self.needle();
                        if let Some(window) = bytes.get(at..at + len)
                            && if fold {
                                window.eq_ignore_ascii_case(needle)
                            } else {
                                window == needle
                            }
                        {
                            self.state.phase = Phase::Start;
                            return Ok(());
                        }
                        at += 1;
                    }
                    self.state.phase = Phase::AdmitBytes { offset: end };
                }
            }
            Phase::AdmitClass => {
                if let Some(c) = read(&mut self.cursor, self.program.unicode(), false) {
                    self.charge(1)?;
                    let (pc, _) = self.program.admission().ok_or(ExecError::InvalidProgram)?;
                    let [op, a, b] = self.program.instruction(pc);
                    self.begin_class(
                        a,
                        b,
                        c,
                        ClassUse::Admission,
                        matches!(op, CLASS_I | CLASS_SORTED_I),
                        matches!(op, CLASS_SORTED | CLASS_SORTED_I),
                    );
                    self.class_step(available.saturating_sub(1))?;
                } else {
                    self.state.phase = Phase::Finished(false);
                }
            }
            Phase::AdmitSuffix(index) => {
                if index == 0 {
                    self.state.phase = Phase::Start;
                } else {
                    self.charge(1)?;
                    let required = self.required(index - 1);
                    let c = read(&mut self.cursor, self.program.unicode(), true);
                    if c.is_some_and(|c| equal(self.program, c, required, self.needle_fold())) {
                        self.state.phase = Phase::AdmitSuffix(index - 1);
                    } else {
                        self.cursor = self.input.cursor();
                        self.state.phase = Phase::AdmitScan;
                    }
                }
            }
            Phase::AdmitScan => {
                if let Some(c) = read(&mut self.cursor, self.program.unicode(), false) {
                    self.charge(1)?;
                    if equal(self.program, c, self.required(0), self.needle_fold()) {
                        self.state.phase = Phase::AdmitProbe {
                            index: 1,
                            scan: self.cursor.mark(),
                        };
                    }
                } else {
                    self.state.phase = Phase::Finished(false);
                }
            }
            Phase::AdmitProbe { index, scan } => {
                if index == self.needle().len() {
                    self.state.phase = Phase::Start;
                } else {
                    self.charge(1)?;
                    let required = self.required(index);
                    if read(&mut self.cursor, self.program.unicode(), false)
                        .is_some_and(|c| equal(self.program, c, required, self.needle_fold()))
                    {
                        self.state.phase = Phase::AdmitProbe {
                            index: index + 1,
                            scan,
                        };
                    } else {
                        self.restore(scan);
                        self.state.phase = Phase::AdmitScan;
                    }
                }
            }
            _ => return Err(ExecError::InvalidProgram),
        }
        Ok(())
    }
}
