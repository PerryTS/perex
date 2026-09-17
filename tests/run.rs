//! Starting a search and running its first quantum in one call:
//! `Search::run`. See `docs/resumption.md`.
use perex::{
    Budget,
    binding::{
        BoundProgram, BoundResources, BoundSubject, ImmutableProgram, ImmutableSubject, Subject,
    },
    compiler::{Node, Range, compile},
    executor::{
        ExecError, Frame, Progress, Resources, Run, Scratch, ScratchOwner, Search, SearchError,
        Undo,
    },
    input::{Input, Position},
    span::{BoundSpan, ReadProgress, Span},
};
use std::{cell::RefCell, convert::Infallible};

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
    fn new(frames: usize, undo: usize) -> Self {
        Self {
            registers: vec![0xcafe; 64],
            frames: vec![Frame::default(); frames],
            undo: vec![Undo::default(); undo],
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
    ("z", ""),
    ("needle", ""),
    ("NeEdLe", "i"),
    ("(\\w+)@(\\w+)\\.com", ""),
    ("[a-z]+!", ""),
    ("(a)\\1", ""),
    ("(?<x>a|b)\\k<x>", ""),
    ("(?<=(a))b", ""),
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
    "ab",
    "bab",
    "hay NEEDLE hay needle",
    "mail user@example.com now",
    "x\nb",
    "éa😀b",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaac",
];

/// Everything a finished search reports.
#[derive(Debug, PartialEq)]
struct Answer {
    outcome: Result<bool, ExecError>,
    captures: Vec<Option<Span>>,
    position: Option<Position>,
    remaining: usize,
}

/// Enough for every search here but the exponential one, which then ends in
/// a work limit on both sides — an answer to compare as well.
const WORK: usize = 200_000;

/// A position in `owner`'s subject `at` units in, as a host that read a span
/// there would hold one.
fn position(owner: &Owner, at: usize) -> Position {
    let subject = BoundSubject::new(owner).unwrap();
    let mut reader = BoundSpan::new(&subject, Span::new(0, at).unwrap()).unwrap();
    let mut work = Budget::new(usize::MAX);
    while reader
        .try_fold(usize::MAX, &mut work, |_| Ok::<_, Infallible>(()))
        .unwrap()
        == ReadProgress::Pending
    {}
    reader.position()
}

/// Drive a search to its end, moving storage at every pause and growing
/// scratch when it asks, and report what it finished with.
fn finish<R: Resources>(
    owner: &Owner,
    mut search: Search<'_, R, Buffers>,
    quantum: usize,
    boolean: bool,
) -> Answer
where
    R::Error: core::fmt::Debug,
{
    let outcome = loop {
        match search.advance(quantum) {
            Ok(Progress::Pending) => owner.relocate(),
            Ok(progress) => break Ok(progress == Progress::Matched),
            Err(SearchError::Execution(ExecError::Frames | ExecError::Undo)) => {
                let need = search.required_scratch();
                search = match search.rebuffer(Buffers::new(need.frames * 2, need.undo * 2)) {
                    Ok(search) => search,
                    Err(rejected) => panic!("{:?}", rejected.error),
                };
            }
            Err(SearchError::Execution(error)) => break Err(error),
            Err(SearchError::Resource(error)) => panic!("{error:?}"),
        }
    };
    let mut captures = vec![None; search.capture_count()];
    if outcome == Ok(true) && !boolean {
        search.copy_captures(&mut captures).unwrap();
    }
    Answer {
        outcome,
        captures,
        position: Some(search.position()),
        remaining: search.remaining_work(),
    }
}

#[test]
fn a_run_answers_as_a_new_search_advanced_by_the_same_quantum() {
    let mut decided = 0;
    let mut paused = 0;
    for &(pattern, flags) in PATTERNS {
        for subject in SUBJECTS {
            let owner = Owner::new(pattern, flags, subject);
            let program = BoundProgram::new(&owner, &mut Budget::new(WORK)).unwrap();
            let bound = BoundSubject::new(&owner).unwrap();
            let resources = BoundResources {
                program: &program,
                subject: &bound,
            };
            let length = subject.encode_utf16().count();
            for start in [0, 1, length + 1] {
                for near in [None, Some(length / 2)] {
                    for quantum in [1, 17, 4096] {
                        let context = format!(
                            "/{pattern}/{flags} on {subject:?} from {start} near {near:?}, quantum {quantum}"
                        );
                        let near = near.map(|at| position(&owner, at));
                        // Small scratch, so capacity requests happen too.
                        let reference = match near {
                            Some(near) => Search::new_near(
                                &resources,
                                start,
                                near,
                                Buffers::new(2, 2),
                                Budget::new(WORK),
                            ),
                            None => Search::new(
                                &resources,
                                start,
                                Buffers::new(2, 2),
                                Budget::new(WORK),
                            ),
                        }
                        .unwrap();
                        let expected = finish(&owner, reference, quantum, false);
                        let run = Search::run(
                            &resources,
                            start,
                            near,
                            Buffers::new(2, 2),
                            Budget::new(WORK),
                            quantum,
                        )
                        .unwrap();
                        let answer = match run {
                            Run::Finished(mut finished) => {
                                decided += 1;
                                let matched = finished.matched();
                                let mut captures = vec![None; finished.capture_count()];
                                if matched {
                                    finished.copy_captures(&mut captures).unwrap();
                                    assert_eq!(finished.capture(0).unwrap(), captures[0]);
                                } else {
                                    assert_eq!(
                                        finished.copy_captures(&mut captures),
                                        Err(ExecError::NotMatched)
                                    );
                                }
                                Answer {
                                    outcome: Ok(matched),
                                    captures,
                                    position: Some(finished.position()),
                                    remaining: finished.remaining_work(),
                                }
                            }
                            Run::Paused(search) => {
                                paused += 1;
                                owner.relocate();
                                finish(&owner, search, quantum, false)
                            }
                        };
                        assert_eq!(answer, expected, "{context}");
                    }
                }
            }
        }
    }
    // Both halves are exercised, not only the one every short search takes.
    assert!(
        decided > 300 && paused > 300,
        "{decided} decided, {paused} paused"
    );
}

#[test]
fn a_boolean_run_answers_without_captures() {
    for &(pattern, flags) in PATTERNS {
        for subject in SUBJECTS {
            let owner = Owner::new(pattern, flags, subject);
            let program = BoundProgram::new(&owner, &mut Budget::new(WORK)).unwrap();
            let bound = BoundSubject::new(&owner).unwrap();
            let resources = BoundResources {
                program: &program,
                subject: &bound,
            };
            for quantum in [1, 4096] {
                let context = format!("/{pattern}/{flags} on {subject:?}, quantum {quantum}");
                let mut reference =
                    Search::new(&resources, 0, Buffers::new(1024, 4096), Budget::new(WORK))
                        .unwrap();
                reference.without_captures();
                let expected = finish(&owner, reference, quantum, true);
                let run = Search::run_without_captures(
                    &resources,
                    0,
                    None,
                    Buffers::new(1024, 4096),
                    Budget::new(WORK),
                    quantum,
                )
                .unwrap();
                let (outcome, remaining) = match run {
                    Run::Finished(mut finished) => {
                        assert_eq!(finished.capture(0), Err(ExecError::Captures), "{context}");
                        (Ok(finished.matched()), finished.remaining_work())
                    }
                    Run::Paused(mut search) => {
                        let outcome = loop {
                            match search.advance(quantum) {
                                Ok(Progress::Pending) => owner.relocate(),
                                Ok(progress) => break Ok(progress == Progress::Matched),
                                Err(SearchError::Execution(error)) => break Err(error),
                                Err(SearchError::Resource(error)) => panic!("{error:?}"),
                            }
                        };
                        assert_eq!(search.capture(0), Err(ExecError::Captures), "{context}");
                        (outcome, search.remaining_work())
                    }
                };
                assert_eq!(outcome, expected.outcome, "{context}");
                assert_eq!(remaining, expected.remaining, "{context}");
            }
        }
    }
}

#[test]
fn a_run_refuses_what_a_new_search_refuses() {
    let owner = Owner::new("(a)(b)", "", "xab");
    let program = BoundProgram::new(&owner, &mut Budget::new(WORK)).unwrap();
    let bound = BoundSubject::new(&owner).unwrap();
    let resources = BoundResources {
        program: &program,
        subject: &bound,
    };
    let mut short = Buffers::new(8, 8);
    short.registers.truncate(3);
    assert!(matches!(
        Search::run(&resources, 0, None, short, Budget::new(WORK), 4096),
        Err(SearchError::Execution(ExecError::Registers))
    ));
    let other = Owner::new("a", "", "a much longer subject");
    let elsewhere = position(&other, 3);
    assert!(matches!(
        Search::run(
            &resources,
            0,
            Some(elsewhere),
            Buffers::new(8, 8),
            Budget::new(WORK),
            4096
        ),
        Err(SearchError::Execution(ExecError::ChangedResources))
    ));
    // A zero quantum does no work and hands the search back.
    match Search::run(
        &resources,
        0,
        None,
        Buffers::new(8, 8),
        Budget::new(WORK),
        0,
    )
    .unwrap()
    {
        Run::Paused(search) => assert_eq!(search.remaining_work(), WORK),
        Run::Finished(_) => panic!("a zero quantum decided a search"),
    }
    // A start past the end is decided without a match.
    match Search::run(
        &resources,
        4,
        None,
        Buffers::new(8, 8),
        Budget::new(WORK),
        4096,
    )
    .unwrap()
    {
        Run::Finished(mut finished) => {
            assert!(!finished.matched());
            assert_eq!(finished.capture(0), Err(ExecError::NotMatched));
        }
        Run::Paused(_) => panic!("a start past the end paused"),
    }
    match Search::run(
        &resources,
        0,
        None,
        Buffers::new(8, 8),
        Budget::new(WORK),
        4096,
    )
    .unwrap()
    {
        Run::Finished(mut finished) => {
            assert!(finished.matched());
            assert_eq!(finished.capture(1).unwrap(), Span::new(1, 2));
            assert_eq!(finished.capture(3), Err(ExecError::Captures));
            let mut output = vec![None; 2];
            assert_eq!(
                finished.copy_captures(&mut output),
                Err(ExecError::Captures)
            );
            let buffers = finished.into_buffers();
            assert_eq!(&buffers.registers[..6], &[1, 3, 1, 2, 2, 3]);
        }
        Run::Paused(_) => panic!("a short search paused"),
    }
}
