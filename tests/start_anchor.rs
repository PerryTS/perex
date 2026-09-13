//! A program anchored at the subject's start: every successful match asserts
//! `^` without `m` before consuming anything, so a search tries only its first
//! start. See `docs/candidate.md`.
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
fn run(words: &[u32], subject: Input<'_>, start: usize, work: usize) -> (Answer, usize) {
    let p = perex::program::Program::from_words(words, &mut Budget::new(1_000_000)).unwrap();
    let mut registers = vec![0; p.register_count()];
    let mut frames = vec![Frame::default(); 4096];
    let mut undo = vec![Undo::default(); 32768];
    let mut captures = vec![None; p.capture_count()];
    let mut budget = Budget::new(work);
    let found = find(
        p,
        subject,
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

/// Subjects of each storage kind, short and past the admission threshold.
fn subjects() -> Vec<(String, Vec<u8>, Vec<u16>)> {
    let texts = [
        "abc".to_string(),
        "xabc".to_string(),
        "".to_string(),
        "\n".to_string(),
        "ab\nabc".to_string(),
        "record_123457".to_string(),
        "!bad_123456".to_string(),
        "😀a😀b".to_string(),
        "éabc".to_string(),
        format!("abc{}", "x1_".repeat(40)),
        format!("{}abc", "x1_".repeat(40)),
        format!("😀{}", "é".repeat(70)),
    ];
    texts
        .into_iter()
        .map(|t| {
            let units = t.encode_utf16().collect();
            let bytes = t.clone().into_bytes();
            (t, bytes, units)
        })
        .collect()
}

/// A start-anchored program finds exactly what the same pattern finds as a
/// branch beside one that never matches, which no derivation treats as
/// anchored, from every start of every subject in every storage.
#[test]
fn anchoring_never_changes_an_answer() {
    let patterns = [
        ("^abc", ""),
        ("^a", "i"),
        ("^[a-z]+_[0-9]+$", ""),
        ("(^a)(b)?", ""),
        ("^a|^b", ""),
        ("(?:^ab|^x)c?", ""),
        ("^", ""),
        ("^$", ""),
        ("^😀", "u"),
        ("^.", "u"),
        ("^\\ud83d", ""),
        ("^(?:a|😀)", "u"),
        ("^(?=a)", ""),
        ("^a", "y"),
        ("^.*", "s"),
        // Not anchored, so they must keep trying later starts.
        ("^a|b", ""),
        ("^a", "m"),
        ("(?:^)?a", ""),
        ("(?<=^)a", ""),
        ("(?:^a)+", ""),
        ("a|^b", ""),
        ("(?m:^a)", ""),
    ];
    let mut compared = 0;
    for (source, flags) in patterns {
        let program = words(source, flags);
        let reference = words(&format!("(?:{source}|(?!))"), flags);
        for (text, bytes, units) in subjects() {
            let inputs = [Input::wtf8(&bytes).unwrap(), Input::utf16(&units)];
            for input in inputs {
                for start in 0..=input.len_utf16() + 1 {
                    let expected = run(&reference, input, start, 10_000_000).0;
                    let actual = run(&program, input, start, 10_000_000).0;
                    assert_eq!(
                        actual, expected,
                        "/{source}/{flags} over {text:?} from {start}"
                    );
                    compared += 1;
                }
            }
        }
    }
    assert!(compared > 10_000, "{compared}");
}

/// A failing start-anchored search costs what its one attempt costs, not a try
/// at every later start, whatever the subject's length and storage and the
/// requested start. ASCII storage reaches the candidate scan; other storage and
/// a program with no start descriptor, like `^\b`, reach the next-start step.
#[test]
fn a_failed_anchor_does_not_try_every_start() {
    let ascii = format!("!bad_{}", "x1_".repeat(30_000));
    let mixed = format!("!bad_{}", "éx1_".repeat(30_000));
    let units: Vec<u16> = mixed.encode_utf16().collect();
    // No position a start descriptor admits, for longer than one scan chunk.
    let bare = "!".repeat(10_000);
    let inputs = [
        ("ASCII", Input::utf8(&ascii)),
        ("no candidate", Input::utf8(&bare)),
        ("non-ASCII", Input::utf8(&mixed)),
        ("UTF-16", Input::utf16(&units)),
    ];
    for (source, flags) in [
        ("^[a-z]", ""),
        ("^[a-z]+_[0-9]+$", ""),
        ("^(?:ab|cd)x", ""),
        ("^\\b", ""),
        ("^(?:ab|x)", "i"),
    ] {
        let program = words(source, flags);
        let reference = words(&format!("(?:{source}|(?!))"), flags);
        for (kind, input) in inputs {
            let length = input.len_utf16();
            for start in [0, 1, length / 2, length] {
                let (found, work) = run(&program, input, start, 1_000_000);
                assert_eq!(
                    found,
                    Ok(None),
                    "/{source}/{flags} over {kind} from {start}"
                );
                assert!(
                    work < 500,
                    "/{source}/{flags} over {kind} from {start} charged {work}"
                );
            }
            // The same search as an unanchorable branch still tries them all.
            let (_, work) = run(&reference, input, 0, 100_000_000);
            assert!(
                work > 50_000,
                "/{source}/{flags} over {kind}: reference charged only {work}"
            );
        }
    }
}
