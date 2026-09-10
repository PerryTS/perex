use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{ExecError, Frame, Progress, Resources, Scratch, Search, SearchError, Undo, find},
    input::Input,
    program::Program,
    span::Span,
};
use std::cell::{Cell, RefCell};

#[derive(Clone)]
enum Subject {
    Bytes(Vec<u8>),
    Units(Vec<u16>),
}
impl Subject {
    fn input(&self) -> Input<'_> {
        match self {
            Self::Bytes(b) => Input::wtf8(b).unwrap(),
            Self::Units(u) => Input::utf16(u),
        }
    }
    fn poison(&mut self) {
        match self {
            Self::Bytes(b) => b.fill(0xff),
            Self::Units(u) => u.fill(0xeeee),
        }
    }
    fn address(&self) -> usize {
        match self {
            Self::Bytes(b) => b.as_ptr() as usize,
            Self::Units(u) => u.as_ptr() as usize,
        }
    }
    fn is_empty(&self) -> bool {
        match self {
            Self::Bytes(b) => b.is_empty(),
            Self::Units(u) => u.is_empty(),
        }
    }
}
struct Owner {
    label: String,
    data: RefCell<(Vec<u32>, Subject)>,
    fail_borrow: Cell<bool>,
    borrows: Cell<usize>,
    moves: Cell<usize>,
}
impl Owner {
    fn new(pattern: &str, flags: &str, subject: Subject) -> Self {
        let mut nodes = vec![Node::default(); pattern.len() * 4 + 128];
        let mut ranges = vec![Range::default(); pattern.len() * 4 + 128];
        let mut words = vec![0; pattern.len() * 48 + 256];
        let p = compile(
            Input::utf8(pattern),
            flags,
            &mut nodes,
            &mut ranges,
            &mut words,
            &mut Budget::new(10_000_000),
        )
        .unwrap();
        let keep = p.words().to_vec();
        words.fill(0xdeadbeef);
        Self {
            label: format!("{pattern:?}/{flags}"),
            data: RefCell::new((keep, subject)),
            fail_borrow: Cell::new(false),
            borrows: Cell::new(0),
            moves: Cell::new(0),
        }
    }
    fn relocate(&self) {
        let mut data = self.data.borrow_mut();
        let replacement = data.clone();
        assert_ne!(data.0.as_ptr(), replacement.0.as_ptr());
        if !data.1.is_empty() {
            assert_ne!(data.1.address(), replacement.1.address());
        }
        data.0.fill(0xdeadbeef);
        data.1.poison();
        *data = replacement; // Drop both poisoned original allocations now.
        self.moves.set(self.moves.get() + 1);
    }
}
impl Resources for Owner {
    type Error = &'static str;
    fn with_views<T>(
        &self,
        use_views: impl FnOnce(Program<'_>, Input<'_>) -> T,
    ) -> Result<T, Self::Error> {
        if self.fail_borrow.replace(false) {
            return Err("borrow unavailable");
        }
        self.borrows.set(self.borrows.get() + 1);
        let data = self.data.borrow();
        let p = Program::from_words(&data.0, &mut Budget::new(10_000_000)).unwrap();
        Ok(use_views(p, data.1.input()))
    }
}

fn compare(owner: &Owner, start: usize, quantum: usize) -> usize {
    let mut registers = vec![0; 1024];
    let mut frames = vec![Frame::default(); 2048];
    let mut undo = vec![Undo::default(); 8192];
    let count = owner.with_views(|p, _| p.capture_count()).unwrap();
    let sentinel = Some(Span::new(901, 902).unwrap());
    let mut expected = vec![sentinel; count];
    let mut budget = Budget::new(2_000_000);
    let matched = owner
        .with_views(|p, input| {
            find(
                p,
                input,
                start,
                Scratch {
                    registers: &mut registers,
                    frames: &mut frames,
                    undo: &mut undo,
                },
                &mut expected,
                &mut budget,
            )
        })
        .unwrap()
        .unwrap();
    let mut search = Search::new(
        owner,
        start,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        Budget::new(2_000_000),
    )
    .unwrap();
    let mut output = vec![sentinel; count];
    let mut pauses = 0;
    loop {
        let before = search.remaining_work();
        let progress = search.advance(quantum).unwrap();
        if progress == Progress::Pending {
            assert!(
                search.remaining_work() < before,
                "positive quantum must make progress"
            );
            assert!(before - search.remaining_work() <= quantum.saturating_add(8448));
            assert_eq!(
                search.copy_captures(&mut output),
                Err(ExecError::NotMatched)
            );
            assert_eq!(output, vec![sentinel; count]);
            owner.relocate();
            pauses += 1;
            assert!(pauses < 2_000_000);
        } else {
            assert_eq!(progress == Progress::Matched, matched);
            if matched {
                search.copy_captures(&mut output).unwrap();
            } else {
                assert_eq!(
                    search.copy_captures(&mut output),
                    Err(ExecError::NotMatched)
                );
            }
            assert_eq!(output, expected);
            assert_eq!(
                search.remaining_work(),
                budget.remaining(),
                "pauses must not replay work or reset the budget: {} start={start} quantum={quantum}",
                owner.label
            );
            let borrows = owner.borrows.get();
            assert_eq!(search.advance(1).unwrap(), progress);
            assert_eq!(
                owner.borrows.get(),
                borrows,
                "completed operations do not reacquire views"
            );
            return pauses;
        }
    }
}

#[test]
fn every_small_quantum_preserves_captures_and_half_pairs_after_relocation() {
    let cases = [
        ("(a|(b))+", "", "aba", 0),
        ("(?=(a+))a*b\\1", "", "baaabac", 0),
        ("(?!(a))b", "", "ab", 0),
        ("(?<=([ab]+)([bc]+))$", "", "abc", 0),
        ("^(a|aa)+b$", "", "aaaaaa", 0),
        ("(?:(?<x>a)|(?<x>b)|(?<x>c))\\k<x>", "", "bb", 0),
        ("((a?)*){2}b", "", "aab", 0),
        ("(.)\\1", "u", "😀😀", 0),
        ("(.)\\1", "uy", "😀😀", 1),
        (r"\ud83d(?:X|\ude00)", "", "😀", 0),
        (r"\ud83d(?=(\ude00))\1", "", "😀", 0),
        (r"(?<=(?:X|\ud83d)\ude00)", "", "😀", 0),
        ("x", "", "", 0),
        ("", "", "", 0),
        ("a", "", "abc", 100),
    ];
    for (pattern, flags, subject, start) in cases {
        for storage in [
            Subject::Bytes(subject.as_bytes().to_vec()),
            Subject::Units(subject.encode_utf16().collect()),
        ] {
            let owner = Owner::new(pattern, flags, storage);
            for quantum in [1, 2, 3, 7, 31, 256, usize::MAX] {
                compare(&owner, start, quantum);
            }
        }
    }
    let bytes = [0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80];
    let owner = Owner::new(r"\ud83d(?=(\ude00))\1", "", Subject::Bytes(bytes.to_vec()));
    assert!(compare(&owner, 0, 1) > 5);
}

#[test]
fn long_operations_pause_without_repeating_prefix_work() {
    let classes = (1..=80)
        .map(|n| format!("\\u{:04x}", n * 32 + 256))
        .collect::<String>();
    let cases = [
        ("needle".to_owned(), "", "x".repeat(800), 0),
        (
            "needle".to_owned(),
            "i",
            format!("{}NEEDLE", "x".repeat(800)),
            0,
        ),
        ("[a-z]".to_owned(), "", "0".repeat(800), 0),
        (format!("[{classes}]"), "", "x".to_owned(), 0),
        (format!("[{classes}]"), "u", "😀".repeat(80), 0),
        (
            "😀needle".to_owned(),
            "u",
            format!("{}😀needle{}", "é".repeat(80), "é".repeat(80)),
            0,
        ),
        (
            "😀needle".to_owned(),
            "u",
            format!("{}😀needle", "é".repeat(80)),
            0,
        ),
        ("(.{24})\\1".to_owned(), "u", "😀".repeat(48), 0),
        ("(?<=(.{24})\\1)$".to_owned(), "u", "😀".repeat(48), 0),
        (
            "x".to_owned(),
            "y",
            format!("{}x{}", "😀".repeat(48), "é".repeat(128)),
            96,
        ),
        ("(a?){40}b".to_owned(), "", "aaaab".to_owned(), 0),
    ];
    for (pattern, flags, subject, start) in cases {
        let owner = Owner::new(&pattern, flags, Subject::Bytes(subject.as_bytes().to_vec()));
        assert!(compare(&owner, start, 1) > 0);
        compare(&owner, start, 17);
    }
}

#[test]
fn zero_quantum_binding_failure_cancellation_and_budget_failure_are_explicit() {
    let owner = Owner::new("(a+)+b", "", Subject::Bytes(b"aaaaab".to_vec()));
    let mut registers = [0; 16];
    let mut frames = [Frame::default(); 128];
    let mut undo = [Undo::default(); 512];
    let mut search = Search::new(
        &owner,
        0,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        Budget::new(10000),
    )
    .unwrap();
    let before = owner.borrows.get();
    assert_eq!(search.advance(0).unwrap(), Progress::Pending);
    assert_eq!(owner.borrows.get(), before);
    assert_eq!(search.remaining_work(), 10000);
    owner.fail_borrow.set(true);
    assert!(matches!(
        search.advance(1),
        Err(SearchError::Resource("borrow unavailable"))
    ));
    assert_eq!(search.remaining_work(), 10000);
    assert_eq!(search.advance(1).unwrap(), Progress::Pending);
    owner.relocate();
    search.cancel();
    let mut output = [Some(Span::new(99, 100).unwrap()); 3];
    assert_eq!(search.copy_captures(&mut output), Err(ExecError::Cancelled));
    assert!(matches!(
        search.advance(1000),
        Err(SearchError::Execution(ExecError::Cancelled))
    ));
    assert_eq!(output, [Some(Span::new(99, 100).unwrap()); 3]);
    for allowance in 0..12 {
        let mut search = Search::new(
            &owner,
            0,
            Scratch {
                registers: &mut registers,
                frames: &mut frames,
                undo: &mut undo,
            },
            Budget::new(allowance),
        )
        .unwrap();
        loop {
            match search.advance(1) {
                Ok(Progress::Pending) => owner.relocate(),
                Err(SearchError::Execution(ExecError::WorkLimit)) => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(search.remaining_work(), 0);
        assert!(matches!(
            search.advance(usize::MAX),
            Err(SearchError::Execution(ExecError::WorkLimit))
        ));
        assert_eq!(search.copy_captures(&mut output), Err(ExecError::WorkLimit));
        assert_eq!(output, [Some(Span::new(99, 100).unwrap()); 3]);
    }
}

#[test]
fn changed_layout_and_frame_exhaustion_cannot_become_a_match_or_restart() {
    let owner = Owner::new("(a|b)", "", Subject::Bytes(b"a".to_vec()));
    let mut registers = [0; 16];
    let mut frames = [];
    let mut undo = [Undo::default(); 16];
    let mut search = Search::new(
        &owner,
        0,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        Budget::new(10000),
    )
    .unwrap();
    loop {
        match search.advance(1) {
            Ok(Progress::Pending) => owner.relocate(),
            Err(SearchError::Execution(ExecError::Frames)) => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(matches!(
        search.advance(1000),
        Err(SearchError::Execution(ExecError::Frames))
    ));
    let mut output = [Some(Span::new(99, 100).unwrap()); 2];
    assert_eq!(search.copy_captures(&mut output), Err(ExecError::Frames));
    assert_eq!(output, [Some(Span::new(99, 100).unwrap()); 2]);
    let mut frames = [Frame::default(); 16];
    let mut search = Search::new(
        &owner,
        0,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        Budget::new(10000),
    )
    .unwrap();
    assert_eq!(search.advance(1).unwrap(), Progress::Pending);
    // An intentionally broken owner violates immutable identity. The cheap
    // shape guard catches this length change; it cannot prove full identity.
    if let Subject::Bytes(bytes) = &mut owner.data.borrow_mut().1 {
        bytes.push(b'a');
    }
    assert!(matches!(
        search.advance(1000),
        Err(SearchError::Execution(ExecError::ChangedResources))
    ));
    assert_eq!(
        search.copy_captures(&mut output),
        Err(ExecError::ChangedResources)
    );
    assert_eq!(output, [Some(Span::new(99, 100).unwrap()); 2]);
}

struct BufferCounts {
    alive: Cell<usize>,
    drops: Cell<usize>,
}
struct OwnedBuffers<'a> {
    registers: Vec<usize>,
    frames: Vec<Frame>,
    undo: Vec<Undo>,
    counts: &'a BufferCounts,
    owner: &'a Owner,
}
impl<'a> OwnedBuffers<'a> {
    fn new(
        owner: &'a Owner,
        counts: &'a BufferCounts,
        registers: usize,
        frames: usize,
        undo: usize,
    ) -> Self {
        // Allocation/accounting is a host action and may move both resources.
        owner.relocate();
        counts.alive.set(counts.alive.get() + 1);
        Self {
            registers: vec![0xcafe; registers],
            frames: vec![Frame::default(); frames],
            undo: vec![Undo::default(); undo],
            counts,
            owner,
        }
    }
}
impl perex::executor::ScratchOwner for OwnedBuffers<'_> {
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
        self.counts.alive.set(self.counts.alive.get() - 1);
        self.counts.drops.set(self.counts.drops.get() + 1);
        // No input/program borrow may be live during old-owner cleanup.
        self.owner.relocate();
    }
}

#[test]
fn growing_owned_scratch_preserves_partial_updates_and_releases_previous_owners() {
    for (pattern, flags, subject) in [
        ("((a|(b))*)c", "", "ababac"),
        ("((a?){2})+b", "", "aaaab"),
        ("(?=(a+))a*b\\1", "", "baaabac"),
        ("^(a|aa)+b$", "", "aaaaaa"),
        (r"\ud83d((?:X|\ude00)+)", "", "😀"),
    ] {
        let owner = Owner::new(pattern, flags, Subject::Bytes(subject.as_bytes().to_vec()));
        let counts = BufferCounts {
            alive: Cell::new(0),
            drops: Cell::new(0),
        };
        let (registers, captures) = owner
            .with_views(|p, _| (p.register_count(), p.capture_count()))
            .unwrap();
        let mut expected = vec![None; captures];
        let mut reference_budget = Budget::new(2_000_000);
        let reference = owner
            .with_views(|p, input| {
                find(
                    p,
                    input,
                    0,
                    Scratch {
                        registers: &mut vec![0; registers],
                        frames: &mut vec![Frame::default(); 2048],
                        undo: &mut vec![Undo::default(); 8192],
                    },
                    &mut expected,
                    &mut reference_budget,
                )
            })
            .unwrap()
            .unwrap();
        let buffers = OwnedBuffers::new(&owner, &counts, registers, 0, 0);
        let mut search = Search::new(&owner, 0, buffers, Budget::new(2_000_000)).unwrap();
        let mut grows = 0;
        let mut saw_frames = false;
        let mut saw_undo = false;
        let mut checked_rejection = false;
        loop {
            match search.advance(3) {
                Ok(Progress::Pending) => owner.relocate(),
                Err(SearchError::Execution(error @ (ExecError::Frames | ExecError::Undo))) => {
                    saw_frames |= error == ExecError::Frames;
                    saw_undo |= error == ExecError::Undo;
                    let remaining = search.remaining_work();
                    assert!(
                        matches!(search.advance(100), Err(SearchError::Execution(e)) if e == error)
                    );
                    assert_eq!(remaining, search.remaining_work());
                    let need = search.required_scratch();
                    if !checked_rejection && need.frames > 0 {
                        let bad = OwnedBuffers::new(&owner, &counts, need.registers, 0, need.undo);
                        let rejected = match search.rebuffer(bad) {
                            Err(e) => e,
                            Ok(_) => panic!("insufficient frames accepted"),
                        };
                        assert_eq!(rejected.error, ExecError::Frames);
                        assert!(
                            rejected
                                .buffers
                                .registers
                                .iter()
                                .all(|&slot| slot == 0xcafe),
                            "failed rebinding wrote the destination"
                        );
                        search = rejected.search;
                        drop(rejected.buffers);
                        assert_eq!(counts.alive.get(), 1);
                        assert_eq!(search.remaining_work(), remaining);
                        checked_rejection = true;
                    }
                    let replacement = OwnedBuffers::new(
                        &owner,
                        &counts,
                        need.registers,
                        need.frames.next_power_of_two(),
                        if need.undo == 0 {
                            0
                        } else {
                            need.undo.next_power_of_two()
                        },
                    );
                    search = match search.rebuffer(replacement) {
                        Ok(s) => s,
                        Err(e) => panic!("rebinding failed: {:?}", e.error),
                    };
                    grows += 1;
                    assert_eq!(
                        counts.alive.get(),
                        1,
                        "old allocation retained after rebinding"
                    );
                    assert_eq!(search.remaining_work(), remaining);
                    assert!(grows < 64);
                }
                Ok(done) => {
                    assert_eq!(done == Progress::Matched, reference, "{pattern}");
                    let mut output = vec![None; captures];
                    if reference {
                        search.copy_captures(&mut output).unwrap();
                    }
                    assert_eq!(output, expected, "{pattern}");
                    assert_eq!(
                        search.remaining_work(),
                        reference_budget.remaining(),
                        "growth replayed work: {pattern}"
                    );
                    break;
                }
                Err(e) => panic!("unexpected {e:?}"),
            }
        }
        assert!(
            saw_frames && saw_undo && grows > 1,
            "missing growth path: {pattern}"
        );
        drop(search);
        assert_eq!(counts.alive.get(), 0);
        assert_eq!(counts.drops.get(), grows + 2); // Initial, replacements, rejected buffer.
    }
}
