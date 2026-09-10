//! Development-only relocation simulation. The evaluator never copies subjects;
//! this mock collector moves and poisons its own original backing allocations.
use perex::{
    Budget,
    executor::{
        ExecError, Frame, Progress, Resources, Scratch, ScratchOwner, ScratchRequirements, Search,
        SearchError, Undo,
    },
    input::Input,
    program::Program,
    span::Span,
};
use std::{cell::RefCell, convert::Infallible};

#[derive(Clone, Copy)]
pub struct Options {
    pub quantum: usize,
    pub relocate: bool,
    pub grow: bool,
}
struct Owner {
    storage: RefCell<(Vec<u32>, Vec<u8>)>,
}
impl Resources for Owner {
    type Error = Infallible;
    fn with_views<T>(&self, f: impl FnOnce(Program<'_>, Input<'_>) -> T) -> Result<T, Self::Error> {
        let storage = self.storage.borrow();
        let p = Program::from_words(&storage.0, &mut Budget::new(usize::MAX)).unwrap();
        let input = Input::wtf8(&storage.1).unwrap();
        Ok(f(p, input))
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
fn error(error: SearchError<Infallible>) -> ExecError {
    match error {
        SearchError::Execution(error) => error,
        SearchError::Resource(never) => match never {},
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
    let mut search = Search::new(&owner, start, buffers, *budget).map_err(error)?;
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
