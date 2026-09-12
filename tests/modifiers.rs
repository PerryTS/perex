use perex::{
    Budget,
    compiler::{CompileError, Node, Range, compile},
    executor::{Frame, Scratch, Undo, find},
    input::Input,
    program::{Program, ProgramError},
    span::Span,
};

fn program(source: &str, flags: &str) -> Result<Vec<u32>, CompileError> {
    let mut nodes = vec![Node::default(); source.len() * 3 + 32];
    let mut ranges = vec![Range::default(); source.len() * 12 + 32];
    let mut words = vec![0; source.len() * 48 + 128];
    compile(
        Input::utf8(source),
        flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut Budget::new(1_000_000),
    )
    .map(|p| p.words().to_vec())
}

fn answer(words: &[u32], input: Input<'_>) -> Option<Vec<Option<Span>>> {
    let p = Program::from_words(words, &mut Budget::new(1_000_000)).unwrap();
    let mut output = vec![None; p.capture_count()];
    find(
        p,
        input,
        0,
        Scratch {
            registers: &mut vec![0; p.register_count()],
            frames: &mut vec![Frame::default(); 256],
            undo: &mut vec![Undo::default(); 8192],
        },
        &mut output,
        &mut Budget::new(2_000_000),
    )
    .unwrap()
    .then_some(output)
}

#[test]
fn lexical_flags_preserve_captures_backreferences_and_outer_behavior() {
    type Case<'a> = (&'a str, &'a str, &'a str, &'a [(usize, usize)]);
    let cases: &[Case<'_>] = &[
        (r"(a)(?i:\1)", "", "aA", &[(0, 2), (0, 1)]),
        (r"(?i:(a))\1", "", "AA", &[(0, 2), (0, 1)]),
        (r"(?i:(?<x>a))(?-i:\k<x>)", "i", "AA", &[(0, 2), (0, 1)]),
        (r"(?i:\bſ\b)", "u", "ſ", &[(0, 1)]),
        (r"(?-i:\W)", "iu", "ſ", &[(0, 1)]),
        (r"(?s:(.))(?-s:.)", "s", "\nA", &[(0, 2), (0, 1)]),
        (r"(?m:^(a)$)", "", "x\na\ny", &[(2, 3), (2, 3)]),
        (r"(?i:a(?-i:b)c)d", "", "AbCd", &[(0, 4)]),
        (r"(?i-m:foo)|bar", "m", "FOO", &[(0, 3)]),
        (r"(?<=^(?i:(a+)))(?-i:b)", "", "AAb", &[(2, 3), (0, 2)]),
        (r"(?<=^(?i:(a+?)))(?-i:b)", "", "AAb", &[(2, 3), (0, 2)]),
    ];
    for &(source, flags, subject, spans) in cases {
        let words = program(source, flags).unwrap();
        let units: Vec<_> = subject.encode_utf16().collect();
        for input in [Input::utf8(subject), Input::utf16(&units)] {
            let expected: Vec<_> = spans.iter().map(|&(lo, hi)| Span::new(lo, hi)).collect();
            assert_eq!(answer(&words, input), Some(expected), "{source}/{flags}");
        }
    }
    assert_eq!(
        answer(
            &program(r"(?i:(?<x>a)|(?<x>b))(?i:\k<x>)", "").unwrap(),
            Input::utf8("aA")
        ),
        Some(vec![Span::new(0, 2), Span::new(0, 1), None])
    );
    for (source, flags, subject) in [
        (r"(?i:(a))\1", "", "Aa"),
        (r"(?i:(?<x>a))(?-i:\k<x>)", "i", "Aa"),
        (r"(?-i:\bſ\b)", "iu", "ſ"),
        (r"(?i:\W)", "u", "ſ"),
        (r"(?s:(.))(?-s:.)", "s", "\n\n"),
        (r"(?-m:^a$)", "m", "x\na\ny"),
        (r"(?i:a)b|aB", "", "AB"),
        (r"(?i:a(?-i:b)c)d", "", "ABCd"),
        (r"(?i:a(?-i:b)c)d", "", "AbCD"),
    ] {
        assert_eq!(
            answer(&program(source, flags).unwrap(), Input::utf8(subject)),
            None,
            "{source}/{flags}"
        );
    }
}

#[test]
fn equivalent_global_and_scoped_flags_have_identical_code_and_storage() {
    for (body, flags) in [
        ("a+", "i"),
        ("[a-z]+", "i"),
        ("^.+$", "ms"),
        (r"(a)\1", "i"),
        (r"\b\w+\b", "i"),
        (r"(?<x>a)\k<x>", "i"),
    ] {
        let mut global = program(body, flags).unwrap();
        let scoped = program(&format!("(?{flags}:{body})"), "").unwrap();
        // Header retains the caller's flags for binding identity. The matcher
        // obtains lexical semantics entirely from equal instruction choices.
        global[2] &= !(2 | 4 | 16);
        assert_eq!(global, scoped, "{body}/{flags}");
    }
    for source in ["(?i:a)", "(?i-:a)", "(?-i:a)", "(?ms-i:a)"] {
        assert_eq!(
            program(source, "").unwrap().len(),
            program("a", "").unwrap().len()
        );
    }
}

#[test]
fn modifier_errors_and_instruction_corruption_are_explicit() {
    for source in [
        "(?-:a)",
        "(?ii:a)",
        "(?i-i:a)",
        "(?--i:a)",
        "(?i-m-s:a)",
        "(?g:a)",
        "(?u:a)",
        "(?i)",
        "(?i:a",
        r"(?\u0069:a)",
    ] {
        assert!(
            matches!(program(source, ""), Err(CompileError::Syntax { .. })),
            "{source}"
        );
    }
    for (source, flags) in [
        ("a", "i"),
        ("[a]", "i"),
        (".", "s"),
        ("^", "m"),
        ("$", "m"),
        (r"\b", "i"),
        (r"(a)\1", "i"),
        (r"(?<x>a)|(?<x>b)\k<x>", "i"),
    ] {
        let words = program(source, flags).unwrap();
        assert_eq!(words[1], 12);
        for pc in 0..words[4] as usize {
            let at = 10 + pc * 3;
            if (19..=26).contains(&words[at]) {
                let mut invalid = words.clone();
                invalid[at + 2] = u32::MAX;
                assert_eq!(
                    Program::from_words(&invalid, &mut Budget::new(1_000_000)).unwrap_err(),
                    ProgramError::Invalid
                );
            }
        }
    }
}
