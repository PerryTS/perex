//! Complete public answers across empty/nonempty record sections and a named
//! capture tail. Relocation and every small work quantum are also exercised
//! by the existing resumption suite, which poisons the old program allocation.
use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{Frame, Scratch, Undo, find},
    input::Input,
    span::Span,
};

#[test]
fn record_section_boundaries_preserve_complete_captures_in_both_storages() {
    type Captures<'a> = &'a [Option<(usize, usize)>];
    type Case<'a> = (&'a str, &'a str, &'a str, Captures<'a>);
    let cases: &[Case<'_>] = &[
        ("a", "", "a", &[Some((0, 1))]),
        ("[a-c]", "", "b", &[Some((0, 1))]),
        ("a{2}", "", "aa", &[Some((0, 2))]),
        (
            "(?<word>[a-c]+)(?:-|_)([0-9]{2,4})(z)?",
            "",
            "abc-123",
            &[Some((0, 7)), Some((0, 3)), Some((4, 7)), None],
        ),
        (
            "(?<word>é+)([0-9]{2})",
            "u",
            "éé12",
            &[Some((0, 4)), Some((0, 2)), Some((2, 4))],
        ),
        (
            "(?<word>😀+)([0-9]{2})",
            "u",
            "😀😀12",
            &[Some((0, 6)), Some((0, 4)), Some((4, 6))],
        ),
    ];
    for &(source, flags, text, expected) in cases {
        let mut nodes = vec![Node::default(); 1024];
        let mut ranges = vec![Range::default(); 2048];
        let mut words = vec![0; 16384];
        let program = compile(
            Input::utf8(source),
            flags,
            &mut nodes,
            &mut ranges,
            &mut words,
            &mut Budget::new(1_000_000),
        )
        .unwrap();
        let units: Vec<_> = text.encode_utf16().collect();
        for input in [Input::utf8(text), Input::utf16(&units)] {
            let mut registers = vec![0; program.register_count()];
            let mut frames = vec![Frame::default(); 256];
            let mut undo = vec![Undo::default(); 2048];
            let mut captures = vec![Span::new(900, 901); program.capture_count()];
            assert!(
                find(
                    program,
                    input,
                    0,
                    Scratch {
                        registers: &mut registers,
                        frames: &mut frames,
                        undo: &mut undo
                    },
                    &mut captures,
                    &mut Budget::new(1_000_000),
                )
                .unwrap(),
                "/{source}/{flags}: {text:?}"
            );
            let actual: Vec<_> = captures
                .iter()
                .map(|s| s.map(|s| (s.start(), s.end())))
                .collect();
            assert_eq!(actual, expected, "/{source}/{flags}: {text:?}");
        }
    }
}
