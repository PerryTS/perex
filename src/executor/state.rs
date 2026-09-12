use super::*;

#[derive(Clone, Copy)]
pub(super) struct AtomState {
    pub minimum_end: usize,
    pub needed: u32,
    pub remaining: u32,
    pub before: Mark,
}

// Admission is complete before the first instruction executes. Its literal
// buffer and an active atom scan therefore never need storage simultaneously.
#[derive(Clone, Copy)]
pub(super) enum Work {
    Idle,
    Needle {
        bytes: [u8; ADMISSION_MAX],
        len: u8,
        fold: bool,
    },
    Atom(AtomState),
}

#[derive(Clone, Copy)]
pub(super) enum ClassUse {
    Trial,
    Admission,
    AtomScan,
    AtomExtend,
}

#[derive(Clone, Copy)]
pub(super) enum AfterRollback {
    Trial,
    Fail,
    AtomRetreat,
    AtomExtend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Shape {
    pub header: [u32; HEADER],
    pub words: usize,
    pub input: (u8, usize, usize),
}
impl Shape {
    pub fn new(p: Program<'_>, input: Input<'_>) -> Self {
        Self {
            header: p.words[..HEADER].try_into().unwrap(),
            words: p.words.len(),
            input: input.shape(),
        }
    }
    pub fn registers(self) -> usize {
        (self.header[3] as usize + self.header[6] as usize) * 2
    }
}
#[derive(Clone, Copy)]
pub(super) enum AfterSeek {
    Start,
    Backref {
        start: usize,
        end: usize,
        matched: Mark,
    },
}
#[derive(Clone, Copy)]
pub(super) enum Phase {
    Admission,
    /// Rebuild the condition literal before resuming its search mid-match.
    BoundPrepare {
        from: usize,
    },
    /// Resume the condition search to bound later start positions.
    BoundScan {
        offset: usize,
    },
    AdmitBytes {
        offset: usize,
    },
    AdmitByteClass {
        offset: usize,
        lo: u8,
        hi: u8,
    },
    AdmitClass,
    AdmitSuffix(usize),
    AdmitScan,
    AdmitProbe {
        index: usize,
        scan: Mark,
    },
    Start,
    Seek {
        target: usize,
        after: AfterSeek,
    },
    Candidate,
    Initialize(usize),
    Trial,
    Execute {
        op: u32,
        a: u32,
        b: u32,
    },
    Class {
        index: u32,
        end: u32,
        negated: bool,
        sorted: bool,
        values: [u32; 4],
        context: ClassUse,
    },
    AtomScan,
    AtomExtend,
    AtomResult {
        matched: bool,
        extend: bool,
    },
    AtomCommit,
    AtomRetreat,
    Named {
        group: usize,
        next: usize,
        selected: usize,
    },
    Backref {
        start: usize,
        end: usize,
        captured: Mark,
        fold: bool,
    },
    Clear {
        next: usize,
        end: usize,
        slot: usize,
    },
    ClearStore {
        next: usize,
        end: usize,
        slot: usize,
    },
    Fail,
    Rollback {
        until: usize,
        after: AfterRollback,
    },
    NextStart,
    Validate(usize),
    Finished(bool),
    Failed(ExecError),
}
impl Phase {
    pub fn outcome(self) -> Option<Result<Progress, ExecError>> {
        match self {
            Self::Finished(true) => Some(Ok(Progress::Matched)),
            Self::Finished(false) => Some(Ok(Progress::NoMatch)),
            Self::Failed(error) => Some(Err(error)),
            _ => None,
        }
    }
}
#[derive(Clone, Copy)]
pub(super) struct State {
    pub blocked: Option<ExecError>,
    pub phase: Phase,
    pub requested_start: usize,
    pub start: Mark,
    pub current: Mark,
    pub pc: usize,
    pub frames: usize,
    pub undo: usize,
    pub assertion: usize,
    pub reverse: bool,
    pub work: Work,
    /// Offset of a known admission-condition occurrence. A start at or before
    /// it still has a later occurrence available; `UNSET` means no bound is
    /// in use, because admission did not run or does not carry the claim.
    pub required_at: usize,
    /// Offset from which the condition has not yet been searched. The search
    /// only moves forward, so the whole bound costs one pass over the subject.
    pub required_from: usize,
}
impl State {
    pub fn new(start: usize, length: usize) -> Self {
        Self {
            blocked: None,
            phase: if start > length {
                Phase::Finished(false)
            } else {
                Phase::Admission
            },
            requested_start: start,
            start: Mark::default(),
            current: Mark::default(),
            pc: 0,
            frames: 0,
            undo: 0,
            assertion: UNSET,
            reverse: false,
            work: Work::Idle,
            required_at: UNSET,
            required_from: 0,
        }
    }
}

#[cfg(all(test, target_pointer_width = "64"))]
mod tests {
    use super::*;
    #[test]
    fn scan_storage_reuses_the_admission_buffer_without_expanding_frames() {
        assert!(core::mem::size_of::<Work>() <= 40);
        assert_eq!(core::mem::size_of::<Frame>(), 48);
        assert_eq!(core::mem::size_of::<Phase>(), 48);
        // Two offsets carry the condition bound. One execution state holds
        // them; they add no frame, undo entry or per-position storage.
        assert_eq!(core::mem::size_of::<State>(), 184);
    }
}
