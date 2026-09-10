use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{Frame, Scratch, Undo, find},
    input::Input,
    program::Program,
    span::Span,
};

fn words(source: &str, flags: &str) -> Vec<u32> {
    let mut nodes = vec![Node::default(); source.len() * 3 + 32];
    let mut ranges = vec![Range::default(); source.len() * 12 + 32];
    let mut storage = vec![0; source.len() * 48 + 128];
    let p = compile(
        Input::utf8(source),
        flags,
        &mut nodes,
        &mut ranges,
        &mut storage,
        &mut Budget::new(1_000_000),
    )
    .unwrap();
    let moved = p.words().to_vec();
    storage.fill(0xdeadbeef);
    moved
}

fn run(words: &[u32], input: Input<'_>, work: usize) -> Vec<Option<Span>> {
    let p = Program::from_words(words, &mut Budget::new(10000)).unwrap();
    let mut registers = vec![0; p.register_count()];
    let mut frames = vec![Frame::default(); 1024];
    let mut undo = vec![Undo::default(); 4096];
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
            &mut Budget::new(work)
        )
        .unwrap()
    );
    captures
}

#[test]
fn nested_single_atom_choices_use_one_repeat_and_find_later_matches() {
    for source in [
        r"(?:(?:(?:\w|(?:))){1,3})+Q",
        r"(?:(?:(?:\w){1,3}){0,2})+Q",
        r"(?:(?:(?:\w){0,2}){1,3})+Q",
        r"(?:(?:(?:\w){0,2}){0,2})+Q",
    ] {
        let nested = words(source, "");
        let simple = words(r"\w*Q", "");
        assert_eq!(nested.len(), simple.len(), "{source}");
        let p = Program::from_words(&nested, &mut Budget::new(10000)).unwrap();
        assert_eq!(p.register_count(), 4);
        let subject = format!("{}ababaQ", "0123456789 ".repeat(8));
        assert_eq!(
            run(&nested, Input::utf8(&subject), 20000),
            vec![Span::new(88, 94)]
        );
    }
}

#[test]
fn gaps_priority_captures_and_lazy_repeats_are_not_flattened() {
    // Inner minima >1 can change first-visit order even when lengths overlap.
    let priority = words(r"^((?:a{3,5}){0,2})(a*?)$", "");
    assert_eq!(
        run(&priority, Input::utf8("aaaaaaa"), 10000),
        vec![Span::new(0, 7), Span::new(0, 5), Span::new(5, 7)]
    );
    let captures = words(r"^((a{1,3})+)(a*?)$", "");
    assert_eq!(
        run(&captures, Input::utf8("aaaaaaa"), 10000),
        vec![
            Span::new(0, 7),
            Span::new(0, 7),
            Span::new(6, 7),
            Span::new(7, 7)
        ]
    );
    let lazy = words(r"^((?:a{1,3}?)+?)(a*)$", "");
    assert_eq!(
        run(&lazy, Input::utf8("aaaaaaa"), 10000),
        vec![Span::new(0, 7), Span::new(0, 1), Span::new(1, 7)]
    );
    for source in [
        r"(?:a{2})+",
        r"(?:a{1,3}?)+",
        r"(?:a{1,3})+?",
        r"(?:(a){1,3})+",
        r"(?:a{1,4294967295}){2}",
        r"(?:(?=a)a?)+",
        r"(?:ab?)+",
    ] {
        let w = words(source, "");
        let p = Program::from_words(&w, &mut Budget::new(10000)).unwrap();
        assert!(p.register_count() >= 6, "{source}");
    }
}

#[test]
fn finite_zero_and_infinite_bounds_preserve_original_surrogate_positions() {
    for (source, flags, spans) in [
        (r"(?:(?:.){0,2}){2,3}", "", vec![Span::new(0, 2)]),
        (r"(?:(?:.){0,2}){2,3}", "u", vec![Span::new(0, 2)]),
        (r"(?:\ud83d?)+", "", vec![Span::new(0, 1)]),
        (r"(?:(?:.){0})+", "u", vec![Span::new(0, 0)]),
        (r"(?:(?:.)+){0}", "u", vec![Span::new(0, 0)]),
    ] {
        let w = words(source, flags);
        let units = [0xd83d, 0xde00];
        let cesu = [0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80];
        assert_eq!(run(&w, Input::utf8("😀"), 10000), spans, "{source}");
        assert_eq!(run(&w, Input::utf16(&units), 10000), spans, "{source}");
        assert_eq!(
            run(&w, Input::wtf8(&cesu).unwrap(), 10000),
            spans,
            "{source}"
        );
    }
    let reverse = words(r"(?<=^((?:a{0,2}){1,3})(a*?))z", "");
    assert_eq!(
        run(&reverse, Input::utf8("aaaaaz"), 10000),
        vec![Span::new(5, 6), Span::new(0, 5), Span::new(5, 5)]
    );
}
