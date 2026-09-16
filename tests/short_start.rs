//! A short ASCII remainder is scanned once, on entering admission, for a byte a
//! match can begin with: none decides the search, and the first one found is
//! where it starts. See `docs/candidate.md`.
use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{ExecError, Frame, Scratch, Undo, find},
    input::Input,
    span::Span,
};

fn words(source: &str, flags: &str) -> Vec<u32> {
    let mut nodes = vec![Node::default(); source.len() * 3 + 64];
    let mut ranges = vec![Range::default(); source.len() * 12 + 64];
    let mut storage = vec![0; source.len() * 48 + 256];
    compile(
        Input::utf8(source),
        flags,
        &mut nodes,
        &mut ranges,
        &mut storage,
        &mut Budget::new(2_000_000),
    )
    .unwrap_or_else(|e| panic!("/{source}/{flags}: {e:?}"))
    .words()
    .to_vec()
}

/// Every capture of a match, `None` for no match, or the error.
type Answer = Result<Option<Vec<Option<Span>>>, ExecError>;

/// A search's answer, with the work it charged.
fn run(words: &[u32], subject: &str, start: usize) -> (Answer, usize) {
    let p = perex::program::Program::from_words(words, &mut Budget::new(1_000_000)).unwrap();
    let mut registers = vec![0; p.register_count()];
    let mut frames = vec![Frame::default(); 4096];
    let mut undo = vec![Undo::default(); 32768];
    let mut captures = vec![None; p.capture_count()];
    let work = 10_000_000;
    let mut budget = Budget::new(work);
    let found = find(
        p,
        Input::utf8(subject),
        start,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        &mut captures,
        &mut budget,
    );
    (
        found.map(|found| found.then(|| captures.clone())),
        work - budget.remaining(),
    )
}

/// ASCII subjects on both sides of the remainder the scan covers, with the
/// byte a pattern needs absent, first, last, in the middle, and repeated.
fn subjects() -> Vec<String> {
    let mut all = Vec::new();
    for len in [0, 1, 2, 7, 8, 9, 15, 16, 17, 31, 32, 62, 63, 64, 65, 80] {
        let fill = "-".repeat(len);
        all.push(fill.clone());
        for piece in ["a", "ab", "cat", "dog", "x1", "Z", "word", "é"] {
            if piece.len() > len {
                continue;
            }
            let rest = len - piece.len();
            for at in [0, rest / 2, rest] {
                all.push(format!("{}{piece}{}", &fill[..at], &fill[at..rest]));
            }
        }
        all.push("ab".repeat(len / 2));
    }
    all
}

const PATTERNS: &[(&str, &str)] = &[
    ("a", ""),
    ("ab", ""),
    ("b", ""),
    ("Z", ""),
    ("z", "i"),
    ("AB", "i"),
    ("[a-c]", ""),
    ("[x-z][0-9]", ""),
    ("cat|dog", ""),
    ("(a)(b)?", ""),
    ("(?<n>a)b", ""),
    ("(?<=-)a", ""),
    ("(?<!-)b", ""),
    ("a(?=b)", ""),
    ("ab$", ""),
    ("[a-z]{2}$", ""),
    ("b$", "m"),
    ("^a", ""),
    ("^a", "m"),
    ("a", "y"),
    ("ab", "y"),
    ("\\bword", ""),
    ("(a)\\1", ""),
    ("a+b", ""),
    ("-+a", ""),
    ("\\w+", ""),
    (".", ""),
    ("[é]", ""),
    ("[é]", "u"),
    ("a", "u"),
    ("\\p{L}", "u"),
    ("[^-]", ""),
    ("a*", ""),
    ("", ""),
];

#[test]
fn a_short_remainder_decides_the_same_answers_as_every_start() {
    let subjects = subjects();
    let mut decided = 0;
    for &(pattern, flags) in PATTERNS {
        let direct = words(pattern, flags);
        // An alternative that asserts nothing can match gives the pattern no
        // byte a match must begin with, so this form is searched as before.
        let reference = words(&format!("(?:{pattern}|(?!))"), flags);
        assert_eq!(reference[7], 0, "/{pattern}/{flags} reference still scans");
        decided += usize::from(direct[7] != 0);
        for subject in &subjects {
            for start in [0, 1, subject.len() / 2, subject.len(), subject.len() + 1] {
                let (answer, work) = run(&direct, subject, start);
                let (expected, reference_work) = run(&reference, subject, start);
                assert_eq!(
                    answer, expected,
                    "/{pattern}/{flags} on {subject:?} from {start}"
                );
                // Where the scan decides a miss, it reads each position once
                // where the search it replaces tried a start at each.
                if answer == Ok(None) && start <= subject.len() && subject.len() - start < 64 {
                    assert!(
                        work <= reference_work,
                        "/{pattern}/{flags} on {subject:?} from {start}: {work} > {reference_work}"
                    );
                }
            }
        }
    }
    assert!(decided > 20, "only {decided} patterns carry a start byte");
}

#[test]
fn a_short_miss_is_charged_the_positions_it_reads() {
    let program = words("z", "");
    for len in [1, 8, 32, 63] {
        let subject = "a".repeat(len);
        let (answer, work) = run(&program, &subject, 0);
        assert_eq!(answer, Ok(None));
        assert_eq!(work, len, "{len} positions");
    }
    // Past the remainder the scan covers, the search seeks and scans as before.
    let (answer, work) = run(&program, &"a".repeat(64), 0);
    assert_eq!(answer, Ok(None));
    assert!(work > 64, "{work}");
}

#[test]
fn a_short_start_keeps_the_end_bound() {
    // A match of `ab$` begins two units before the end, so a candidate at the
    // subject's start is not where a search needs to begin.
    let program = words("ab$", "");
    let subject = format!("a{}xb", "-".repeat(40));
    let (answer, work) = run(&program, &subject, 0);
    assert_eq!(answer, Ok(None));
    assert!(work < 16, "{work}");
}
