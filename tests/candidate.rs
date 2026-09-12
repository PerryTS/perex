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
    for slot in [7, 8] {
        for descriptor in [3, 2 | (9 << 8) | (8 << 16), 2 | (128 << 16)] {
            let mut bad = original.clone();
            bad[slot] = descriptor;
            assert_eq!(
                Program::from_words(&bad, &mut Budget::new(1000)).unwrap_err(),
                ProgramError::Invalid
            );
        }
    }
    // Word 7 has no field above its range; word 8's upper byte is the
    // end-anchored length bound, so the same bits are valid there.
    let mut bad = original.clone();
    bad[7] = 2 | (1 << 24);
    assert_eq!(
        Program::from_words(&bad, &mut Budget::new(1000)).unwrap_err(),
        ProgramError::Invalid
    );
    let mut bounded = original.clone();
    bounded[8] = original[8] | (7 << 24);
    assert!(Program::from_words(&bounded, &mut Budget::new(1000)).is_ok());
    for version in [7, 8, 9] {
        let mut bad = original.clone();
        bad[1] = version;
        assert_eq!(
            Program::from_words(&bad, &mut Budget::new(1000)).unwrap_err(),
            ProgramError::Invalid
        );
    }
}

#[test]
fn end_candidate_preserves_all_captures_and_optional_or_multiline_endings() {
    for source in [
        "^(a+)+$",
        "(a+)$",
        "(a)?$",
        "(?:a$|b$)",
        "(?:a$|b)",
        "a$(?:b?)",
        "a$(?:)",
        "a$(?=)",
        "a$(?!b)",
        "(?:a$)+",
        "(?:a$)*",
        "a{0}$",
        "(?:a{0})+$",
        "a(b)?$",
        "(a)?b$",
        "(a)?\\1$",
        "(?<x>a)?\\k<x>$",
        "(?=(a))\\1$",
        "(?<=a)(b)$",
        "(?m:a$)",
        "(?-m:a$)",
        "(?i:K)$",
        "(?i:[Kſ])$",
        "[^a-z]$",
        "[]$",
        "[^]$",
        "[^\\x00-\\x7f]$",
        "(?:a$|)$",
        "(?:a|)$",
        "\\uDC00$",
        "\\uD800$",
        "(😀)$",
    ] {
        for flags in ["", "m", "i", "u", "ui", "y", "uy"] {
            let enabled = words(source, flags);
            let mut disabled = enabled.clone();
            disabled[8] = 0;
            for subject in [
                "",
                "a",
                "b",
                "aa",
                "ab",
                "aab",
                "aa!",
                "a\n",
                "a\r\n",
                "a\u{2028}",
                "A",
                "K",
                "ſ",
                "K",
                "😀",
                "a😀",
            ] {
                let units: Vec<_> = subject.encode_utf16().collect();
                for start in [0, 1, units.len(), units.len() + 1] {
                    let expected = run(&disabled, Input::utf16(&units), start, 100_000);
                    assert!(expected.is_ok(), "{source}/{flags}, {subject:?}, {start}");
                    assert_eq!(
                        run(&enabled, Input::utf16(&units), start, 100_000),
                        expected,
                        "original UTF16 {source}/{flags}, {subject:?}, {start}"
                    );
                    assert_eq!(
                        run(&enabled, Input::utf8(subject), start, 100_000),
                        expected,
                        "original UTF8 {source}/{flags}, {subject:?}, {start}"
                    );
                }
            }
            for units in [&[0xd800][..], &[0xdc00], &[0x61, 0xd800], &[0xd800, 0xdc00]] {
                let expected = run(&disabled, Input::utf16(units), 0, 100_000);
                assert_eq!(
                    run(&enabled, Input::utf16(units), 0, 100_000),
                    expected,
                    "surrogates {source}/{flags}, {units:?}"
                );
            }
        }
    }
}

#[test]
fn impossible_anchored_repeat_finishes_with_one_unit_of_work_and_no_stack() {
    // Keep this witness bounded even if the necessary-condition check regresses.
    let enabled = words("^(a+)+$", "");
    let mut disabled = enabled.clone();
    disabled[8] = 0;
    let text = "a".repeat(40) + "!";
    for input in [Input::utf8(&text), Input::utf16(&[0x61, 0x21])] {
        let program = Program::from_words(&enabled, &mut Budget::new(1000)).unwrap();
        let mut registers = vec![0; program.register_count()];
        let mut captures = vec![Span::new(7, 9); program.capture_count()];
        assert_eq!(
            find(
                program,
                input,
                0,
                Scratch {
                    registers: &mut registers,
                    frames: &mut [],
                    undo: &mut []
                },
                &mut captures,
                &mut Budget::new(1)
            ),
            Ok(false)
        );
        assert!(captures.iter().all(|c| *c == Span::new(7, 9)));
    }
    assert_eq!(
        run(&enabled, Input::utf8(&text), 0, 0),
        Err(ExecError::WorkLimit)
    );
    assert_eq!(
        run(&disabled, Input::utf8(&text), 0, 1000),
        Err(ExecError::WorkLimit)
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
