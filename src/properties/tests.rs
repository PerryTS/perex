use super::*;
use crate::{
    Budget,
    binding::BoundProgram,
    compiler::{Node, Range, compile},
    dfa::{self, Found},
    executor::{Frame, Scratch, Undo, find},
    input::Input,
    program::Program,
    span::Span,
};
use std::vec;
#[cfg(feature = "selectable-unicode")]
use std::vec::Vec;

fn compiled<'a>(pattern: &str, flags: &str, words: &'a mut [u32]) -> Program<'a> {
    compile(
        Input::utf8(pattern),
        flags,
        &mut [Node::default(); 512],
        &mut [Range::default(); 1024],
        words,
        &mut Budget::new(10_000_000),
    )
    .unwrap()
}

fn answers(program: Program<'_>, subject: &str, expected: &[Option<Span>]) {
    let input = Input::utf8(subject);
    let mut registers = vec![0; program.register_count()];
    let mut frames = [Frame::default(); 128];
    let mut undo = [Undo::default(); 1024];
    let mut captures = vec![None; program.capture_count()];
    for _ in 0..8 {
        assert!(
            find(
                program,
                input,
                0,
                Scratch {
                    registers: &mut registers,
                    frames: &mut frames,
                    undo: &mut undo,
                },
                &mut captures,
                &mut Budget::new(1_000_000)
            )
            .unwrap()
        );
        assert_eq!(captures, expected);
    }
    if dfa::eligible(program) {
        for size in [dfa::minimum_words(program).unwrap(), 65536] {
            let mut cache = vec![0; size];
            for _ in 0..8 {
                assert_eq!(
                    dfa::find(program, input, 0, &mut cache, &mut Budget::new(1_000_000)).unwrap(),
                    Found::Match(expected[0].unwrap())
                );
                assert_eq!(
                    dfa::is_match(program, input, 0, &mut cache, &mut Budget::new(1_000_000))
                        .unwrap(),
                    dfa::Tested::Match,
                );
                if subject.is_ascii() && size == 65536 {
                    let mut actual = vec![None; program.capture_count()];
                    assert_eq!(
                        dfa::captures(
                            program,
                            input,
                            expected[0].unwrap().start(),
                            &mut cache,
                            &mut Budget::new(1_000_000),
                            &mut actual,
                        )
                        .unwrap(),
                        Found::Match(expected[0].unwrap()),
                    );
                    assert_eq!(actual, expected);
                }
            }
        }
    }
}

#[test]
fn installed_membership_agrees_with_database() {
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(install_unicode_properties);
        }
    });
    install_unicode_properties();
    let membership = Resolver::default().require();
    for id in 0..data::PROPERTIES.len() as u32 {
        for point in [0, 65, 127, 128, 0xe9, 0x3b1, 0x200c, 0x1f600, 0x10ffff] {
            assert_eq!(membership.contains(id, point), contains_data(id, point));
        }
    }
}

#[cfg(feature = "selectable-unicode")]
fn uninstalled_child() -> bool {
    let Ok(words) = std::env::var("PEREX_TEST_UNINSTALLED_WORDS") else {
        return false;
    };
    assert!(MEMBERSHIP.get().is_none());
    let mut plain = [0; 4096];
    let p = compiled(r"\s+", "", &mut plain);
    answers(p, "x  !", &[Span::new(1, 3)]);
    Program::from_words(p.words(), &mut Budget::new(100_000)).unwrap();
    assert_eq!(SLOT_READS.with(|reads| reads.get()), 0);
    for pattern in [r"\p{L}", "(?<α>a)"] {
        assert!(
            std::panic::catch_unwind(|| {
                compiled(pattern, "u", &mut [0; 4096]);
            })
            .is_err()
        );
    }
    let words: Vec<u32> = words.split(',').map(|s| s.parse().unwrap()).collect();
    assert!(
        std::panic::catch_unwind(|| {
            Program::from_words(&words, &mut Budget::new(1_000_000)).unwrap();
        })
        .is_err()
    );
    true
}

#[test]
fn unicode_membership_is_resolved_only_on_cold_paths() {
    #[cfg(feature = "selectable-unicode")]
    if uninstalled_child() {
        return;
    }
    install_unicode_properties();
    let cases: &[(&str, &str, &str, &[Option<Span>])] = &[
        (
            r"(\p{L}+)",
            "gu",
            "12 αβ!A",
            &[Span::new(3, 5), Span::new(3, 5)],
        ),
        (
            r"(\p{L}+)",
            "gu",
            "12 Ab!A",
            &[Span::new(3, 5), Span::new(3, 5)],
        ),
        (r"[\p{Script=Greek}\p{Nd}]", "u", "!?Δ9", &[Span::new(2, 3)]),
        (r"(\p{Lu})", "iu", "!ω", &[Span::new(1, 2), Span::new(1, 2)]),
        (r"[\P{Ll}]", "iv", "!a", &[Span::new(0, 1)]),
        (
            r"[\p{L}&&\p{Script=Greek}]+",
            "v",
            "aαβ1",
            &[Span::new(1, 3)],
        ),
        (
            "(?<λέξη>α+)",
            "u",
            "1αα",
            &[Span::new(1, 3), Span::new(1, 3)],
        ),
        (r"\p{RGI_Emoji}", "v", "x😀", &[Span::new(1, 3)]),
        (
            r"(?<=\p{L})(\p{Nd})",
            "u",
            "α3",
            &[Span::new(1, 2), Span::new(1, 2)],
        ),
    ];
    for &(pattern, flags, subject, expected) in cases {
        #[cfg(feature = "selectable-unicode")]
        let before = SLOT_READS.with(|reads| reads.get());
        let mut words = [0; 4096];
        let p = compiled(pattern, flags, &mut words);
        #[cfg(feature = "selectable-unicode")]
        assert_eq!(
            SLOT_READS.with(|reads| reads.get()),
            before + 1,
            "{pattern}"
        );
        answers(p, subject, expected);
        #[cfg(feature = "selectable-unicode")]
        assert_eq!(SLOT_READS.with(|reads| reads.get()), before + 1);
        let moved = p.words().to_vec();
        words.fill(0xdeadbeef);
        let bound = BoundProgram::new(moved.as_slice(), &mut Budget::new(10_000_000)).unwrap();
        #[cfg(feature = "selectable-unicode")]
        let resolved = SLOT_READS.with(|reads| reads.get());
        #[cfg(feature = "selectable-unicode")]
        assert_eq!(resolved, before + 2);
        let witness = bound.witness();
        let relocated = bound.into_storage().to_vec();
        let rebound = BoundProgram::new_witnessed(relocated.as_slice(), witness).unwrap();
        assert_eq!(rebound.witness(), witness);
        for _ in 0..4 {
            rebound
                .with_view(|p| answers(p, subject, expected))
                .unwrap();
        }
        #[cfg(feature = "selectable-unicode")]
        assert_eq!(SLOT_READS.with(|reads| reads.get()), resolved);

        #[cfg(feature = "selectable-unicode")]
        if pattern == r"(\p{L}+)" || pattern == "(?<λέξη>α+)" {
            use std::string::ToString;
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "properties::tests::unicode_membership_is_resolved_only_on_cold_paths",
                ])
                .env(
                    "PEREX_TEST_UNINSTALLED_WORDS",
                    moved
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(","),
                )
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "{}\n{}",
                std::string::String::from_utf8_lossy(&child.stdout),
                std::string::String::from_utf8_lossy(&child.stderr)
            );
        }
    }
}
