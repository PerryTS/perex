//! Complete answers against the evaluator, original storage and relocation.
use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{ExecError, Frame, Scratch, Undo, find},
    input::Input,
    program::Program,
    span::Span,
};

fn compile_words(pattern: &str, flags: &str) -> Vec<u32> {
    let mut nodes = vec![Node::default(); 1024];
    let mut ranges = vec![Range::default(); 1024];
    let mut words = vec![0; 8192];
    compile(
        Input::utf8(pattern),
        flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut Budget::new(1_000_000),
    )
    .unwrap()
    .words()
    .to_vec()
}
fn view(words: &[u32]) -> Program<'_> {
    Program::from_words(words, &mut Budget::new(1_000_000)).unwrap()
}
fn evaluator(program: Program<'_>, input: Input<'_>, start: usize) -> Option<Span> {
    let mut registers = vec![0; program.register_count()];
    let mut frames = [Frame::default(); 64];
    let mut undo = [Undo::default(); 128];
    let mut captures = vec![None; program.capture_count()];
    let matched = find(
        program,
        input,
        start,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        &mut captures,
        &mut Budget::new(1_000_000),
    )
    .unwrap();
    assert_eq!(captures.len(), 1, "the prefix answer is every capture");
    matched.then(|| captures[0].unwrap())
}
fn agree(program: Program<'_>, input: Input<'_>) {
    for start in 0..=input.len_utf16() + 1 {
        let expected = evaluator(program, input, start);
        let prefix = program
            .literal_prefix()
            .expect("fixture must use the exact prefix executor");
        assert_eq!(
            prefix.find(input, start, &mut Budget::new(1000)).unwrap(),
            expected,
            "start {start}"
        );
        let mut cache = [];
        assert_eq!(perex::dfa::minimum_words(program), Some(0));
        assert_eq!(
            perex::dfa::find(program, input, start, &mut cache, &mut Budget::new(1000)).unwrap(),
            expected.map_or(perex::dfa::Found::NoMatch, perex::dfa::Found::Match)
        );
    }
}
#[test]
fn complete_answers_all_flags_and_original_storage() {
    for pattern in [
        "^--",
        "^-",
        "^no-",
        r"^\u002d\x2d",
        "^é",
        "^😀",
        r"^\ud800",
        "^abcdefghijklmnopqrstuvwxyz012345",
    ] {
        for flags in ["", "g", "y", "gy", "u", "gu", "yu", "v", "dgs"] {
            let words = compile_words(pattern, flags);
            let program = view(&words);
            for subject in [
                "",
                "-",
                "--",
                "--option",
                "no-option",
                "x--",
                "\n--",
                "éclair",
                "😀tail",
                "a😀",
            ] {
                agree(program, Input::utf8(subject));
                let units: Vec<_> = subject.encode_utf16().collect();
                agree(program, Input::utf16(&units));
            }
            agree(program, Input::wtf8(&[0xed, 0xa0, 0x80, b'x']).unwrap());
            // Separately encoded surrogate halves are valid generalized WTF-8.
            agree(
                program,
                Input::wtf8(&[0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80, b'x']).unwrap(),
            );
        }
    }
}
#[test]
fn other_programs_decline_and_descriptor_cannot_be_forged() {
    for (pattern, flags) in [
        ("^--", "m"),
        ("^--", "i"),
        ("^(--)", ""),
        ("^--$", ""),
        ("--", ""),
        ("^--|x", ""),
        ("^--?", ""),
        ("^", ""),
        ("^abcdefghijklmnopqrstuvwxyz0123456", ""),
    ] {
        let mut words = compile_words(pattern, flags);
        assert!(
            view(&words).literal_prefix().is_none(),
            "/{pattern}/{flags}"
        );
        words[7] |= 1 << 26;
        assert!(Program::from_words(&words, &mut Budget::new(1_000_000)).is_err());
    }
    let mut words = compile_words("^--", "");
    words[7] &= !(1 << 26);
    assert!(Program::from_words(&words, &mut Budget::new(1_000_000)).is_err());
}
#[test]
fn relocation_and_work_exhaustion() {
    let mut words = compile_words("^--", "");
    let moved = words.clone();
    words.fill(0xdead_beef);
    drop(words);
    let prefix = view(&moved).literal_prefix().unwrap();
    assert_eq!(
        prefix.find(Input::utf8("--abc"), 0, &mut Budget::new(3)),
        Ok(Span::new(0, 2))
    );
    assert_eq!(
        prefix.find(Input::utf8("--abc"), 0, &mut Budget::new(2)),
        Err(ExecError::WorkLimit)
    );
    assert_eq!(
        prefix.find(Input::utf8("xx"), 0, &mut Budget::new(0)),
        Err(ExecError::WorkLimit)
    );
}
