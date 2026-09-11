use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{ExecError, Frame, Scratch, Undo, find},
    input::Input,
    program::{Program, ProgramError},
    span::Span,
};
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
fn run(
    words: &[u32],
    input: Input<'_>,
    start: usize,
    work: usize,
) -> Result<Option<Vec<Option<Span>>>, ExecError> {
    let p = Program::from_words(words, &mut Budget::new(100_000)).unwrap();
    let mut registers = vec![0; p.register_count()];
    let mut frames = vec![Frame::default(); 1024];
    let mut undo = vec![Undo::default(); 8192];
    let mut captures = vec![Span::new(7, 9); p.capture_count()];
    let result = find(
        p,
        input,
        start,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        &mut captures,
        &mut Budget::new(work),
    );
    if result != Ok(true) {
        assert!(captures.iter().all(|c| *c == Span::new(7, 9)));
    }
    result.map(|found| found.then_some(captures))
}
#[test]
fn candidate_ranges_preserve_nullable_prefixes_captures_assertions_and_case() {
    for (source, flags) in [
        ("needle", ""),
        ("(a)?(b+)", ""),
        ("a*b|c+d", ""),
        ("(?:a|)b", ""),
        ("(?=(a+))a*b\\1", ""),
        ("(?<=a+)b", ""),
        ("(?!a)b", ""),
        ("(a)?\\1b", ""),
        ("(?:(?<x>a)|(?<x>b))+", ""),
        ("^a|b", "m"),
        ("[a-f]+", "i"),
        ("[\\p{Lu}]+", "ui"),
        ("[^a-z]+", "i"),
        ("(?i:K)(?-i:S)", "u"),
        ("(?i:[Kſ])", "u"),
        ("(?i:[Kſ])", ""),
        ("(?:a?)*", ""),
        ("(a?){2}b", ""),
        ("\\p{White_Space}", "u"),
        ("[^\\x00-\\x7f]", ""),
        ("\\P{ASCII}", "ui"),
        ("a", "y"),
    ] {
        let enabled = words(source, flags);
        let mut disabled = enabled.clone();
        disabled[7] = 0;
        for tail in [
            "", "needle", "aaab", "bc", "KS", "ks", "bba", "\nAa", "😀b", "ſK",
        ] {
            for prefix in [
                "".to_owned(),
                "x".repeat(8),
                "x".repeat(255),
                "x".repeat(256),
            ] {
                let subject = prefix.clone() + tail;
                let units: Vec<_> = subject.encode_utf16().collect();
                for start in [0, prefix.len(), units.len(), units.len() + 1] {
                    let expected = run(&disabled, Input::utf8(&subject), start, 2_000_000);
                    assert!(
                        expected.is_ok(),
                        "reference execution failed: {source}/{flags} {subject:?}"
                    );
                    assert_eq!(
                        run(&enabled, Input::utf8(&subject), start, 2_000_000),
                        expected,
                        "{source}/{flags}, {subject:?}, {start}"
                    );
                    assert_eq!(
                        run(&enabled, Input::utf16(&units), start, 2_000_000),
                        expected,
                        "original UTF16 {source}/{flags}"
                    );
                }
            }
        }
    }
}
#[test]
fn candidate_skipping_is_required_for_the_bounded_long_miss() {
    let enabled = words("needle", "");
    let mut disabled = enabled.clone();
    disabled[7] = 0;
    let text = "x".repeat(4096);
    assert_eq!(run(&enabled, Input::utf8(&text), 0, 4200), Ok(None));
    assert_eq!(
        run(&disabled, Input::utf8(&text), 0, 4200),
        Err(ExecError::WorkLimit)
    );
    assert_eq!(
        run(&enabled, Input::utf8(&text), 0, 0),
        Err(ExecError::WorkLimit)
    );
    let impossible = words("[^\\x00-\\x7f]", "");
    assert_eq!(impossible[7], 1);
    assert_eq!(run(&impossible, Input::utf8(&text), 0, 2), Ok(None));
}
#[test]
fn descriptors_are_validated_and_old_formats_rejected() {
    let original = words("needle", "");
    for descriptor in [3, 2 | (9 << 8) | (8 << 16), 2 | (128 << 16), 2 | (1 << 24)] {
        let mut bad = original.clone();
        bad[7] = descriptor;
        assert_eq!(
            Program::from_words(&bad, &mut Budget::new(1000)).unwrap_err(),
            ProgramError::Invalid
        );
    }
    let mut old = original.clone();
    old[1] = 7;
    assert_eq!(
        Program::from_words(&old, &mut Budget::new(1000)).unwrap_err(),
        ProgramError::Invalid
    );
}

#[test]
fn mixed_subject_starts_preserve_half_pairs_and_utf16_capture_offsets() {
    for (source, flags) in [
        ("(needle)", ""),
        ("(needle)", "uy"),
        ("[^\\x00-\\x7f]", ""),
        ("[^\\x00-\\x7f]", "u"),
        ("\\uDC00", ""),
        ("\\uDC00", "y"),
        ("(?i:K)", "u"),
        ("(?<=éx{8})(needle)", "u"),
        ("(?<x>needle)\\k<x>", ""),
    ] {
        let enabled = words(source, flags);
        let mut disabled = enabled.clone();
        disabled[7] = 0;
        for prefix in ["é", "😀", "𐀀", "K"] {
            for size in [0, 7, 8, 9, 255, 256, 257] {
                let subject = format!("{prefix}{}needleneedle{prefix}", "x".repeat(size));
                let units: Vec<_> = subject.encode_utf16().collect();
                for start in [0, 1, 2, size + 1, size + 2, units.len()] {
                    let expected = run(&disabled, Input::utf8(&subject), start, 2_000_000);
                    assert_eq!(
                        run(&enabled, Input::utf8(&subject), start, 2_000_000),
                        expected,
                        "{source}/{flags}, {subject:?}, start={start}"
                    );
                    assert_eq!(
                        run(&enabled, Input::utf16(&units), start, 2_000_000),
                        expected
                    );
                }
            }
        }
    }
    let enabled = words("needle", "");
    let mut disabled = enabled.clone();
    disabled[7] = 0;
    let text = format!("é{}", "x".repeat(4096));
    assert_eq!(run(&enabled, Input::utf8(&text), 0, 4300), Ok(None));
    assert_eq!(
        run(&disabled, Input::utf8(&text), 0, 4300),
        Err(ExecError::WorkLimit)
    );
}
