//! Development-only relocation simulation. The evaluator never copies subjects;
//! this mock collector moves and poisons its own original backing allocations.
use perex::{
    Budget,
    binding::{
        BoundProgram, BoundResources, BoundSubject, ImmutableProgram, ImmutableSubject, Subject,
    },
    executor::{
        ExecError, Frame, Progress, Run, Scratch, ScratchOwner, ScratchRequirements, Search,
        SearchError, Undo,
    },
    program::Program,
    span::{BoundSpan, ReadProgress, Span},
};
use std::{cell::RefCell, convert::Infallible};

#[derive(Clone, Copy)]
pub struct Options {
    pub quantum: usize,
    pub relocate: bool,
    pub grow: bool,
    /// Start each search from a position elsewhere in the subject, which must
    /// never change an answer.
    pub near: bool,
    /// Start with `Search::run`, which decides in one call what it can and
    /// hands back a search to continue otherwise.
    pub run: bool,
}
struct Owner {
    storage: RefCell<(Vec<u32>, Vec<u8>)>,
}
impl ImmutableProgram for Owner {
    type Error = Infallible;
    fn with_words<T>(&self, f: impl FnOnce(&[u32]) -> T) -> Result<T, Self::Error> {
        Ok(f(&self.storage.borrow().0))
    }
}
impl ImmutableSubject for Owner {
    type Error = Infallible;
    fn with_subject<T>(&self, f: impl FnOnce(Subject<'_>) -> T) -> Result<T, Self::Error> {
        Ok(f(Subject::Wtf8(&self.storage.borrow().1)))
    }
}
impl Owner {
    fn relocate(&self) {
        let mut storage = self.storage.borrow_mut();
        let replacement = storage.clone();
        assert_ne!(storage.0.as_ptr(), replacement.0.as_ptr());
        if !storage.1.is_empty() {
            assert_ne!(storage.1.as_ptr(), replacement.1.as_ptr());
        }
        storage.0.fill(0xdeadbeef);
        storage.1.fill(0xff);
        *storage = replacement;
    }
}
fn error<E: core::fmt::Debug>(error: SearchError<E>) -> ExecError {
    match error {
        SearchError::Execution(error) => error,
        SearchError::Resource(error) => panic!("immutable development owner failed: {error:?}"),
    }
}

// This owner belongs to the development host. Growth and cleanup deliberately
// collect outside the engine's view scopes when relocation checking is enabled.
struct OwnedBuffers<'a> {
    owner: &'a Owner,
    relocate: bool,
    registers: Vec<usize>,
    frames: Vec<Frame>,
    undo: Vec<Undo>,
}
impl<'a> OwnedBuffers<'a> {
    fn new(owner: &'a Owner, relocate: bool, sizes: ScratchRequirements) -> Self {
        if relocate {
            owner.relocate();
        }
        Self {
            owner,
            relocate,
            registers: vec![0xcafe; sizes.registers],
            frames: vec![Frame::default(); sizes.frames],
            undo: vec![Undo::default(); sizes.undo],
        }
    }
}
impl ScratchOwner for OwnedBuffers<'_> {
    fn scratch(&mut self) -> Scratch<'_> {
        Scratch {
            registers: &mut self.registers,
            frames: &mut self.frames,
            undo: &mut self.undo,
        }
    }
}
impl Drop for OwnedBuffers<'_> {
    fn drop(&mut self) {
        self.registers.fill(0xdeadbeef);
        self.frames.fill(Frame::default());
        self.undo.fill(Undo::default());
        if self.relocate {
            self.owner.relocate();
        }
    }
}

enum Buffers<'s, 'r> {
    Borrowed(Scratch<'s>),
    Owned(OwnedBuffers<'r>),
}
impl ScratchOwner for Buffers<'_, '_> {
    fn scratch(&mut self) -> Scratch<'_> {
        match self {
            Self::Borrowed(scratch) => scratch.scratch(),
            Self::Owned(buffers) => buffers.scratch(),
        }
    }
}
pub fn find(
    program: Program<'_>,
    subject: &[u8],
    start: usize,
    scratch: Scratch<'_>,
    captures: &mut [Option<Span>],
    budget: &mut Budget,
    options: Options,
) -> Result<bool, ExecError> {
    let owner = Owner {
        storage: RefCell::new((program.words().to_vec(), subject.to_vec())),
    };
    let bound_program = BoundProgram::new(&owner, &mut Budget::new(usize::MAX))
        .unwrap_or_else(|e| panic!("{:?}", e.error));
    let bound_subject = BoundSubject::new(&owner).unwrap_or_else(|e| panic!("{:?}", e.error));
    let resources = BoundResources {
        program: &bound_program,
        subject: &bound_subject,
    };
    let mut sizes = ScratchRequirements {
        registers: program.register_count(),
        frames: 0,
        undo: 0,
    };
    let buffers = if options.grow {
        Buffers::Owned(OwnedBuffers::new(&owner, options.relocate, sizes))
    } else {
        Buffers::Borrowed(scratch)
    };
    let near = options.near.then(|| {
        // A position that differs from the start and varies with it, so a
        // corpus reaches hints before, at and after its starts, including
        // between surrogate halves. Reading to it is harness work, charged to
        // an allowance of its own, and relocation still happens between reads.
        let length = bound_subject.with_view(|input| input.len_utf16()).unwrap();
        let at = start.wrapping_mul(7).wrapping_add(length / 3) % (length + 1);
        let mut reader = BoundSpan::new(&bound_subject, Span::new(0, at).unwrap())
            .unwrap_or_else(|e| panic!("{e:?}"));
        let mut work = Budget::new(usize::MAX);
        while reader
            .try_fold(options.quantum, &mut work, |_| Ok::<_, Infallible>(()))
            .unwrap_or_else(|e| panic!("{e:?}"))
            == ReadProgress::Pending
        {
            if options.relocate {
                owner.relocate();
            }
        }
        let position = reader.position();
        if options.relocate {
            owner.relocate();
        }
        position
    });
    let mut search = if options.run {
        match Search::run(&resources, start, near, buffers, *budget, options.quantum)
            .map_err(error)?
        {
            Run::Finished(mut finished) => {
                *budget = Budget::new(finished.remaining_work());
                if !finished.matched() {
                    return Ok(false);
                }
                finished.copy_captures(captures)?;
                return Ok(true);
            }
            Run::Paused(search) => {
                if options.relocate {
                    owner.relocate();
                }
                search
            }
        }
    } else {
        match near {
            Some(near) => Search::new_near(&resources, start, near, buffers, *budget),
            None => Search::new(&resources, start, buffers, *budget),
        }
        .map_err(error)?
    };
    let result = loop {
        match search.advance(options.quantum).map_err(error) {
            Ok(Progress::Pending) => {
                if options.relocate {
                    owner.relocate();
                }
            }
            Err(error @ (ExecError::Frames | ExecError::Undo)) if options.grow => {
                let need = search.required_scratch();
                // Keep the reference probe's existing maximum capacities. A
                // failure at the cap remains visible instead of being waived.
                if need.frames > 16_384 || need.undo > 131_072 {
                    break Err(error);
                }
                sizes.frames = sizes.frames.max(if need.frames == 0 {
                    0
                } else {
                    need.frames.next_power_of_two()
                });
                sizes.undo = sizes.undo.max(if need.undo == 0 {
                    0
                } else {
                    need.undo.next_power_of_two()
                });
                let replacement =
                    Buffers::Owned(OwnedBuffers::new(&owner, options.relocate, sizes));
                search = match search.rebuffer(replacement) {
                    Ok(search) => search,
                    Err(rejected) => {
                        panic!("sufficient replacement rejected: {:?}", rejected.error)
                    }
                };
            }
            result => break result,
        }
    };
    *budget = Budget::new(search.remaining_work());
    match result? {
        Progress::Matched => {
            search.copy_captures(captures)?;
            Ok(true)
        }
        Progress::NoMatch => Ok(false),
        Progress::Pending => unreachable!(),
    }
}
