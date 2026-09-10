use perex::{
    Budget,
    compiler::{CompileError, Node, Range, compile},
    executor::{Frame, Scratch, Undo, find},
    input::Input,
    span::Span,
};
fn run(
    source: &str,
    flags: &str,
    subject: &str,
) -> Result<Option<Vec<Option<Span>>>, CompileError> {
    let mut nodes = vec![Node::default(); source.len() * 3 + 32];
    let mut ranges = vec![Range::default(); source.len() * 12 + 32];
    let mut words = vec![0; source.len() * 48 + 128];
    let p = compile(
        Input::utf8(source),
        flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut Budget::new(100_000),
    )?;
    let mut registers = vec![0; p.register_count()];
    let mut frames = [Frame::default(); 128];
    let mut undo = [Undo::default(); 512];
    let mut captures = vec![None; p.capture_count()];
    let matched = find(
        p,
        Input::utf8(subject),
        0,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        &mut captures,
        &mut Budget::new(100_000),
    )
    .unwrap();
    Ok(matched.then_some(captures))
}
#[test]
fn numeric_escape_consumption_preserves_atom_and_quantifier_boundaries() {
    for (source, subject, end) in [
        (r"\1", "\u{1}", 1),
        (r"\08+", "\08", 2),
        (r"\400+", " 000", 4),
        (r"\777+", "?77", 3),
        (r"\1234+", "S444", 4),
        (r"[\400]", "0", 1),
        (r"\8+", "888", 3),
        (
            r"\99999999999999999999999999999",
            "99999999999999999999999999999",
            29,
        ),
    ] {
        assert_eq!(
            run(source, "", subject).unwrap(),
            Some(vec![Span::new(0, end)]),
            "{source}"
        );
    }
    for source in [
        r"\1",
        r"\01",
        r"\08",
        r"[\1]",
        r"[\8]",
        r"\99999999999999999999999999999",
    ] {
        assert!(
            matches!(run(source, "u", ""), Err(CompileError::Syntax { .. })),
            "{source}"
        );
    }
}
#[test]
fn whole_pattern_capture_count_includes_forward_and_named_groups_only() {
    assert_eq!(
        run(r"\2(a)(?<n>b)", "u", "ab").unwrap(),
        Some(vec![Span::new(0, 2), Span::new(0, 1), Span::new(1, 2)])
    );
    assert_eq!(
        run(r"\2(a)[(](?:b)(?=c)", "", "\u{2}a(bc").unwrap(),
        Some(vec![Span::new(0, 4), Span::new(1, 2)])
    );
    assert_eq!(
        run(r"(a)\1", "", "aa").unwrap(),
        Some(vec![Span::new(0, 2), Span::new(0, 1)])
    );
    assert_eq!(
        run(r"\2\k<n>(a)(?<n>b)", "", "ab").unwrap(),
        Some(vec![Span::new(0, 2), Span::new(0, 1), Span::new(1, 2)])
    );
}
#[test]
fn control_prefixes_and_quantified_assertions_keep_legacy_semantics() {
    for (source, subject, end) in [
        (r"\c*", "\\ccc", 4),
        (r"\c", "\\c", 2),
        (r"[\c0]", "\u{10}", 1),
        (r"[\c_]", "\u{1f}", 1),
        (r"[\c]", "\\", 1),
    ] {
        assert_eq!(
            run(source, "", subject).unwrap(),
            Some(vec![Span::new(0, end)]),
            "{source}"
        );
    }
    assert_eq!(
        run(r"(?=(a))*a", "", "a").unwrap(),
        Some(vec![Span::new(0, 1), None])
    );
    assert_eq!(
        run(r"(?=(a))+a", "", "a").unwrap(),
        Some(vec![Span::new(0, 1), Span::new(0, 1)])
    );
    for (source, flags) in [
        (r"\c", "u"),
        (r"[\c0]", "u"),
        (r"(?=a)*", "u"),
        (r"(?<=a)*", ""),
        (r"(?<!a)?", ""),
    ] {
        assert!(
            matches!(run(source, flags, ""), Err(CompileError::Syntax { .. })),
            "{source}"
        );
    }
}
