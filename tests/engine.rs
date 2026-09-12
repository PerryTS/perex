use perex::{
    Budget,
    compiler::{CompileError, Node, Range, compile},
    executor::{ExecError, Frame, Scratch, Undo, find},
    input::Input,
    program::{Program, ProgramError},
    span::Span,
};

fn run(
    pattern: &str,
    flags: &str,
    subject: &str,
    start: usize,
) -> Option<Vec<Option<(usize, usize)>>> {
    let mut nodes = vec![Node::default(); 4096];
    let mut ranges = vec![Range::default(); 4096];
    let mut words = vec![0; 16384];
    let p = compile(
        Input::utf8(pattern),
        flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut Budget::new(1_000_000),
    )
    .unwrap();
    let mut registers = vec![0; p.register_count()];
    let mut frames = vec![Frame::default(); 4096];
    let mut undo = vec![Undo::default(); 16384];
    let mut captures = vec![None; p.capture_count()];
    find(
        p,
        Input::utf8(subject),
        start,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        &mut captures,
        &mut Budget::new(1_000_000),
    )
    .unwrap()
    .then(|| {
        captures
            .into_iter()
            .map(|c| c.map(|s| (s.start(), s.end())))
            .collect()
    })
}
#[test]
fn unicode_properties_preserve_complements_scripts_and_original_captures() {
    assert_eq!(
        run("[\\p{L}\\p{N}_]+", "u", " Ω1_!", 0),
        Some(vec![Some((1, 4))])
    );
    assert_eq!(run("\\P{Lowercase_Letter}", "u", "a", 0), None);
    assert_eq!(
        run("\\P{Lowercase_Letter}", "iu", "a", 0),
        Some(vec![Some((0, 1))])
    );
    assert_eq!(run("[^\\P{Lowercase_Letter}]", "iu", "a", 0), None);
    assert_eq!(run("\\p{Script=Hiragana}", "u", "ー", 0), None);
    assert_eq!(
        run("\\p{Script_Extensions=Hiragana}", "u", "ー", 0),
        Some(vec![Some((0, 1))])
    );
    assert_eq!(run("\\p{Script_Extensions=Common}", "u", "ー", 0), None);
    assert_eq!(
        run("(\\p{L})\\1", "iu", "𐐀𐐨", 0),
        Some(vec![Some((0, 4)), Some((0, 2))])
    );
    assert_eq!(
        run("(?<=\\p{L})\\p{Nd}", "u", "α3", 0),
        Some(vec![Some((1, 2))])
    );
    assert_eq!(run("\\p{Cs}", "u", "😀", 0), None);
    assert_eq!(
        run("\\p{Default_Ignorable_Code_Point}", "u", "\u{200d}", 0),
        Some(vec![Some((0, 1))])
    );
}
#[test]
fn property_programs_use_bounded_references_and_validate_relocation() {
    let mut nodes = [Node::default(); 8];
    let mut ranges = [Range::default(); 1];
    let mut words = [0; 32];
    for pattern in ["\\p{Any}", "\\p{Alphabetic}", "\\p{L}", "\\p{Script=Han}"] {
        let p = compile(
            Input::utf8(pattern),
            "u",
            &mut nodes,
            &mut ranges,
            &mut words,
            &mut Budget::new(10000),
        )
        .unwrap();
        // Ten header words, four instructions and one two-word property
        // reference, regardless of how many Unicode intervals it contains.
        assert_eq!(p.size_bytes(), 96);
        let mut moved = p.words().to_vec();
        words.fill(0xdeadbeef);
        let p = Program::from_words(&moved, &mut Budget::new(10000)).unwrap();
        assert_eq!(p.size_bytes(), 96);
        let mut registers = [0; 2];
        let mut frames = [Frame::default(); 1];
        let mut undo = [Undo::default(); 1];
        let mut captures = [None; 1];
        assert!(
            find(
                p,
                Input::utf8("字"),
                0,
                Scratch {
                    registers: &mut registers,
                    frames: &mut frames,
                    undo: &mut undo
                },
                &mut captures,
                &mut Budget::new(100)
            )
            .unwrap()
        );
        assert_eq!(captures, [Span::new(0, 1)]);
        let end = moved.len();
        moved[end - 1] = 2; // Complement is a Boolean, not arbitrary flags.
        assert_eq!(
            Program::from_words(&moved, &mut Budget::new(10000)).unwrap_err(),
            ProgramError::Invalid
        );
        moved[end - 1] = 0;
        moved[end - 2] = u32::MAX;
        assert_eq!(
            Program::from_words(&moved, &mut Budget::new(10000)).unwrap_err(),
            ProgramError::Invalid
        );
    }
    for source in [
        "\\p{Unknown}",
        "\\p{sc=latin}",
        "\\p{gc=Alphabetic}",
        "\\p{Other_Alphabetic}",
        "[a-\\p{L}]",
        "\\p{Alphabetic=True}",
    ] {
        assert!(matches!(
            compile(
                Input::utf8(source),
                "u",
                &mut nodes,
                &mut ranges,
                &mut words,
                &mut Budget::new(10000)
            ),
            Err(CompileError::Syntax { .. })
        ));
    }
}
#[test]
fn case_equivalence_obeys_mode_and_keeps_original_capture_spans() {
    assert_eq!(
        run("(s)(k)\\1", "iu", "ſKS", 0),
        Some(vec![Some((0, 3)), Some((0, 1)), Some((1, 2))])
    );
    assert_eq!(run("(s)(k)\\1", "i", "ſKS", 0), None);
    assert_eq!(run("[ω]", "iu", "Ω", 0), Some(vec![Some((0, 1))]));
    assert_eq!(run("[ω]", "i", "Ω", 0), None);
    assert_eq!(run("[ß]", "iu", "ẞ", 0), Some(vec![Some((0, 1))]));
    assert_eq!(run("[ß]", "i", "ẞ", 0), None);
    assert_eq!(run("ß", "iu", "ss", 0), None);
    assert_eq!(run("i", "iu", "ıİ", 0), None);
    assert_eq!(run("[\\W]", "iu", "ſK", 0), None);
    assert_eq!(run("[^\\W]+", "iu", "ſK", 0), Some(vec![Some((0, 2))]));
    assert_eq!(run("\\b\\w+\\b", "iu", " ſK!", 0), Some(vec![Some((1, 3))]));
    assert_eq!(
        run("(.)\\1", "iu", "𐐀𐐨", 0),
        Some(vec![Some((0, 4)), Some((0, 2))])
    );
    assert_eq!(
        run("(?<=^\\1(.))$", "iu", "𐐀𐐨", 0),
        Some(vec![Some((4, 4)), Some((2, 4))])
    );
    assert_eq!(run("[^a-z]", "iu", "ſK", 0), None);
    assert_eq!(run("[A-z]", "i", "_", 0), Some(vec![Some((0, 1))]));
}
#[test]
fn casefold_backreferences_borrow_each_original_string_representation() {
    let mut nodes = [Node::default(); 32];
    let mut ranges = [Range::default(); 32];
    let mut storage = [0; 256];
    let p = compile(
        Input::utf8("(𐐀)\\1(?<=𐐨)"),
        "iu",
        &mut nodes,
        &mut ranges,
        &mut storage,
        &mut Budget::new(10000),
    )
    .unwrap();
    let original_units = [0xd801, 0xdc00, 0xd801, 0xdc28];
    let original_bytes = [0xf0, 0x90, 0x90, 0x80, 0xf0, 0x90, 0x90, 0xa8];
    let separate_surrogates = [
        0xed, 0xa0, 0x81, 0xed, 0xb0, 0x80, 0xed, 0xa0, 0x81, 0xed, 0xb0, 0xa8,
    ];
    for input in [
        Input::utf16(&original_units),
        Input::wtf8(&original_bytes).unwrap(),
        Input::wtf8(&separate_surrogates).unwrap(),
    ] {
        let mut registers = [0; 4];
        let mut frames = [Frame::default(); 8];
        let mut undo = [Undo::default(); 16];
        let mut captures = [None; 2];
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
            .unwrap()
        );
        assert_eq!(captures, [Span::new(0, 4), Span::new(0, 2)]);
        assert!(
            captures[0]
                .unwrap()
                .units(input)
                .unwrap()
                .eq(original_units)
        );
        assert!(
            captures[1]
                .unwrap()
                .units(input)
                .unwrap()
                .eq([0xd801, 0xdc00])
        );
    }
}
#[test]
fn ordered_matching_repetition_and_resets() {
    assert_eq!(
        run("(ab|a)b", "", "ab", 0),
        Some(vec![Some((0, 2)), Some((0, 1))])
    );
    assert_eq!(
        run("(a|(b))+", "", "aba", 0),
        Some(vec![Some((0, 3)), Some((2, 3)), None])
    );
    assert_eq!(run("(a*)*", "", "b", 0), Some(vec![Some((0, 0)), None]));
    assert_eq!(
        run("(a*)+", "", "b", 0),
        Some(vec![Some((0, 0)), Some((0, 0))])
    );
    assert_eq!(run("a{2,4}?a", "", "aaaaa", 0), Some(vec![Some((0, 3))]));
    assert_eq!(run("a{2,4}a", "", "aaaaa", 0), Some(vec![Some((0, 5))]));
    assert_eq!(run("a*?b", "", "aaab", 0), Some(vec![Some((0, 4))]));
    assert_eq!(run("^$", "", "a", 0), None);
    assert_eq!(run("$", "", "x\n", 0), Some(vec![Some((2, 2))]));
    assert_eq!(run("$", "m", "x\n", 0), Some(vec![Some((1, 1))]));
}
#[test]
fn assertions_are_atomic_and_backreferences_observe_captures() {
    assert_eq!(
        run("(?=(a+))a*b\\1", "", "baabac", 0),
        Some(vec![Some((2, 5)), Some((2, 3))])
    );
    assert_eq!(
        run("(?!a(b))a(c)", "", "ac", 0),
        Some(vec![Some((0, 2)), None, Some((1, 2))])
    );
    assert_eq!(
        run("(?<=([ab]+)([bc]+))$", "", "abc", 0),
        Some(vec![Some((3, 3)), Some((0, 1)), Some((1, 3))])
    );
    assert_eq!(run("(?<!ab)c", "", "abc c", 0), Some(vec![Some((4, 5))]));
    assert_eq!(run("(a)?b\\1", "", "b", 0), Some(vec![Some((0, 1)), None]));
    assert_eq!(
        run("\\1(a)", "", "a", 0),
        Some(vec![Some((0, 1)), Some((0, 1))])
    );
}
#[test]
fn exact_string_modes_and_classes() {
    assert_eq!(
        run("(.)", "", "😀", 0),
        Some(vec![Some((0, 1)), Some((0, 1))])
    );
    assert_eq!(
        run("(.)", "uy", "😀", 1),
        Some(vec![Some((0, 2)), Some((0, 2))])
    );
    assert_eq!(run("\\ud83d", "u", "😀", 0), None);
    assert_eq!(
        run("\\ud83d\\ude00", "u", "😀", 0),
        Some(vec![Some((0, 2))])
    );
    assert_eq!(run("[^]", "u", "😀", 0), Some(vec![Some((0, 2))]));
    assert_eq!(run("[]", "", "x", 0), None);
    assert_eq!(run("[a-c-]+", "", "z-abcc", 0), Some(vec![Some((1, 6))]));
    assert_eq!(
        run("\\b\\w+\\s\\d+\\b", "", "a 12!", 0),
        Some(vec![Some((0, 4))])
    );
    assert_eq!(run(".", "", "\u{2028}", 0), None);
    assert_eq!(run(".", "s", "\u{2028}", 0), Some(vec![Some((0, 1))]));
}
#[test]
fn programs_relocate_and_do_not_retain_pattern_or_compile_scratch() {
    let mut nodes = vec![Node::default(); 128];
    let mut ranges = vec![Range::default(); 128];
    let mut storage = vec![0; 1024];
    let source = String::from("(?<=(a+))b\\1");
    let p = compile(
        Input::utf8(&source),
        "",
        &mut nodes,
        &mut ranges,
        &mut storage,
        &mut Budget::new(10000),
    )
    .unwrap();
    let moved = p.words().to_vec();
    assert_ne!(moved.as_ptr(), p.words().as_ptr());
    storage.fill(0xdeadbeef);
    drop(storage);
    drop(source);
    drop(nodes);
    drop(ranges);
    let p = Program::from_words(&moved, &mut Budget::new(10000)).unwrap();
    let mut registers = [0; 16];
    let mut frames = [Frame::default(); 64];
    let mut undo = [Undo::default(); 128];
    let mut captures = [None; 2];
    assert!(
        find(
            p,
            Input::utf8("aabaa"),
            0,
            Scratch {
                registers: &mut registers,
                frames: &mut frames,
                undo: &mut undo
            },
            &mut captures,
            &mut Budget::new(10000)
        )
        .unwrap()
    );
    assert_eq!(captures, [Span::new(2, 5), Span::new(0, 2)]);
}
#[test]
fn corruption_limits_and_partial_outputs_are_explicit() {
    let mut nodes = [Node::default(); 64];
    let mut ranges = [Range::default(); 64];
    let mut storage = [0; 512];
    let p = compile(
        // An expensive assertion at a plausible start must still report its
        // work limit. Candidate skipping may bypass impossible start positions.
        Input::utf8("(?!(a+)+b)a"),
        "",
        &mut nodes,
        &mut ranges,
        &mut storage,
        &mut Budget::new(10000),
    )
    .unwrap();
    let mut corrupt = p.words().to_vec();
    corrupt[9] = 999;
    assert_eq!(
        Program::from_words(&corrupt, &mut Budget::new(10000)).unwrap_err(),
        ProgramError::Invalid
    );
    let mut registers = [0; 32];
    let mut frames = [Frame::default(); 128];
    let mut undo = [Undo::default(); 512];
    let mut captures = [Span::new(0, 1); 2];
    let error = find(
        p,
        Input::utf8("aaaaaaaaaaaaac"),
        0,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        &mut captures,
        &mut Budget::new(200),
    );
    assert_eq!(error, Err(ExecError::WorkLimit));
    assert_eq!(captures, [Span::new(0, 1); 2]);
    let error = find(
        p,
        Input::utf8("a"),
        0,
        Scratch {
            registers: &mut registers,
            frames: &mut [],
            undo: &mut undo,
        },
        &mut captures,
        &mut Budget::new(10000),
    );
    assert_eq!(error, Err(ExecError::Frames));
    assert_eq!(captures, [Span::new(0, 1); 2]);
    assert!(matches!(
        compile(
            Input::utf8("a"),
            "",
            &mut nodes,
            &mut ranges,
            &mut [],
            &mut Budget::new(10000)
        ),
        Err(CompileError::ProgramStorage { .. })
    ));
    assert!(matches!(
        compile(
            Input::utf8("a"),
            "",
            &mut [],
            &mut ranges,
            &mut storage,
            &mut Budget::new(10000)
        ),
        Err(CompileError::Nodes)
    ));
}
#[test]
fn wide_capture_indices_and_long_sequences_do_not_use_recursive_emission() {
    let pattern = "()".repeat(256);
    let result = run(&pattern, "", "", 0).unwrap();
    assert_eq!(result.len(), 257);
    assert!(result.iter().all(|s| *s == Some((0, 0))));
    let pattern = "a".repeat(1000);
    assert_eq!(run(&pattern, "", &pattern, 0), Some(vec![Some((0, 1000))]));
}

#[test]
fn unicode_nonboundary_search_does_not_enter_a_surrogate_pair() {
    // RegExpBuiltinExec uses StringToCodePoints in Unicode mode. QuickJS agrees;
    // Node 26.8.1 has a separately recorded disagreement on zero-width \\B.
    assert_eq!(run("\\B", "u", "a😀b", 0), None);
    assert_eq!(run("\\B", "uy", "a😀b", 2), None);
    assert_eq!(run("\\B", "", "a😀b", 0), Some(vec![Some((2, 2))]));
}

#[test]
fn program_mutations_cannot_escape_bounds_or_work_limits() {
    let mut nodes = [Node::default(); 128];
    let mut ranges = [Range::default(); 128];
    let mut storage = [0; 2048];
    let p = compile(
        Input::utf8("(?<=(a|b)+)(?!c)[a-z]{1,3}\\1"),
        "u",
        &mut nodes,
        &mut ranges,
        &mut storage,
        &mut Budget::new(10000),
    )
    .unwrap();
    let original = p.words().to_vec();
    let mut registers = [0; 128];
    let mut frames = [Frame::default(); 128];
    let mut undo = [Undo::default(); 512];
    let mut captures = [None; 16];
    for at in 0..original.len() {
        for mask in [1, 2, 0x8000_0000, u32::MAX] {
            let mut changed = original.clone();
            changed[at] ^= mask;
            if let Ok(p) = Program::from_words(&changed, &mut Budget::new(10000)) {
                let _ = find(
                    p,
                    Input::utf8("aba😀b"),
                    0,
                    Scratch {
                        registers: &mut registers,
                        frames: &mut frames,
                        undo: &mut undo,
                    },
                    &mut captures,
                    &mut Budget::new(1000),
                );
            }
        }
    }
}

#[test]
fn work_and_undo_limits_do_not_become_negative_assertion_success() {
    let mut nodes = [Node::default(); 128];
    let mut ranges = [Range::default(); 128];
    let mut storage = [0; 2048];
    let p = compile(
        Input::utf8("(?!(a+)+b)c"),
        "",
        &mut nodes,
        &mut ranges,
        &mut storage,
        &mut Budget::new(10000),
    )
    .unwrap();
    let mut registers = [0; 128];
    let mut frames = [Frame::default(); 128];
    let mut undo = [Undo::default(); 512];
    let mut captures = [Span::new(7, 9); 2];
    assert_eq!(
        find(
            p,
            Input::utf8("c"),
            0,
            Scratch {
                registers: &mut registers,
                frames: &mut frames,
                undo: &mut []
            },
            &mut captures,
            &mut Budget::new(1000)
        ),
        Err(ExecError::Undo)
    );
    assert_eq!(captures, [Span::new(7, 9); 2]);
    let p = compile(
        Input::utf8("z"),
        "",
        &mut nodes,
        &mut ranges,
        &mut storage,
        &mut Budget::new(10000),
    )
    .unwrap();
    assert_eq!(
        find(
            p,
            Input::utf8("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            0,
            Scratch {
                registers: &mut registers,
                frames: &mut frames,
                undo: &mut undo
            },
            &mut captures,
            &mut Budget::new(30)
        ),
        Err(ExecError::WorkLimit)
    );
    assert_eq!(captures, [Span::new(7, 9); 2]);
}

#[test]
fn compile_work_nesting_and_invalid_extensions_are_distinct() {
    let mut nodes = vec![Node::default(); 1024];
    let mut ranges = [Range::default(); 64];
    let mut words = vec![0; 4096];
    for pattern in ["(?r)", "(?q:a)", "(?", "a{3,2}", "[z-a]"] {
        assert!(
            matches!(
                compile(
                    Input::utf8(pattern),
                    "",
                    &mut nodes,
                    &mut ranges,
                    &mut words,
                    &mut Budget::new(10000)
                ),
                Err(CompileError::Syntax { .. })
            ),
            "{pattern}"
        );
    }
    assert!(matches!(
        compile(
            Input::utf8("a"),
            "",
            &mut nodes,
            &mut ranges,
            &mut words,
            &mut Budget::new(0)
        ),
        Err(CompileError::WorkLimit)
    ));
    let deep = format!("{}a{}", "(".repeat(200), ")".repeat(200));
    assert!(matches!(
        compile(
            Input::utf8(&deep),
            "",
            &mut nodes,
            &mut ranges,
            &mut words,
            &mut Budget::new(10000)
        ),
        Err(CompileError::NestingLimit)
    ));
}

#[test]
fn noncapturing_group_retains_quantifiable_atom_boundary() {
    assert_eq!(run("(?:^)*", "u", "a", 0), Some(vec![Some((0, 0))]));
    assert_eq!(run("(?:(?=a))+", "u", "a", 0), Some(vec![Some((0, 0))]));
    assert_eq!(run("(?:(?!b)){2}", "u", "a", 0), Some(vec![Some((0, 0))]));
    assert_eq!(run("(?:(?<=a))+", "u", "ab", 0), Some(vec![Some((1, 1))]));
}
