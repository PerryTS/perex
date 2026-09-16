use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{Frame, Scratch, Undo, find},
    input::Input,
    program::{Program, ProgramError},
    span::Span,
};

fn program(source: &str, flags: &str) -> Vec<u32> {
    let mut nodes = vec![Node::default(); 4096];
    let mut ranges = vec![Range::default(); 4096];
    let mut words = vec![0; 32768];
    compile(
        Input::utf8(source),
        flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut Budget::new(2_000_000),
    )
    .unwrap()
    .words()
    .to_vec()
}

fn answer(words: &[u32], units: &[u16]) -> Option<Vec<Option<Span>>> {
    let p = Program::from_words(words, &mut Budget::new(1_000_000)).unwrap();
    let mut registers = vec![0; p.register_count()];
    let mut frames = vec![Frame::default(); 256];
    let mut undo = vec![Undo::default(); 2048];
    let mut captures = vec![None; p.capture_count()];
    find(
        p,
        Input::utf16(units),
        0,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        &mut captures,
        &mut Budget::new(100_000),
    )
    .unwrap()
    .then_some(captures)
}

fn members() -> String {
    (0..128)
        .rev()
        .map(|i| format!("\\u{:04x}", 0x100 + i * 3))
        .collect()
}

#[test]
fn sorted_classes_preserve_original_membership_and_reject_invalid_order() {
    let source = format!("[{}]", members());
    let words = program(&source, "u");
    assert_eq!(words[1], 13);
    assert_eq!(words[11 + 3], 27); // Sorted class following capture-zero start.
    for unit in 0..0x400u16 {
        let expected = (0..128).any(|i| unit == 0x100 + i * 3);
        assert_eq!(answer(&words, &[unit]).is_some(), expected, "{unit:x}");
    }
    let start = 11 + words[4] as usize * 3;
    let mut changed = words.clone();
    changed.swap(start, start + 2);
    assert_eq!(
        Program::from_words(&changed, &mut Budget::new(10000)).unwrap_err(),
        ProgramError::Invalid
    );
    let mut changed = words.clone();
    changed[start + 2] = changed[start + 1]; // Overlapping intervals cannot use binary search.
    assert_eq!(
        Program::from_words(&changed, &mut Budget::new(10000)).unwrap_err(),
        ProgramError::Invalid
    );
}

#[test]
fn class_normalization_merges_duplicates_and_overlaps_without_more_storage() {
    let source = format!("[{}]", "a-z".repeat(32));
    let compact = program(&source, "");
    let ordinary = program("[a-z]", "");
    assert_eq!(compact, ordinary);
    let source = format!("[{}\\p{{L}}]", members());
    let mixed = program(&source, "u");
    assert_eq!(mixed[11 + 3], 3); // Property union uses its existing evaluator.
}

#[test]
fn binary_and_linear_class_membership_preserve_full_capture_results() {
    for class in [format!("[{}]", members()), format!("[^{}]", members())] {
        for source in [
            class.clone(),
            format!("({class}+)({class}?)"),
            format!("({class}+?)({class}*)"),
            format!("(?<=({class}+))({class})"),
            format!("({class})\\1"),
            format!("(?=({class}))\\1"),
        ] {
            for flags in ["", "u", "i", "iu"] {
                let words = program(&source, flags);
                let mut linear = words.clone();
                for pc in 0..words[4] as usize {
                    let at = 11 + pc * 3;
                    linear[at] = match linear[at] {
                        27 => 3,
                        28 => 20,
                        other => other,
                    };
                }
                for units in [
                    &[][..],
                    &[0x100],
                    &[0x100, 0x100],
                    &[0x101, 0x100, 0x103, 0x100],
                    &[0x17f, 0x53, 0x73, 0x212a, 0x4b, 0x6b],
                    &[0xd800, 0xdc00, 0xdfff],
                ] {
                    assert_eq!(
                        answer(&words, units),
                        answer(&linear, units),
                        "{source} {flags} {units:?}"
                    );
                }
            }
        }
    }
}
