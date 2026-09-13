//! Binding storage the host has already validated: a subject from its known
//! UTF-16 length, and a program from the witness of an earlier validation. See
//! `docs/binding.md`.
use perex::{
    Budget,
    binding::{
        BoundProgram, BoundProgramError, BoundResources, BoundSubject, ImmutableProgram,
        ImmutableSubject, Subject, SubjectError,
    },
    compiler::{Node, Range, compile},
    executor::{Frame, Progress, Resources, Scratch, ScratchOwner, Search, SearchError, Undo},
    input::{Input, Position},
    span::Span,
};
use std::cell::{Cell, RefCell};

#[derive(Clone, Debug)]
enum Storage {
    Bytes(Vec<u8>),
    Units(Vec<u16>),
}

/// Program and subject storage that moves at every pause, poisoning and
/// freeing what it moved from, and counts how often it is borrowed.
#[derive(Debug)]
struct Owner {
    data: RefCell<(Vec<u32>, Storage)>,
    borrows: Cell<usize>,
}

fn words(pattern: &str, flags: &str) -> Vec<u32> {
    let mut nodes = vec![Node::default(); pattern.len() * 4 + 128];
    let mut ranges = vec![Range::default(); pattern.len() * 4 + 128];
    let mut words = vec![0; pattern.len() * 48 + 256];
    compile(
        Input::utf8(pattern),
        flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut Budget::new(10_000_000),
    )
    .unwrap_or_else(|e| panic!("/{pattern}/{flags}: {e:?}"))
    .words()
    .to_vec()
}

impl Owner {
    fn new(words: Vec<u32>, storage: Storage) -> Self {
        Self {
            data: RefCell::new((words, storage)),
            borrows: Cell::new(0),
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
    }
}

impl ImmutableProgram for Owner {
    type Error = ();
    fn with_words<T>(&self, f: impl FnOnce(&[u32]) -> T) -> Result<T, ()> {
        self.borrows.set(self.borrows.get() + 1);
        Ok(f(&self.data.borrow().0))
    }
}

impl ImmutableSubject for Owner {
    type Error = ();
    fn with_subject<T>(&self, f: impl FnOnce(Subject<'_>) -> T) -> Result<T, ()> {
        self.borrows.set(self.borrows.get() + 1);
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

impl ScratchOwner for Buffers {
    fn scratch(&mut self) -> Scratch<'_> {
        Scratch {
            registers: &mut self.registers,
            frames: &mut self.frames,
            undo: &mut self.undo,
        }
    }
}

fn buffers() -> Buffers {
    Buffers {
        registers: vec![0xcafe; 64],
        frames: vec![Frame::default(); 1024],
        undo: vec![Undo::default(); 4096],
    }
}

/// A search's answer, captures, remaining work and position.
type Outcome = (bool, Vec<Option<Span>>, usize, Position);

fn run<R: Resources>(mut search: Search<'_, R, Buffers>, owner: &Owner) -> Outcome
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
    if progress == Progress::Matched {
        search.copy_captures(&mut captures).unwrap();
    }
    (
        progress == Progress::Matched,
        captures,
        search.remaining_work(),
        search.position(),
    )
}

fn wtf8(text: &str) -> Storage {
    Storage::Bytes(text.as_bytes().to_vec())
}

fn utf16_len(storage: &Storage) -> usize {
    match storage {
        Storage::Bytes(bytes) => Input::wtf8(bytes).unwrap().len_utf16(),
        Storage::Units(units) => units.len(),
    }
}

/// A binding from a known length and a witness is the binding validation
/// would have made. Every pattern, from every start, gives the same answer,
/// captures, work and position over both, with storage moving at every pause,
/// and a position from either is accepted by the other.
#[test]
fn counted_and_witnessed_bindings_find_what_validated_ones_find() {
    let mut lone = b"a\xed\xa0\xbd\xc3\xa9x".to_vec();
    lone.extend_from_slice("😀a".as_bytes());
    let subjects = [
        wtf8(""),
        wtf8("ab ax a"),
        wtf8("aé éxa"),
        wtf8("é😀a😀😀 xa"),
        Storage::Bytes(lone),
        Storage::Units("a😀é xa".encode_utf16().collect()),
        Storage::Units(vec![0xd83d, b'a' as u16, 0xde00]),
    ];
    let patterns = [
        ("a", ""),
        ("(é+)(x?)", ""),
        ("[😀-🙏]+", "u"),
        (".", "u"),
        ("(?<=é)a", ""),
        ("\\b\\w+", ""),
        ("(a).*\\1", ""),
        ("a$", "m"),
        ("a", "y"),
        ("\\ud83d", ""),
        ("(?:)", "u"),
    ];
    let mut compared = 0;
    for storage in &subjects {
        for (pattern, flags) in patterns {
            let owner = Owner::new(words(pattern, flags), storage.clone());
            let length = utf16_len(storage);
            let validated = BoundProgram::new(&owner, &mut Budget::new(10_000_000)).unwrap();
            let subject = BoundSubject::new(&owner).unwrap();
            let witnessed = BoundProgram::new_witnessed(&owner, validated.witness()).unwrap();
            let counted = BoundSubject::new_counted(&owner, length).unwrap();
            let checked = BoundResources {
                program: &validated,
                subject: &subject,
            };
            let trusted = BoundResources {
                program: &witnessed,
                subject: &counted,
            };
            for start in 0..=length + 1 {
                let context = format!("/{pattern}/{flags} over {storage:?} from {start}");
                let budget = Budget::new(1_000_000);
                let expected = run(
                    Search::new(&checked, start, buffers(), budget).unwrap(),
                    &owner,
                );
                let actual = run(
                    Search::new(&trusted, start, buffers(), budget).unwrap(),
                    &owner,
                );
                assert_eq!(expected.0, actual.0, "{context}");
                assert_eq!(expected.1, actual.1, "{context}");
                assert_eq!(expected.2, actual.2, "{context}");
                assert_eq!(expected.3, actual.3, "{context}");
                let crossed = run(
                    Search::new_near(&trusted, start, expected.3, buffers(), budget)
                        .unwrap_or_else(|_| panic!("{context}: position refused")),
                    &owner,
                );
                assert_eq!(
                    (expected.0, &expected.1),
                    (crossed.0, &crossed.1),
                    "{context}"
                );
                compared += 1;
            }
        }
    }
    assert!(compared > 500, "{compared}");
}

/// What is constant work to check is checked, and the storage comes back.
#[test]
fn a_length_or_witness_that_cannot_be_right_is_refused() {
    let refused = |storage: Storage, length: usize| {
        let owner = Owner::new(words("a", ""), storage);
        match BoundSubject::new_counted(&owner, length) {
            Ok(_) => false,
            Err(error) => {
                assert!(std::ptr::eq(error.storage, &owner));
                matches!(error.error, SubjectError::ChangedLayout)
            }
        }
    };
    // Units must match exactly.
    assert!(refused(Storage::Units(vec![97, 98]), 3));
    assert!(refused(Storage::Units(vec![97, 98]), 1));
    assert!(!refused(Storage::Units(vec![97, 98]), 2));
    // Bytes must be a length that many units could occupy.
    assert!(refused(wtf8("ab"), 3));
    assert!(refused(wtf8("中中"), 1));
    assert!(!refused(wtf8("中中"), 2));
    assert!(!refused(wtf8("😀"), 2));
    assert!(!refused(wtf8(""), 0));
    assert!(refused(wtf8(""), 1));

    let owner = Owner::new(words("a+", ""), wtf8("aaa"));
    let witness = BoundProgram::new(&owner, &mut Budget::new(1_000_000))
        .unwrap()
        .witness();
    for other in [words("b+", ""), words("a+b", "")] {
        let other = Owner::new(other, wtf8("aaa"));
        let error = BoundProgram::new_witnessed(&other, witness)
            .err()
            .expect("a witness of another program is refused");
        assert!(std::ptr::eq(error.storage, &other));
        assert!(matches!(error.error, BoundProgramError::ChangedLayout));
    }
}

/// The point of both is that nothing is scanned, so what they do not check is
/// stated here rather than implied: bytes that are not generalized UTF-8, and
/// program words behind an unchanged header, are accepted. Each takes exactly
/// one borrow of its owner.
#[test]
fn counted_and_witnessed_bindings_trust_what_they_do_not_check() {
    let invalid = Owner::new(words("a", ""), Storage::Bytes(vec![0xff, b'a', 0xc3]));
    assert!(BoundSubject::new(&invalid).is_err());
    invalid.borrows.set(0);
    assert!(BoundSubject::new_counted(&invalid, 2).is_ok());
    assert_eq!(invalid.borrows.get(), 1);

    let original = words("(a)(b)", "");
    let owner = Owner::new(original.clone(), wtf8("ab"));
    let witness = BoundProgram::new(&owner, &mut Budget::new(1_000_000))
        .unwrap()
        .witness();
    // An instruction's operand, past the header, rewritten to one validation
    // rejects.
    let mut changed = original;
    let last = changed.len() - 1;
    changed[last] = u32::MAX;
    let changed = Owner::new(changed, wtf8("ab"));
    assert!(BoundProgram::new(&changed, &mut Budget::new(1_000_000)).is_err());
    changed.borrows.set(0);
    assert!(BoundProgram::new_witnessed(&changed, witness).is_ok());
    assert_eq!(changed.borrows.get(), 1);
}
