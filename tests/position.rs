//! Starting a search or a span reader from a position the subject already
//! produced. See `docs/resumption.md`.
use perex::{
    Budget,
    binding::{
        BoundProgram, BoundResources, BoundSubject, ImmutableProgram, ImmutableSubject, Subject,
    },
    compiler::{Node, Range, compile},
    executor::{
        ExecError, Frame, Progress, Scratch, ScratchOwner, Search, SearchError, Undo, find,
    },
    input::{Input, Position},
    program::Program,
    span::{BoundSpan, ReadError, ReadProgress, Span},
};
use std::cell::{Cell, RefCell};

#[derive(Clone, Debug)]
enum Storage {
    Bytes(Vec<u8>),
    Units(Vec<u16>),
}

/// A subject and program whose storage moves at every pause, with the old
/// allocations poisoned and freed, as a moving collector's would be.
#[derive(Debug)]
struct Owner {
    data: RefCell<(Vec<u32>, Storage)>,
    moves: Cell<usize>,
}

impl Owner {
    fn new(pattern: &str, flags: &str, storage: Storage) -> Self {
        let mut nodes = vec![Node::default(); pattern.len() * 4 + 128];
        let mut ranges = vec![Range::default(); pattern.len() * 4 + 128];
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
            data: RefCell::new((program.words().to_vec(), storage)),
            moves: Cell::new(0),
        }
    }

    fn relocate(&self) {
        let mut data = self.data.borrow_mut();
        let replacement = data.clone();
        data.0.fill(0xdead_beef);
        match &mut data.1 {
            Storage::Bytes(bytes) => bytes.fill(0xff),
            Storage::Units(units) => units.fill(0xeeee),
        }
        *data = replacement;
        self.moves.set(self.moves.get() + 1);
    }

    fn with_input<T>(&self, f: impl FnOnce(Program<'_>, Input<'_>) -> T) -> T {
        let data = self.data.borrow();
        let program = Program::from_words(&data.0, &mut Budget::new(10_000_000)).unwrap();
        let input = match &data.1 {
            Storage::Bytes(bytes) => Input::wtf8(bytes).unwrap(),
            Storage::Units(units) => Input::utf16(units),
        };
        f(program, input)
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
        let data = self.data.borrow();
        Ok(f(match &data.1 {
            Storage::Bytes(bytes) => Subject::Wtf8(bytes),
            Storage::Units(units) => Subject::Utf16(units),
        }))
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

/// A finished search: whether it matched, its captures, where it stood, and
/// the work it left.
struct Outcome {
    matched: bool,
    captures: Vec<Option<Span>>,
    position: Position,
    remaining: usize,
}

/// Advance to completion in small quanta, moving the storage at every pause.
fn finish<R: perex::executor::Resources>(
    mut search: Search<'_, R, Buffers>,
    owner: &Owner,
) -> Outcome
where
    R::Error: std::fmt::Debug,
{
    let progress = loop {
        match search.advance(3) {
            Ok(Progress::Pending) => owner.relocate(),
            Ok(progress) => break progress,
            Err(SearchError::Execution(error)) => panic!("search failed: {error:?}"),
            Err(SearchError::Resource(error)) => panic!("owner failed: {error:?}"),
        }
    };
    let mut captures = vec![None; search.capture_count()];
    let matched = progress == Progress::Matched;
    if matched {
        search.copy_captures(&mut captures).unwrap();
    }
    Outcome {
        matched,
        captures,
        position: search.position(),
        remaining: search.remaining_work(),
    }
}

/// A position at every UTF-16 index of the subject, including between the two
/// halves of every astral character, read through a span reader.
fn every_position(subject: &BoundSubject<&Owner>, length: usize) -> Vec<Position> {
    let mut reader = BoundSpan::new(subject, Span::new(0, length).unwrap()).unwrap();
    let mut positions = vec![reader.position()];
    let mut budget = Budget::new(usize::MAX);
    while positions.len() <= length {
        let progress = reader
            .try_fold(1, &mut budget, |_| Ok::<_, ()>(()))
            .unwrap_or_else(|_| panic!("reading the subject failed"));
        positions.push(reader.position());
        if progress == ReadProgress::Complete {
            break;
        }
    }
    for (index, position) in positions.iter().enumerate() {
        assert_eq!(position.utf16(), index);
    }
    positions
}

fn wtf8(text: &str) -> Storage {
    Storage::Bytes(text.as_bytes().to_vec())
}

fn subjects() -> Vec<(&'static str, Storage)> {
    // U+D83D alone, as generalized UTF-8 encodes a lone surrogate.
    let mut lone = b"a\xed\xa0\xbd\xc3\xa9x".to_vec();
    lone.extend_from_slice("😀a".as_bytes());
    vec![
        ("ascii", wtf8("ab ax a")),
        ("two-byte", wtf8("aé éxa")),
        ("astral", wtf8("é😀a😀😀 xa")),
        ("lone surrogate", Storage::Bytes(lone)),
        ("utf-16", Storage::Units("a😀é xa".encode_utf16().collect())),
        // Long enough that a hint is often nearer than either end.
        ("long", wtf8(&"aé😀 x".repeat(4))),
    ]
}

const PATTERNS: &[(&str, &str)] = &[
    ("a", ""),
    ("x", ""),
    ("é", ""),
    ("(é+)(x?)", ""),
    ("😀", "u"),
    ("[😀-🙏]+", "u"),
    (".", ""),
    (".", "u"),
    ("(?<=é)a", ""),
    ("(?<![a-z])a", ""),
    ("\\b\\w+", ""),
    ("(a).*\\1", ""),
    ("a$", ""),
    ("a", "y"),
    ("\\ud83d", ""),
    ("\\ude00", ""),
    ("(?:)", ""),
    ("(?:)", "u"),
];

/// Where a search starts from never changes what it finds. Every pattern, from
/// every start including past the end, with a hint at every position of the
/// subject, gives the answer and captures a plain search gives, never charges
/// more work than one, and after a match stands at the match's end.
#[test]
fn a_hint_never_changes_an_answer() {
    let (mut compared, mut cheaper) = (0, 0);
    for (label, storage) in subjects() {
        for &(pattern, flags) in PATTERNS {
            let owner = Owner::new(pattern, flags, storage.clone());
            let length = owner.with_input(|_, input| input.len_utf16());
            let program = BoundProgram::new(&owner, &mut Budget::new(10_000_000)).unwrap();
            let subject = BoundSubject::new(&owner).unwrap();
            let resources = BoundResources {
                program: &program,
                subject: &subject,
            };
            let positions = every_position(&subject, length);
            for start in 0..=length + 1 {
                let mut expected = vec![None; 32];
                let mut budget = Budget::new(10_000_000);
                let found = owner.with_input(|program, input| {
                    let mut buffers = Buffers::new();
                    let count = program.capture_count();
                    let found = find(
                        program,
                        input,
                        start,
                        buffers.scratch(),
                        &mut expected[..count],
                        &mut budget,
                    )
                    .unwrap();
                    expected.truncate(count);
                    found
                });
                for &near in &positions {
                    let context = format!(
                        "/{pattern}/{flags} over {label}, start {start}, near {}",
                        near.utf16()
                    );
                    let search = Search::new_near(
                        &resources,
                        start,
                        near,
                        Buffers::new(),
                        Budget::new(10_000_000),
                    )
                    .unwrap_or_else(|_| panic!("{context}: refused"));
                    let outcome = finish(search, &owner);
                    assert_eq!(outcome.matched, found, "{context}");
                    if found {
                        assert_eq!(outcome.captures, expected, "{context}");
                        assert_eq!(
                            Some(outcome.position.utf16()),
                            expected[0].map(|span| span.end()),
                            "{context}: a match leaves the search at its end"
                        );
                    }
                    cheaper += usize::from(outcome.remaining > budget.remaining());
                    assert!(
                        outcome.remaining >= budget.remaining(),
                        "{context}: charged {} where a plain search charged {}",
                        10_000_000 - outcome.remaining,
                        10_000_000 - budget.remaining()
                    );
                    compared += 1;
                }
            }
        }
    }
    // Most hints are no nearer than an end on subjects this short, so the
    // count that actually started from one is what shows the path was taken.
    assert!(
        compared > 10_000 && cheaper > 1_000,
        "{compared} compared, {cheaper} cheaper"
    );
}

/// Work a global loop charges to find every match, starting each search at the
/// previous match's end, from that match's position or not.
fn global_loop(repeats: usize, near: bool) -> (usize, usize) {
    let text = "ééé a ".repeat(repeats);
    let owner = Owner::new("a", "", wtf8(&text));
    let program = BoundProgram::new(&owner, &mut Budget::new(10_000_000)).unwrap();
    let subject = BoundSubject::new(&owner).unwrap();
    let resources = BoundResources {
        program: &program,
        subject: &subject,
    };
    let mut budget = Budget::new(usize::MAX);
    let (mut start, mut position, mut matches) = (0, None, 0);
    loop {
        let search = match position {
            Some(position) if near => {
                Search::new_near(&resources, start, position, Buffers::new(), budget)
            }
            _ => Search::new(&resources, start, Buffers::new(), budget),
        }
        .unwrap_or_else(|_| panic!("refused"));
        let outcome = finish(search, &owner);
        budget = Budget::new(outcome.remaining);
        if !outcome.matched {
            break;
        }
        matches += 1;
        start = outcome.captures[0].unwrap().end();
        position = Some(outcome.position);
    }
    assert_eq!(matches, repeats);
    (usize::MAX - budget.remaining(), matches)
}

/// Work a `split`-shaped loop charges: a sticky attempt at every position,
/// moving past a match or on by one unit, each from the previous search's
/// position or not.
fn sticky_loop(repeats: usize, near: bool) -> usize {
    let text = "ééé,aé;".repeat(repeats);
    let owner = Owner::new("[,;]", "y", wtf8(&text));
    let length = owner.with_input(|_, input| input.len_utf16());
    let program = BoundProgram::new(&owner, &mut Budget::new(10_000_000)).unwrap();
    let subject = BoundSubject::new(&owner).unwrap();
    let resources = BoundResources {
        program: &program,
        subject: &subject,
    };
    let mut budget = Budget::new(usize::MAX);
    let (mut at, mut position, mut matches) = (0, None, 0);
    while at < length {
        let search = match position {
            Some(position) if near => {
                Search::new_near(&resources, at, position, Buffers::new(), budget)
            }
            _ => Search::new(&resources, at, Buffers::new(), budget),
        }
        .unwrap_or_else(|_| panic!("refused"));
        let outcome = finish(search, &owner);
        budget = Budget::new(outcome.remaining);
        position = Some(outcome.position);
        if outcome.matched {
            matches += 1;
            at = outcome.captures[0].unwrap().end();
            assert_eq!(outcome.position.utf16(), at);
        } else {
            assert_eq!(
                outcome.position.utf16(),
                at,
                "a failed sticky attempt stays at its start"
            );
            at += 1;
        }
    }
    assert_eq!(matches, repeats * 2);
    usize::MAX - budget.remaining()
}

#[test]
fn a_sticky_loop_from_positions_does_linear_work() {
    let (near_small, near_large) = (sticky_loop(1_000, true), sticky_loop(2_000, true));
    let (plain_small, plain_large) = (sticky_loop(1_000, false), sticky_loop(2_000, false));
    assert!(
        near_large * 10 < near_small * 22,
        "from positions: {near_small} then {near_large}"
    );
    assert!(
        plain_large * 10 > plain_small * 35,
        "from the ends: {plain_small} then {plain_large}"
    );
    assert!(
        near_large * 20 < plain_large,
        "{near_large} from positions against {plain_large} from the ends"
    );
}

#[test]
fn a_global_loop_from_positions_does_linear_work() {
    let (near_small, _) = global_loop(2_000, true);
    let (near_large, _) = global_loop(4_000, true);
    let (plain_small, _) = global_loop(2_000, false);
    let (plain_large, _) = global_loop(4_000, false);
    // Doubling the subject doubles the matches. From positions that doubles
    // the work; from the ends each search also seeks twice as far.
    assert!(
        near_large * 10 < near_small * 22,
        "from positions: {near_small} then {near_large}"
    );
    assert!(
        plain_large * 10 > plain_small * 35,
        "from the ends: {plain_small} then {plain_large}"
    );
    assert!(
        near_large * 20 < plain_large,
        "{near_large} from positions against {plain_large} from the ends"
    );
}

/// Where a global loop resumes after an empty match: the next code point under
/// the `u` flag, the next unit otherwise. A unit is not enough there, because a
/// start inside a surrogate pair is normalized back to the pair, which would
/// find the same empty match forever.
fn after_empty(owner: &Owner, at: usize, unicode: bool) -> usize {
    if !unicode {
        return at + 1;
    }
    owner.with_input(|_, input| {
        let mut cursor = input.cursor_at(at).expect("a position in the subject");
        match cursor.next_point() {
            Some(_) => cursor.position(),
            None => at + 1,
        }
    })
}

/// Walking every match of a subject: restarting one search must give what a
/// new search from the previous position gives — the same spans, captures,
/// charged work and positions — for every pattern and storage, with the owner
/// relocating at every pause.
#[test]
fn a_restarted_search_finds_what_a_new_one_finds() {
    let mut compared = 0;
    for (label, storage) in subjects() {
        for &(pattern, flags) in PATTERNS {
            let owner = Owner::new(pattern, flags, storage.clone());
            let length = owner.with_input(|_, input| input.len_utf16());
            let program = BoundProgram::new(&owner, &mut Budget::new(10_000_000)).unwrap();
            let subject = BoundSubject::new(&owner).unwrap();
            let resources = BoundResources {
                program: &program,
                subject: &subject,
            };
            // Every match, as a global loop takes them: from the end of the
            // last one, a unit on when it was empty.
            let mut fresh = Vec::new();
            let (mut start, mut near) = (0, None);
            // One allowance for the whole walk, as a host gives an operation,
            // so the work each match leaves can be compared.
            let mut budget = Budget::new(usize::MAX);
            while start <= length {
                let search = match near {
                    Some(near) => Search::new_near(&resources, start, near, Buffers::new(), budget),
                    None => Search::new(&resources, start, Buffers::new(), budget),
                }
                .unwrap_or_else(|_| panic!("/{pattern}/{flags} over {label} from {start}"));
                let outcome = finish(search, &owner);
                budget = Budget::new(outcome.remaining);
                near = Some(outcome.position);
                if !outcome.matched {
                    break;
                }
                let span = outcome.captures[0].expect("a match has a span");
                fresh.push((
                    outcome.captures.clone(),
                    outcome.remaining,
                    outcome.position,
                ));
                start = if span.is_empty() {
                    after_empty(&owner, span.end(), flags.contains('u'))
                } else {
                    span.end()
                };
            }

            let mut restarted = Vec::new();
            let mut search =
                Search::new(&resources, 0, Buffers::new(), Budget::new(usize::MAX)).unwrap();
            let mut start = 0;
            while start <= length {
                let progress = loop {
                    match search.advance(3) {
                        Ok(Progress::Pending) => owner.relocate(),
                        Ok(progress) => break progress,
                        Err(error) => panic!("/{pattern}/{flags} over {label}: {error:?}"),
                    }
                };
                if progress != Progress::Matched {
                    break;
                }
                let mut captures = vec![None; search.capture_count()];
                search.copy_captures(&mut captures).unwrap();
                let span = captures[0].expect("a match has a span");
                restarted.push((captures, search.remaining_work(), search.position()));
                start = if span.is_empty() {
                    after_empty(&owner, span.end(), flags.contains('u'))
                } else {
                    span.end()
                };
                search.restart_at(start);
            }
            assert_eq!(
                restarted, fresh,
                "/{pattern}/{flags} over {label}: restarting must find what new searches find"
            );
            compared += fresh.len();
        }
    }
    assert!(compared > 300, "{compared} matches compared");
}

/// A search over borrowed scratch finds what one that owns it finds, restarts
/// the same way, and hands the borrow back. A host that keeps its buffers
/// across calls lends them like this instead of moving them into every search.
#[test]
fn a_borrowed_scratch_owner_searches_like_an_owned_one() {
    for (label, storage) in subjects() {
        for &(pattern, flags) in PATTERNS {
            let owner = Owner::new(pattern, flags, storage.clone());
            let length = owner.with_input(|_, input| input.len_utf16());
            let program = BoundProgram::new(&owner, &mut Budget::new(10_000_000)).unwrap();
            let subject = BoundSubject::new(&owner).unwrap();
            let resources = BoundResources {
                program: &program,
                subject: &subject,
            };
            let mut lent = Buffers::new();
            for start in 0..=length {
                let owned = finish(
                    Search::new(&resources, start, Buffers::new(), Budget::new(1_000_000)).unwrap(),
                    &owner,
                );
                let mut search =
                    Search::new(&resources, start, &mut lent, Budget::new(1_000_000)).unwrap();
                let progress = loop {
                    match search.advance(3) {
                        Ok(Progress::Pending) => owner.relocate(),
                        Ok(progress) => break progress,
                        Err(error) => panic!("/{pattern}/{flags} over {label}: {error:?}"),
                    }
                };
                let mut captures = vec![None; search.capture_count()];
                let matched = progress == Progress::Matched;
                if matched {
                    search.copy_captures(&mut captures).unwrap();
                }
                assert_eq!(matched, owned.matched, "/{pattern}/{flags} over {label}");
                if matched {
                    assert_eq!(captures, owned.captures, "/{pattern}/{flags} over {label}");
                }
                assert_eq!(
                    search.remaining_work(),
                    owned.remaining,
                    "/{pattern}/{flags} over {label}"
                );
                search.restart_at(start);
                // Handing the borrow back leaves the owner reusable for the
                // next search, which is how a host keeps one set of buffers.
                let _: &mut Buffers = search.into_buffers();
            }
        }
    }
}

/// Restarting is a new operation: what the previous one ended as does not
/// survive it, and a start past the end is no match.
#[test]
fn a_restart_leaves_no_state_of_the_search_before_it() {
    let owner = Owner::new("a+", "", wtf8("xaaay"));
    let program = BoundProgram::new(&owner, &mut Budget::new(1_000_000)).unwrap();
    let subject = BoundSubject::new(&owner).unwrap();
    let resources = BoundResources {
        program: &program,
        subject: &subject,
    };
    // Frames too small for the search's retries: a capacity request, which the
    // restart must clear.
    let mut cramped = Buffers::new();
    cramped.frames.clear();
    cramped.undo.clear();
    let mut search = Search::new(&resources, 0, cramped, Budget::new(1_000_000)).unwrap();
    let blocked = search.advance(64);
    assert!(matches!(
        blocked,
        Err(SearchError::Execution(ExecError::Frames | ExecError::Undo))
    ));
    search.restart_at(0);
    let search = search
        .rebuffer(Buffers::new())
        .unwrap_or_else(|_| panic!("replacement scratch is sufficient"));
    let outcome = finish(search, &owner);
    assert!(outcome.matched);
    assert_eq!(outcome.captures[0], Span::new(1, 4));

    // A cancelled search restarts, and a start past the end is no match.
    let mut search = Search::new(&resources, 0, Buffers::new(), Budget::new(1_000_000)).unwrap();
    search.cancel();
    assert!(matches!(
        search.advance(64),
        Err(SearchError::Execution(ExecError::Cancelled))
    ));
    search.restart_at(5);
    assert_eq!(search.advance(64).unwrap(), Progress::NoMatch);
    search.restart_at(6);
    assert_eq!(search.advance(64).unwrap(), Progress::NoMatch);
    search.restart_at(1);
    assert_eq!(search.advance(64).unwrap(), Progress::Matched);
}

#[test]
fn a_position_from_another_subject_is_refused() {
    let owner = Owner::new("a", "", wtf8("éa"));
    let other = Owner::new("a", "", wtf8("éaé"));
    // The same layout, three bytes and two units, but where `éa` has a
    // character boundary after `é`, `aé` has the middle of `é`.
    let same_layout = Owner::new("a", "", wtf8("aé"));
    let program = BoundProgram::new(&owner, &mut Budget::new(1_000_000)).unwrap();
    let subject = BoundSubject::new(&owner).unwrap();
    let other_subject = BoundSubject::new(&other).unwrap();
    let same_subject = BoundSubject::new(&same_layout).unwrap();
    let resources = BoundResources {
        program: &program,
        subject: &subject,
    };

    let foreign = every_position(&other_subject, 3)[1];
    assert!(matches!(
        Search::new_near(&resources, 1, foreign, Buffers::new(), Budget::new(1_000)),
        Err(SearchError::Execution(ExecError::ChangedResources))
    ));
    assert!(matches!(
        BoundSpan::new_near(&subject, Span::new(1, 2).unwrap(), foreign),
        Err(ReadError::ChangedPosition)
    ));

    // `éa` after one unit is byte two; in `aé` byte two is a continuation byte.
    let misplaced = every_position(&subject, 2)[1];
    let same_resources = BoundResources {
        program: &program,
        subject: &same_subject,
    };
    let mut search = Search::new_near(
        &same_resources,
        1,
        misplaced,
        Buffers::new(),
        Budget::new(1_000),
    )
    .unwrap_or_else(|_| panic!("the layout matches, so construction accepts it"));
    let mut outcome = Ok(Progress::Pending);
    for _ in 0..16 {
        outcome = search.advance(1);
        if !matches!(outcome, Ok(Progress::Pending)) {
            break;
        }
    }
    assert!(matches!(
        outcome,
        Err(SearchError::Execution(ExecError::ChangedResources))
    ));
    assert!(matches!(
        BoundSpan::new_near(&same_subject, Span::new(1, 2).unwrap(), misplaced),
        Err(ReadError::ChangedPosition)
    ));
}

/// A span reader from a position reads exactly what one without reads, and a
/// capture next to the position costs its distance rather than a seek from an
/// end.
#[test]
fn a_span_reader_from_a_position_reads_the_same_units() {
    let text = "é😀a".repeat(400);
    let owner = Owner::new("a", "", wtf8(&text));
    let subject = BoundSubject::new(&owner).unwrap();
    let length = owner.with_input(|_, input| input.len_utf16());
    let positions = every_position(&subject, length);
    let read = |reader: &mut BoundSpan<'_, &Owner>| {
        let mut units = Vec::new();
        let mut budget = Budget::new(usize::MAX);
        loop {
            let progress = reader
                .try_fold(7, &mut budget, |unit| {
                    units.push(unit);
                    Ok::<_, ()>(())
                })
                .unwrap_or_else(|_| panic!("read failed"));
            if progress == ReadProgress::Complete {
                return (units, usize::MAX - budget.remaining());
            }
            owner.relocate();
        }
    };
    for (start, end) in [(0, 0), (1, 3), (600, 605), (1_000, 1_600), (1_599, 1_600)] {
        let span = Span::new(start, end).unwrap();
        let (expected, plain) = read(&mut BoundSpan::new(&subject, span).unwrap());
        for near in [0, start.saturating_sub(2), start, start + 3, length] {
            let near = positions[near.min(length)];
            let mut reader = BoundSpan::new_near(&subject, span, near).unwrap();
            let (units, work) = read(&mut reader);
            assert_eq!(units, expected, "{start}..{end} near {}", near.utf16());
            assert!(work <= plain, "{start}..{end} near {}", near.utf16());
            assert_eq!(reader.position().utf16(), end);
        }
        if start >= 2 && end < length {
            let (_, work) =
                read(&mut BoundSpan::new_near(&subject, span, positions[start - 2]).unwrap());
            assert_eq!(work, 2 + (end - start), "{start}..{end}");
        }
    }
    assert!(owner.moves.get() > 0);
}
