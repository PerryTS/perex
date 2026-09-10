use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{Frame, Scratch, Undo, find},
    input::{Cursor, Input},
    program::Program,
    span::Span,
};

#[test]
fn backtracking_and_assertion_frames_restore_half_pair_positions_after_relocation() {
    let units = [0xd83d, 0xde00];
    let cesu = [0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80];
    for (source, spans) in [
        (r"\ud83d(?:X|\ude00)", vec![Span::new(0, 2)]),
        (r"(?<=(?:X|\ud83d)\ude00)", vec![Span::new(2, 2)]),
        (
            r"\ud83d(?!X)(\ude00)",
            vec![Span::new(0, 2), Span::new(1, 2)],
        ),
        (
            r"\ud83d(?=(\ude00))\1",
            vec![Span::new(0, 2), Span::new(1, 2)],
        ),
        (
            r"\ud83d((?:X|\ude00)+)",
            vec![Span::new(0, 2), Span::new(1, 2)],
        ),
    ] {
        let mut source_bytes = source.as_bytes().to_vec();
        let mut nodes = [Node::default(); 128];
        let mut ranges = [Range::default(); 128];
        let mut storage = [0; 2048];
        let p = compile(
            Input::wtf8(&source_bytes).unwrap(),
            "",
            &mut nodes,
            &mut ranges,
            &mut storage,
            &mut Budget::new(10000),
        )
        .unwrap();
        let moved = p.words().to_vec();
        storage.fill(0xdeadbeef);
        source_bytes.fill(0);
        drop(source_bytes);
        let p = Program::from_words(&moved, &mut Budget::new(10000)).unwrap();
        for input in [
            Input::utf8("😀"),
            Input::utf16(&units),
            Input::wtf8(&cesu).unwrap(),
        ] {
            let mut registers = [0; 32];
            let mut frames = [Frame::default(); 32];
            let mut undo = [Undo::default(); 128];
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
                    &mut Budget::new(10000)
                )
                .unwrap(),
                "{source}"
            );
            assert_eq!(captures, spans, "{source}");
        }
    }
}

#[test]
#[cfg(target_pointer_width = "64")]
fn input_and_frames_do_not_keep_padding_for_separate_flags() {
    assert_eq!(size_of::<Input<'_>>(), 32);
    assert_eq!(size_of::<Cursor<'_>>(), 48);
    assert_eq!(size_of::<Frame>(), 48);
    assert_eq!(size_of::<Undo>(), 16);
}
