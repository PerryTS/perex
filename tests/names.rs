use perex::{
    Budget,
    compiler::{CompileError, Node, Range, compile},
    executor::{ExecError, Frame, Scratch, Undo, find},
    input::Input,
    program::{Program, ProgramError},
    span::Span,
};

fn program(pattern: &str, flags: &str) -> Vec<u32> {
    let mut nodes = vec![Node::default(); pattern.len() * 3 + 32];
    let mut ranges = vec![Range::default(); pattern.len() * 4 + 32];
    let mut words = vec![0; pattern.len() * 48 + 128];
    let p = compile(
        Input::utf8(pattern),
        flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut Budget::new(100_000),
    )
    .unwrap();
    let moved = p.words().to_vec();
    words.fill(0xdeadbeef);
    moved
}
fn execute(p: Program<'_>, input: Input<'_>) -> Vec<Option<Span>> {
    let mut registers = vec![0; p.register_count()];
    let mut frames = [Frame::default(); 128];
    let mut undo = [Undo::default(); 512];
    let mut captures = vec![None; p.capture_count()];
    assert!(
        find(
            p,
            input,
            0,
            Scratch {
                registers: &mut registers,
                frames: &mut frames,
                undo: &mut undo
            },
            &mut captures,
            &mut Budget::new(100_000)
        )
        .unwrap()
    );
    captures
}

#[test]
fn names_and_results_survive_program_and_subject_relocation() {
    let source = String::from(r"\k<later>(?<first>a)(?<later>b)|(?<first>c)");
    let moved = program(&source, "u");
    drop(source);
    let p = Program::from_words(&moved, &mut Budget::new(100_000)).unwrap();
    let names: Vec<_> = p
        .named_groups()
        .map(|g| {
            (
                String::from_utf16(&g.name_units().collect::<Vec<_>>()).unwrap(),
                g.capture_indices().to_vec(),
            )
        })
        .collect();
    assert_eq!(
        names,
        [("first".into(), vec![1, 3]), ("later".into(), vec![2])]
    );
    assert!(p.named_group(2).is_none());
    let mut old = b"ab".to_vec();
    let subject = old.clone();
    old.fill(0xff);
    let captures = execute(p, Input::wtf8(&subject).unwrap());
    assert_eq!(
        captures,
        [Span::new(0, 2), Span::new(0, 1), Span::new(1, 2), None]
    );
    assert_eq!(
        captures[1]
            .unwrap()
            .units(Input::wtf8(&subject).unwrap())
            .unwrap()
            .collect::<Vec<_>>(),
        [97]
    );
}

#[test]
fn duplicate_names_follow_repeat_resets_and_reverse_execution() {
    for (source, flags, input, expected) in [
        (
            r"(?:(?<x>a)|(?<x>b))+\k<x>",
            "",
            "abb",
            vec![Span::new(0, 3), None, Span::new(1, 2)],
        ),
        (
            r"(?<=\k<x>(?:(?<x>a)|(?<x>b)))c",
            "",
            "bbc",
            vec![Span::new(2, 3), None, Span::new(1, 2)],
        ),
        (
            r"(?<x>\k<x>a)",
            "",
            "a",
            vec![Span::new(0, 1), Span::new(0, 1)],
        ),
    ] {
        let words = program(source, flags);
        let p = Program::from_words(&words, &mut Budget::new(100_000)).unwrap();
        assert_eq!(execute(p, Input::utf8(input)), expected, "{source}");
    }
    let words = program(r"(?<𐐀>.)\k<\uD801\uDC00>", "");
    let p = Program::from_words(&words, &mut Budget::new(100_000)).unwrap();
    let bytes = [0xed, 0xa0, 0xbd, 0xed, 0xa0, 0xbd];
    let units = [0xd83d, 0xd83d];
    let expected = vec![Span::new(0, 2), Span::new(0, 1)];
    assert_eq!(execute(p, Input::wtf8(&bytes).unwrap()), expected);
    assert_eq!(execute(p, Input::utf16(&units)), expected);
}

#[test]
fn unnamed_programs_do_not_pay_for_name_metadata() {
    let plain = program("(a)\\1", "");
    let named = program("(?<x>a)\\k<x>", "");
    // One count, one 3-word descriptor, one packed name word, one capture index.
    assert_eq!(named.len(), plain.len() + 6);
    assert_eq!(&plain[7..], &named[7..plain.len()]);
}

#[test]
fn names_require_bounded_scratch_and_work() {
    let mut nodes = [Node::default(); 64];
    let mut ranges = [Range::default(); 1];
    let mut words = [0; 512];
    assert_eq!(
        compile(
            Input::utf8("(?<long>a)"),
            "",
            &mut nodes,
            &mut ranges,
            &mut words,
            &mut Budget::new(1000)
        )
        .unwrap_err(),
        CompileError::Ranges
    );
    let words = program(r"(?:(?<x>a)|(?<x>b))\k<x>", "");
    assert_eq!(
        Program::from_words(&words, &mut Budget::new(0)).unwrap_err(),
        ProgramError::WorkLimit
    );
    let p = Program::from_words(&words, &mut Budget::new(1000)).unwrap();
    let mut registers = [0; 16];
    let mut frames = [Frame::default(); 32];
    let mut undo = [Undo::default(); 64];
    let mut captures = [Span::new(7, 8); 3];
    assert_eq!(
        find(
            p,
            Input::utf8("bb"),
            0,
            Scratch {
                registers: &mut registers,
                frames: &mut frames,
                undo: &mut undo
            },
            &mut captures,
            &mut Budget::new(1)
        ),
        Err(ExecError::WorkLimit)
    );
    assert_eq!(captures, [Span::new(7, 8); 3]);
}

#[test]
fn malformed_name_tables_are_bounded_and_checked() {
    let original = program(r"(?<𐐀>a)|(?<𐐀>b)|(?<y>c)", "u");
    for at in 0..original.len() {
        for mask in [1, 2, 0x8000_0000, u32::MAX] {
            let mut words = original.clone();
            words[at] ^= mask;
            if let Ok(p) = Program::from_words(&words, &mut Budget::new(10_000)) {
                for group in p.named_groups() {
                    assert!(String::from_utf16(&group.name_units().collect::<Vec<_>>()).is_ok());
                    assert!(
                        group
                            .capture_indices()
                            .iter()
                            .all(|&c| c > 0 && (c as usize) < p.capture_count())
                    );
                }
            }
        }
    }
    for end in 0..original.len() {
        assert!(Program::from_words(&original[..end], &mut Budget::new(10_000)).is_err());
    }
}
