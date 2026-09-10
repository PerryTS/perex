use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{ExecError, Frame, Scratch, Undo, find},
    input::Input,
    program::{Program, ProgramError},
    span::Span,
};
fn compile_words(source: &str, flags: &str) -> Vec<u32> {
    let mut nodes = vec![Node::default(); source.len() * 3 + 32];
    let mut ranges = vec![Range::default(); source.len() * 12 + 32];
    let mut words = vec![0; source.len() * 48 + 128];
    let p = compile(
        Input::utf8(source),
        flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut Budget::new(1_000_000),
    )
    .unwrap();
    let moved = p.words().to_vec();
    words.fill(0xdeadbeef);
    moved
}
fn execute(
    words: &[u32],
    input: Input<'_>,
    start: usize,
    work: usize,
) -> Result<Option<Vec<Option<Span>>>, ExecError> {
    let p = Program::from_words(words, &mut Budget::new(1_000_000)).unwrap();
    let mut regs = vec![0; p.register_count()];
    let mut frames = vec![Frame::default(); 16384];
    let mut undo = vec![Undo::default(); 131072];
    let mut caps = vec![Span::new(7, 9); p.capture_count()];
    let outcome = find(
        p,
        input,
        start,
        Scratch {
            registers: &mut regs,
            frames: &mut frames,
            undo: &mut undo,
        },
        &mut caps,
        &mut Budget::new(work),
    );
    if !matches!(outcome, Ok(true)) {
        assert!(caps.iter().all(|s| *s == Span::new(7, 9)));
    }
    outcome.map(|found| found.then_some(caps))
}
#[test]
fn required_search_conditions_reject_long_misses_without_more_program_storage() {
    let subject = "az ".repeat(1100);
    for pattern in [
        r"a*missing",
        r"(?<!#.*)root\s*=",
        r"[^/]+:token",
        r".*->",
        r"\s+(?=[^()]*\))",
        r"([A-F].*[a-f])|([a-f].*[A-F])",
    ] {
        let enabled = compile_words(pattern, "");
        assert_ne!(enabled[2] & 128, 0, "{pattern}");
        let mut disabled = enabled.clone();
        disabled[2] &= 63;
        assert_eq!(enabled.len(), disabled.len());
        assert_eq!(
            execute(&enabled, Input::utf8(&subject), 0, 10000).unwrap(),
            None,
            "{pattern}"
        );
        assert_eq!(
            execute(&disabled, Input::utf8(&subject), 0, 10000),
            Err(ExecError::WorkLimit),
            "{pattern}"
        );
    }
}
#[test]
fn equally_ranked_literals_prefer_the_required_suffix() {
    let subject = "a".repeat(3300);
    let enabled = compile_words("a+z", "");
    let mut disabled = enabled.clone();
    disabled[2] &= 63;
    assert_eq!(
        execute(&enabled, Input::utf8(&subject), 0, 10000).unwrap(),
        None
    );
    assert_eq!(
        execute(&disabled, Input::utf8(&subject), 0, 10000),
        Err(ExecError::WorkLimit)
    );
}
#[test]
fn alternatives_optional_repeats_and_negative_assertions_cannot_invent_requirements() {
    let subject = "b".repeat(128);
    for pattern in [
        r"a*",
        r"a*b|b+",
        r"(?!a+)b+",
        r"(?:(?<x>a)|(?<x>b))+",
        r"(?:a+|b+)",
        r"(?:a+)?b+",
    ] {
        let enabled = compile_words(pattern, "");
        let mut disabled = enabled.clone();
        disabled[2] &= 63;
        let actual = execute(&enabled, Input::utf8(&subject), 0, 1_000_000).unwrap();
        assert!(actual.is_some(), "{pattern}");
        assert_eq!(
            actual,
            execute(&disabled, Input::utf8(&subject), 0, 1_000_000).unwrap(),
            "{pattern}"
        );
    }
}
#[test]
fn lookbehind_checks_the_original_prefix_and_retains_reverse_order() {
    let subject = format!("needle{}z", "a".repeat(100));
    for pattern in [r"(?<=needle.*)z", r"(?<=\w*needle)a+"] {
        let words = compile_words(pattern, "g");
        assert_ne!(words[2] & 128, 0);
        assert!(
            execute(&words, Input::utf8(&subject), 6, 100000)
                .unwrap()
                .is_some(),
            "{pattern}"
        );
    }
}
#[test]
fn admission_uses_lossless_original_units_and_the_same_case_equivalence() {
    for (pattern, flags, tail) in [
        (r".*ſK", "ui", "sK"),
        (r".*ſK", "i", "ſK"),
        (r".*😀", "u", "😀"),
        (r".*\ud83d", "", "😀"),
        (r".*\ude00", "", "😀"),
    ] {
        let subject = format!("{}{}", "a".repeat(80), tail);
        let words = compile_words(pattern, flags);
        let units: Vec<_> = subject.encode_utf16().collect();
        let mut cesu = Vec::new();
        for &u in &units {
            let p = u as u32;
            if p < 128 {
                cesu.push(p as u8)
            } else if p < 2048 {
                cesu.extend([(0xc0 | p >> 6) as u8, (0x80 | p & 63) as u8])
            } else {
                cesu.extend([
                    (0xe0 | p >> 12) as u8,
                    (0x80 | p >> 6 & 63) as u8,
                    (0x80 | p & 63) as u8,
                ])
            }
        }
        let reference = execute(&words, Input::utf16(&units), 0, 100000).unwrap();
        assert!(reference.is_some(), "{pattern}");
        assert_eq!(
            execute(&words, Input::utf8(&subject), 0, 100000).unwrap(),
            reference
        );
        assert_eq!(
            execute(&words, Input::wtf8(&cesu).unwrap(), 0, 100000).unwrap(),
            reference
        );
    }
}
#[test]
fn generic_suffix_probe_preserves_earlier_matches_and_reverse_requirements() {
    for (source, flags) in [
        (r".*ſK", "ui"),
        (r".*😀", "u"),
        (r".*\ude00", ""),
        (r"(?<=ſK.*)z+", "ui"),
    ] {
        let enabled = compile_words(source, flags);
        let mut disabled = enabled.clone();
        disabled[2] &= 63;
        for subject in [
            format!("{}ſK😀z", "x".repeat(128)),
            format!("ſK😀z{}", "x".repeat(128)),
            format!("{}ſK😀{}z", "x".repeat(64), "x".repeat(64)),
            format!("{}sK", "x".repeat(128)),
            "x".repeat(128),
        ] {
            let units: Vec<_> = subject.encode_utf16().collect();
            for input in [Input::utf8(&subject), Input::utf16(&units)] {
                assert_eq!(
                    execute(&enabled, input, 0, 1_000_000),
                    execute(&disabled, input, 0, 1_000_000),
                    "{source}: {subject}"
                );
            }
        }
    }
}
#[test]
fn chunk_boundaries_relocation_and_budgets_do_not_change_answers() {
    let words = compile_words(r".*needle", "");
    for prefix in [63, 64, 253, 254, 255, 256, 511] {
        let subject = format!("{}needle", "x".repeat(prefix));
        assert_eq!(
            execute(&words, Input::utf8(&subject), 0, 100000).unwrap(),
            Some(vec![Span::new(0, prefix + 6)])
        );
    }
    let subject = "x".repeat(1000);
    assert_eq!(
        execute(&words, Input::utf8(&subject), 0, 100),
        Err(ExecError::WorkLimit)
    );
    let mut bad = words.clone();
    bad[2] = (bad[2] & 63) | 128 | 0xffffff00;
    assert_eq!(
        Program::from_words(&bad, &mut Budget::new(100000)).unwrap_err(),
        ProgramError::Invalid
    );
    let mut bad = words;
    bad[2] = (bad[2] & 63) | 128;
    assert_eq!(
        Program::from_words(&bad, &mut Budget::new(100000)).unwrap_err(),
        ProgramError::Invalid
    );
}
