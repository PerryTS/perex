//! The forward-admission bound: every successful match consumes the admission
//! condition at or after its own start, so a start with no later occurrence
//! cannot match, and neither can any start after it. These checks cover the
//! compiler's claim, the answers the bound must preserve, and its behaviour
//! under work exhaustion and relocation.
use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{ExecError, Frame, Scratch, Undo, find},
    input::Input,
    program::Program,
    span::Span,
};

const FORWARD: u32 = 1 << 31;

fn words(source: &str, flags: &str) -> Vec<u32> {
    let mut nodes = vec![Node::default(); source.len() * 3 + 32];
    let mut ranges = vec![Range::default(); source.len() * 12 + 32];
    let mut storage = vec![0; source.len() * 48 + 128];
    compile(
        Input::utf8(source),
        flags,
        &mut nodes,
        &mut ranges,
        &mut storage,
        &mut Budget::new(2_000_000),
    )
    .unwrap()
    .words()
    .to_vec()
}

fn forward(source: &str, flags: &str) -> bool {
    words(source, flags)[9] & FORWARD != 0
}

fn run(words: &[u32], subject: &str, start: usize, work: usize) -> Result<Option<Span>, ExecError> {
    let p = Program::from_words(words, &mut Budget::new(1_000_000)).unwrap();
    let mut registers = vec![0; p.register_count()];
    let mut frames = vec![Frame::default(); 4096];
    let mut undo = vec![Undo::default(); 32768];
    let mut captures = vec![None; p.capture_count()];
    find(
        p,
        Input::utf8(subject),
        start,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        &mut captures,
        &mut Budget::new(work),
    )
    .map(|found| found.then(|| captures[0].unwrap()))
}

#[test]
fn only_a_condition_consumed_after_the_start_carries_the_claim() {
    for (source, flags) in [
        ("a+END", ""),
        ("x*END", ""),
        ("(?:a+END|b+END)", ""),
        ("a+[XY]", ""),
        ("(a+)(END)", ""),
    ] {
        assert!(forward(source, flags), "/{source}/{flags}");
    }
    for (source, flags) in [
        // A lookbehind body can satisfy the condition before the match start.
        ("(?<=END)a+", ""),
        // Lookahead is conservatively excluded with every other assertion.
        ("(?=.*END)a+", ""),
        // Without a repetition there is no admission condition at all.
        ("END", ""),
        ("[a-z]+", ""),
    ] {
        assert!(!forward(source, flags), "/{source}/{flags}");
    }
}

#[test]
fn a_condition_only_before_the_start_reports_no_match() {
    let w = words("a+END", "");
    // The condition is present, so presence alone admits the search; only its
    // position proves that no start can reach it.
    let subject = format!("END{}", "a".repeat(4000));
    assert_eq!(run(&w, &subject, 0, 10_000_000).unwrap(), None);
    // The same shape with the condition absent was already rejected.
    let subject = "a".repeat(4000);
    assert_eq!(run(&w, &subject, 0, 10_000_000).unwrap(), None);
}

#[test]
fn a_later_occurrence_is_found_after_an_earlier_one_is_passed() {
    let w = words("a+END", "");
    // The bound must advance past the occurrence at zero and keep searching.
    let subject = format!("END{}END", "a".repeat(60));
    assert_eq!(run(&w, &subject, 0, 10_000_000).unwrap(), Span::new(3, 66));

    // Several occurrences, with the match using the last one.
    let subject = format!("END{}ENDxEND", "a".repeat(60));
    assert_eq!(run(&w, &subject, 0, 10_000_000).unwrap(), Span::new(3, 66));

    // A match that begins after an occurrence it does not use.
    let subject = format!("{}bENDb{}aaaEND", "q".repeat(40), "q".repeat(40));
    let at = subject.find("aaaEND").unwrap();
    assert_eq!(
        run(&w, &subject, 0, 10_000_000).unwrap(),
        Span::new(at, subject.len())
    );
}

#[test]
fn requested_starts_and_sticky_matching_keep_their_answers() {
    let w = words("a+END", "");
    let subject = format!("{}aaaEND{}", "q".repeat(60), "q".repeat(10));
    let at = subject.find("aaaEND").unwrap();
    // Every requested start reports what the general search would. The run of
    // `a` is three long, so a start inside it still matches, shorter.
    for start in [0, 1, at - 1, at, at + 1, at + 2] {
        let expected = Span::new(start.max(at), at + 6);
        assert_eq!(
            run(&w, &subject, start, 10_000_000).unwrap(),
            expected,
            "{start}"
        );
    }
    // Past the run there is no `a` left to consume before the condition.
    assert_eq!(run(&w, &subject, at + 3, 10_000_000).unwrap(), None);
    // A start past the only occurrence has nothing left to reach.
    assert_eq!(run(&w, &subject, at + 7, 10_000_000).unwrap(), None);

    let sticky = words("a+END", "y");
    assert_eq!(run(&sticky, &subject, 0, 10_000_000).unwrap(), None);
    assert_eq!(
        run(&sticky, &subject, at, 10_000_000).unwrap(),
        Span::new(at, at + 6)
    );
}

/// A search from a late start pays for the subject from that start on, not for
/// the condition's occurrences before it. A global loop starts one search per
/// match, and each used to find the first occurrence from the subject's
/// beginning and then step through every later one to reach its start, which
/// made the loop quadratic.
#[test]
fn a_late_start_does_not_pay_for_earlier_occurrences() {
    let source = "[a-z]+[0-9]+ ";
    assert!(forward(source, ""));
    let bound = words(source, "");
    let mut unbounded = bound.clone();
    unbounded[9] &= !FORWARD;
    // The condition recurs every five units in one subject, and occurs only
    // once, at the end, in the other.
    let recurring = "ab12 ".repeat(4_000);
    let once = format!("{} ", "ab12".repeat(5_000));
    // Every start, over a subject past the admission threshold, for several
    // conditions: literal, folded, after an alternation, and at the subject's
    // edges.
    let short = "ab12 cdEND aaEND xyz12 END".repeat(5);
    for (source, flags) in [
        ("[a-z]+[0-9]+ ", ""),
        ("a+END", ""),
        ("(?:ab|cd)[0-9]* ", ""),
        ("[a-z]+end", "i"),
        ("x[a-z]*12 ", "y"),
    ] {
        let checked = words(source, flags);
        assert!(forward(source, flags), "/{source}/{flags}");
        let mut plain = checked.clone();
        plain[9] &= !FORWARD;
        for start in 0..=short.len() + 1 {
            assert_eq!(
                run(&checked, &short, start, 10_000_000).unwrap(),
                run(&plain, &short, start, 10_000_000).unwrap(),
                "/{source}/{flags} from {start}"
            );
        }
    }
    for subject in [recurring.as_str(), once.as_str()] {
        let length = subject.len();
        for start in [
            0,
            1,
            4,
            5,
            6,
            9_999,
            10_000,
            length - 10,
            length - 5,
            length - 1,
            length,
        ] {
            let expected = run(&unbounded, subject, start, 100_000_000).unwrap();
            assert_eq!(
                run(&bound, subject, start, 100_000_000).unwrap(),
                expected,
                "{start}"
            );
        }
        // Near the end, what is left is a few records, whatever came before.
        // From the beginning, the first scan alone would charge the subject.
        for start in [length - 10, length - 5] {
            assert!(
                run(&bound, subject, start, 2_000).is_ok(),
                "{start} of {length}"
            );
        }
    }
}

#[test]
fn a_lookbehind_condition_before_the_start_still_matches() {
    // The claim is absent here, so the condition's position must not bound the
    // search: the match begins after the text the lookbehind examines.
    let w = words("(?<=END)a+", "");
    assert!(!forward("(?<=END)a+", ""));
    let subject = format!("END{}", "a".repeat(100));
    assert_eq!(run(&w, &subject, 0, 10_000_000).unwrap(), Span::new(3, 103));
}

#[test]
fn the_bound_is_charged_and_resumes_across_relocation() {
    let source = "a+END";
    let subject = format!("END{}END", "a".repeat(60));
    let w = words(source, "");
    let complete = run(&w, &subject, 0, 10_000_000).unwrap();
    assert_eq!(complete, Span::new(3, 66));

    // An allowance too small to finish is an explicit outcome, never a wrong
    // answer, and never an unbounded scan.
    let mut exhausted = false;
    for work in 1..64 {
        match run(&w, &subject, 0, work) {
            Err(ExecError::WorkLimit) => exhausted = true,
            Ok(found) => assert_eq!(found, complete, "work {work}"),
            Err(other) => panic!("work {work}: {other:?}"),
        }
    }
    assert!(exhausted, "a small allowance must be able to run out");

    // The same answer must survive the program moving between borrows.
    let moved = w.clone();
    let mut poisoned = w.clone();
    poisoned.fill(0xdeadbeef);
    assert_eq!(run(&moved, &subject, 0, 10_000_000).unwrap(), complete);
}
