//! Initial Unicode-sets flag admission. Remaining class/property syntax is
//! explicitly unsupported; these complete programs use the existing evaluator.
use perex::{
    Budget,
    compiler::{CompileError, Node, Range, compile},
    input::Input,
};

fn program(source: &str, flags: &str) -> Result<Vec<u32>, CompileError> {
    let mut nodes = vec![Node::default(); 512];
    let mut ranges = vec![Range::default(); 512];
    let mut words = vec![0; 8192];
    compile(
        Input::utf8(source),
        flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut Budget::new(1_000_000),
    )
    .map(|p| p.words().to_vec())
}

#[test]
fn unicode_sets_nonclass_patterns_share_the_complete_unicode_program() {
    // Exact emitted words include instructions, capture/reset metadata, names,
    // lexical flags and Unicode matching policy, not just the first match.
    for source in [
        "",
        "(?:)",
        ".",
        "😀+",
        r"(a|(b))+\2",
        r"(?<pair>.)(?<=\k<pair>)",
        r"(?i:\w)(?-i:\W)",
        r"\b\w+\b",
        r"\D\S",
        r"\u{1f600}",
        r"(?<=ab)c|^c$",
    ] {
        for (u, v) in [("u", "v"), ("iu", "iv"), ("guy", "gvy")] {
            assert_eq!(
                program(source, u).unwrap(),
                program(source, v).unwrap(),
                "{source}/{v}"
            );
        }
    }
}

#[test]
fn unicode_sets_admission_keeps_syntax_errors_and_remaining_omissions_explicit() {
    for (source, flags) in [
        ("", "uv"),
        ("", "vv"),
        (r"\a", "v"),
        (r"\8", "v"),
        ("a{", "v"),
        ("(?=a)?", "v"),
    ] {
        assert!(
            matches!(program(source, flags), Err(CompileError::Syntax { .. })),
            "{source}/{flags}"
        );
    }
    for source in [
        "[a]",
        "[a&&b]",
        "[a--b]",
        r"[\q{ab|a}]",
        r"\p{Letter}",
        r"\P{Lowercase_Letter}",
        r"\p{RGI_Emoji}",
    ] {
        assert!(
            matches!(
                program(source, "iv"),
                Err(CompileError::Unsupported {
                    feature: "Unicode sets",
                    ..
                })
            ),
            "{source}"
        );
    }
}
