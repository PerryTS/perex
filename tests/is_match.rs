//! Answering only whether a match exists: `executor::is_match` and
//! `Search::without_captures`. See `docs/engine.md`.
use perex::{
    Budget,
    binding::{
        BoundProgram, BoundResources, BoundSubject, ImmutableProgram, ImmutableSubject, Subject,
    },
    compiler::{Node, Range, compile},
    executor::{
        ExecError, Frame, Progress, Scratch, ScratchOwner, Search, SearchError, Undo, find,
        is_match,
    },
    input::Input,
    program::Program,
};
use std::cell::RefCell;

/// A program and subject whose storage moves, poisoned, at every pause.
#[derive(Debug)]
struct Owner {
    data: RefCell<(Vec<u32>, Vec<u8>)>,
}

impl Owner {
    fn new(pattern: &str, flags: &str, subject: &str) -> Self {
        let mut nodes = vec![Node::default(); pattern.len() * 4 + 128];
        let mut ranges = vec![Range::default(); pattern.len() * 12 + 128];
        let mut words = vec![0; pattern.len() * 48 + 256];
        let program = compile(
            Input::utf8(pattern),
            flags,
            &mut nodes,
            &mut ranges,
            &mut words,
            &mut Budget::new(10_000_000),
        )
        .unwrap_or_else(|e| panic!("/{pattern}/{flags}: {e:?}"));
        Self {
            data: RefCell::new((program.words().to_vec(), subject.as_bytes().to_vec())),
        }
    }

    fn relocate(&self) {
        let mut data = self.data.borrow_mut();
        let replacement = data.clone();
        data.0.fill(0xdead_beef);
        data.1.fill(0xff);
        *data = replacement;
    }

    fn with_input<T>(&self, f: impl FnOnce(Program<'_>, Input<'_>) -> T) -> T {
        let data = self.data.borrow();
        let program = Program::from_words(&data.0, &mut Budget::new(10_000_000)).unwrap();
        f(program, Input::wtf8(&data.1).unwrap())
    }
}

impl ImmutableProgram for Owner {
    type Error = ();
    fn with_words<T>(&self, f: impl FnOnce(&[u32]) -> T) -> Result<T, ()> {
        Ok(f(&self.data.borrow().0))
    }
}

impl ImmutableSubject for Owner {
    type Error = ();
    fn with_subject<T>(&self, f: impl FnOnce(Subject<'_>) -> T) -> Result<T, ()> {
        Ok(f(Subject::Wtf8(&self.data.borrow().1)))
    }
}

struct Buffers {
    registers: Vec<usize>,
    frames: Vec<Frame>,
    undo: Vec<Undo>,
}

impl Buffers {
    fn new() -> Self {
        Self {
            registers: vec![0xcafe; 64],
            frames: vec![Frame::default(); 1024],
            undo: vec![Undo::default(); 4096],
        }
    }
}

impl ScratchOwner for Buffers {
    fn scratch(&mut self) -> Scratch<'_> {
        Scratch {
            registers: &mut self.registers,
            frames: &mut self.frames,
            undo: &mut self.undo,
        }
    }
}

const PATTERNS: &[(&str, &str)] = &[
    ("a", ""),
    ("needle", ""),
    ("NeEdLe", "i"),
    ("(\\w+)@(\\w+)\\.com", ""),
    ("[a-z]+[0-9]+", ""),
    ("(a)\\1", ""),
    ("(?<x>a|b)\\k<x>", ""),
    ("(?<=(a))b", ""),
    ("(a)|b", ""),
    ("(?:(a)|b)+\\1", ""),
    ("^b", "m"),
    ("c$", ""),
    ("a*", ""),
    ("", ""),
    ("\\p{L}+", "u"),
    ("[\\q{ab}c]", "v"),
    ("x", "y"),
    ("(a+)+b", ""),
];

const SUBJECTS: &[&str] = &[
    "",
    "a",
    "aa",
    "ab",
    "ba",
    "bab",
    "needle",
    "hay NEEDLE hay",
    "mail user@example.com now",
    "abc123",
    "x\nb",
    "abc",
    "éa😀b",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaac",
];

/// What `find` answers, and the work it charged.
fn found(owner: &Owner, start: usize) -> (Result<bool, ExecError>, usize) {
    owner.with_input(|program, input| {
        let mut buffers = Buffers::new();
        let mut captures = vec![None; program.capture_count()];
        let mut budget = Budget::new(10_000_000);
        let answer = find(
            program,
            input,
            start,
            buffers.scratch(),
            &mut captures,
            &mut budget,
        );
        (answer, 10_000_000 - budget.remaining())
    })
}

/// What `is_match` answers, and the work it charged.
fn matched(owner: &Owner, start: usize) -> (Result<bool, ExecError>, usize) {
    owner.with_input(|program, input| {
        let mut buffers = Buffers::new();
        let mut budget = Budget::new(10_000_000);
        let answer = is_match(program, input, start, buffers.scratch(), &mut budget);
        (answer, 10_000_000 - budget.remaining())
    })
}

#[test]
fn a_boolean_answer_is_the_answer_find_gives() {
    for &(pattern, flags) in PATTERNS {
        for subject in SUBJECTS {
            let owner = Owner::new(pattern, flags, subject);
            for start in 0..=subject.encode_utf16().count() + 1 {
                let context = format!("/{pattern}/{flags} on {subject:?} from {start}");
                let (expected, found_work) = found(&owner, start);
                let (answer, work) = matched(&owner, start);
                assert_eq!(answer, expected, "{context}");
                // A match is charged two units per capture less: the check it
                // leaves out. A miss is charged the same.
                let count = owner.with_input(|program, _| program.capture_count());
                let saved = if expected == Ok(true) { 2 * count } else { 0 };
                assert_eq!(work + saved, found_work, "{context}");
            }
        }
    }
}

#[test]
fn a_paused_boolean_search_answers_as_the_synchronous_one() {
    for &(pattern, flags) in PATTERNS {
        for subject in SUBJECTS {
            let owner = Owner::new(pattern, flags, subject);
            let program = BoundProgram::new(&owner, &mut Budget::new(10_000_000)).unwrap();
            let bound = BoundSubject::new(&owner).unwrap();
            let resources = BoundResources {
                program: &program,
                subject: &bound,
            };
            for start in [0, 1, subject.len()] {
                let (expected, work) = matched(&owner, start);
                for quantum in [1, 17, usize::MAX] {
                    let context = format!(
                        "/{pattern}/{flags} on {subject:?} from {start}, quantum {quantum}"
                    );
                    let mut search =
                        Search::new(&resources, start, Buffers::new(), Budget::new(10_000_000))
                            .unwrap();
                    search.without_captures();
                    let answer = loop {
                        match search.advance(quantum) {
                            Ok(Progress::Pending) => owner.relocate(),
                            Ok(progress) => break Ok(progress == Progress::Matched),
                            Err(SearchError::Execution(error)) => break Err(error),
                            Err(SearchError::Resource(_)) => panic!("{context}"),
                        }
                    };
                    assert_eq!(answer, expected, "{context}");
                    assert_eq!(10_000_000 - search.remaining_work(), work, "{context}");
                    assert_eq!(search.capture(0), Err(ExecError::Captures), "{context}");
                    let mut output = vec![None; search.capture_count()];
                    assert_eq!(
                        search.copy_captures(&mut output),
                        Err(ExecError::Captures),
                        "{context}"
                    );
                    assert!(output.iter().all(Option::is_none), "{context}");
                }
            }
        }
    }
}

#[test]
fn restarting_keeps_a_search_boolean() {
    let owner = Owner::new("(a)", "", "bab");
    let program = BoundProgram::new(&owner, &mut Budget::new(10_000_000)).unwrap();
    let bound = BoundSubject::new(&owner).unwrap();
    let resources = BoundResources {
        program: &program,
        subject: &bound,
    };
    let mut search = Search::new(&resources, 0, Buffers::new(), Budget::new(10_000_000)).unwrap();
    search.without_captures();
    assert!(matches!(search.advance(usize::MAX), Ok(Progress::Matched)));
    search.restart_at(2);
    assert!(matches!(search.advance(usize::MAX), Ok(Progress::NoMatch)));
    search.restart_at(0);
    assert!(matches!(search.advance(usize::MAX), Ok(Progress::Matched)));
    assert_eq!(search.capture(1), Err(ExecError::Captures));
    // Without it, the same search reports its captures.
    let mut search = Search::new(&resources, 0, Buffers::new(), Budget::new(10_000_000)).unwrap();
    assert!(matches!(search.advance(usize::MAX), Ok(Progress::Matched)));
    assert!(search.capture(1).unwrap().is_some());
}
