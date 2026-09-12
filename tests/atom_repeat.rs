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
        &mut Budget::new(1_000_000),
    )
    .unwrap()
    .words()
    .to_vec()
}

// A development witness using the existing general instructions of this same
// VM. Production compilation/matching has no alternate-engine route.
fn general(mut words: Vec<u32>) -> Vec<u32> {
    let table = 10 + words[4] as usize * 3 + words[5] as usize * 2;
    for i in 0..words[6] as usize {
        let at = table + i * 8;
        if words[at + 7] == 1 {
            let entry = words[at + 3] as usize - 2;
            assert_eq!(words[10 + entry * 3], 18);
            words[10 + entry * 3] = 13;
            words[at + 7] = 0;
        }
    }
    Program::from_words(&words, &mut Budget::new(1_000_000)).unwrap();
    words
}

fn execute(
    words: &[u32],
    input: Input<'_>,
    frame_capacity: usize,
    undo_capacity: usize,
    allowance: usize,
) -> (Result<bool, ExecError>, Vec<Option<Span>>, usize) {
    let p = Program::from_words(words, &mut Budget::new(1_000_000)).unwrap();
    let mut registers = vec![0; p.register_count()];
    let mut frames = vec![Frame::default(); frame_capacity];
    let mut undo = vec![Undo::default(); undo_capacity];
    let mut captures = vec![Span::new(900, 901); p.capture_count()];
    let mut budget = Budget::new(allowance);
    let result = find(
        p,
        input,
        0,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        &mut captures,
        &mut budget,
    );
    (result, captures, allowance - budget.remaining())
}

#[test]
fn long_consuming_repeats_need_one_retry_frame_and_two_capture_undos() {
    for flags in ["", "u", "i", "ui"] {
        let w = words("^([a-z]+)$", flags);
        for length in [1, 2, 64, 4096, 65_536] {
            let subject = "a".repeat(length);
            let units: Vec<_> = subject.encode_utf16().collect();
            for input in [Input::utf8(&subject), Input::utf16(&units)] {
                let (result, captures, _) = execute(&w, input, 1, 2, 2_000_000);
                assert_eq!(result, Ok(true), "{flags} {length}");
                assert_eq!(captures, vec![Span::new(0, length); 2]);
            }
        }
        assert_eq!(
            execute(&general(w), Input::utf8(&"a".repeat(64)), 1, 2, 2_000_000).0,
            Err(ExecError::Frames)
        );
    }
}

#[test]
fn retries_preserve_complete_captures_order_and_general_instruction_answers() {
    type CaptureCase<'a> = (&'a str, &'a str, &'a str, &'a [(usize, usize)]);
    let cases: &[CaptureCase<'_>] = &[
        (
            r"^([a-z]+)([a-z]{2})$",
            "",
            "abcdef",
            &[(0, 6), (0, 4), (4, 6)],
        ),
        (
            r"^([a-z]+?)([a-z]{2})$",
            "",
            "abcdef",
            &[(0, 6), (0, 4), (4, 6)],
        ),
        (
            r"^([a-z]+)([a-z]*)([a-z]{2})$",
            "",
            "abcdef",
            &[(0, 6), (0, 4), (4, 4), (4, 6)],
        ),
        (
            r"(?<=^([a-z]{2})([a-z]+))$",
            "",
            "abcdef",
            &[(6, 6), (0, 2), (2, 6)],
        ),
        (
            r"(?<=^([a-z]{2})([a-z]+?))$",
            "",
            "abcdef",
            &[(6, 6), (0, 2), (2, 6)],
        ),
        (r"^(.+)(.)\1$", "", "aba", &[(0, 3), (0, 1), (1, 2)]),
        (r"^(.+)(.)\1$", "u", "😀x😀", &[(0, 5), (0, 2), (2, 3)]),
        (r"(?<=(.)(.+))x", "", "abcx", &[(3, 4), (0, 1), (1, 3)]),
        (
            r"^([a-z]{2,4}?)([a-z]{3})$",
            "",
            "abcdef",
            &[(0, 6), (0, 3), (3, 6)],
        ),
        (r"^(a{0})(a*)$", "", "aaa", &[(0, 3), (0, 0), (0, 3)]),
        (r"^(?:a{0})*$", "", "", &[(0, 0)]),
        (r"^(a+)+b$", "", "aaaaab", &[(0, 6), (0, 5)]),
        (r"^(\ud83d*)(\ude00)$", "", "😀", &[(0, 2), (0, 1), (1, 2)]),
        (
            r"(?<=^(\ud83d)(\ude00*))$",
            "",
            "😀",
            &[(2, 2), (0, 1), (1, 2)],
        ),
        (r"^(.{1,3})(.)$", "u", "😀a😀", &[(0, 5), (0, 3), (3, 5)]),
        (
            r"(?<=^(.)(.{1,3}))$",
            "u",
            "😀a😀",
            &[(5, 5), (0, 2), (2, 5)],
        ),
    ];
    for &(source, flags, subject, spans) in cases {
        let optimized = words(source, flags);
        let original = general(optimized.clone());
        let units: Vec<_> = subject.encode_utf16().collect();
        let expected: Vec<_> = spans.iter().map(|&(a, b)| Span::new(a, b)).collect();
        for input in [Input::utf8(subject), Input::utf16(&units)] {
            let a = execute(&optimized, input, 256, 8192, 2_000_000);
            let b = execute(&original, input, 256, 8192, 2_000_000);
            assert_eq!(a.0, Ok(true), "{source} {flags}");
            assert_eq!(a.1, expected, "{source} {flags}");
            assert_eq!((a.0, &a.1), (b.0, &b.1), "{source} {flags}");
        }
    }
    for source in [
        r"^([a-z]{2,4}?)([a-z]{3})$",
        r"^([a-z]{2,4})([a-z]{3})$",
        r"^([a-z]{9,})$",
        r"^(a+)+b$",
    ] {
        let optimized = words(source, "");
        let original = general(optimized.clone());
        let a = execute(&optimized, Input::utf8("aaaaaaaa"), 256, 8192, 2_000_000);
        let b = execute(&original, Input::utf8("aaaaaaaa"), 256, 8192, 2_000_000);
        assert_eq!(a.0, Ok(false), "{source}");
        assert_eq!((a.0, a.1), (b.0, b.1), "{source}");
    }
}

#[test]
fn every_insufficient_work_allowance_preserves_output() {
    for source in [
        r"^([a-z]+)([a-z]{2})$",
        r"^([a-z]+?)([a-z]{2})$",
        r"(?<=^([a-z]{2})([a-z]+))$",
        r"(?!(a+)+x)(a+)y$",
    ] {
        let w = words(source, "");
        let subject = if source.contains('y') {
            "aaaay"
        } else {
            "abcdef"
        };
        let (ok, expected, work) = execute(&w, Input::utf8(subject), 256, 8192, 2_000_000);
        assert_eq!(ok, Ok(true), "{source}");
        for allowance in 0..work {
            let (error, output, _) = execute(&w, Input::utf8(subject), 256, 8192, allowance);
            assert_eq!(
                error,
                Err(ExecError::WorkLimit),
                "{source} allowance {allowance}"
            );
            assert_eq!(output, vec![Span::new(900, 901); expected.len()]);
        }
        let (ok, actual, used) = execute(&w, Input::utf8(subject), 256, 8192, work);
        assert_eq!(ok, Ok(true));
        assert_eq!(actual, expected);
        assert_eq!(used, work);
    }
}

#[test]
fn optimized_record_validation_rejects_shape_and_version_corruption() {
    let w = words("^([a-z]+)$", "");
    assert_eq!(w[1], 11);
    let table = 10 + w[4] as usize * 3 + w[5] as usize * 2;
    assert_eq!(w[table + 7], 1);
    let entry = w[table + 3] as usize - 2;
    for (index, value) in [
        (1, 6),
        (table + 7, 0),
        (table + 7, 2),
        (table + 3, 1),
        (table + 3, u32::MAX),
        (table + 4, w[table + 4] - 1),
        (table + 5, 0),
        (10 + entry * 3, 13),
        (10 + (entry + 1) * 3, 0),
        (10 + (entry + 3) * 3, 4),
        (10 + (entry + 4) * 3, 0),
    ] {
        let mut bad = w.clone();
        bad[index] = value;
        assert_eq!(
            Program::from_words(&bad, &mut Budget::new(1_000_000)).unwrap_err(),
            ProgramError::Invalid,
            "word {index}"
        );
    }
    for source in ["(?:ab)+", "(a)+", "(?:a|b)+", "(?=a)+", "(a)\\1+"] {
        let w = words(source, "");
        let table = 10 + w[4] as usize * 3 + w[5] as usize * 2;
        assert!(
            (0..w[6] as usize).all(|i| w[table + i * 8 + 7] == 0),
            "{source}"
        );
    }
}

#[test]
fn literal_retry_filter_keeps_general_vm_captures_and_errors() {
    for source in [
        r"^(.*ab)",
        r"^(.{2,9}ab)",
        r"^(.{2,9}?ab)",
        r"^(.*ab|.*ac)",
        r"^(?:.*ab)(c)?",
        r"(?<=^(ab.*))$",
        r"(?<=^(ab.{2,9}))$",
        r"^(.*ſK)",
        r"(?<=^(ſK.*))$",
        r"^(.*\ud83d)",
        r"(?<=^(\ude00.*))$",
        r"(?!(.*ab))x",
        r"^(.*ab)\1",
    ] {
        for flags in ["", "u", "i", "ui"] {
            let optimized = words(source, flags);
            let original = general(optimized.clone());
            for subject in [
                "",
                "ab",
                "abxxxxxxxx",
                "ababxxabc",
                "ſKxxxxxxxx",
                "sKxxxxſK",
                "😀xxxx😀",
            ] {
                let units: Vec<_> = subject.encode_utf16().collect();
                for input in [Input::utf8(subject), Input::utf16(&units)] {
                    let a = execute(&optimized, input, 1024, 8192, 2_000_000);
                    let b = execute(&original, input, 1024, 8192, 2_000_000);
                    assert_eq!((&a.0, &a.1), (&b.0, &b.1), "{source} {flags} {subject}");
                    for allowance in 0..a.2 {
                        let (error, captures, _) =
                            execute(&optimized, input, 1024, 8192, allowance);
                        assert_eq!(
                            error,
                            Err(ExecError::WorkLimit),
                            "{source} {flags} {subject} {allowance}"
                        );
                        assert!(captures.iter().all(|s| *s == Span::new(900, 901)));
                    }
                }
            }
        }
    }
}
