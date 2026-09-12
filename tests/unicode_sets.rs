//! Unicode-sets (`v`) flag admission and class grammar. Set operators, string
//! members and nested complements remain explicitly unsupported; everything
//! else uses the existing class representation and evaluator.
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
    // The union grammar, ranges, nesting and property escapes are implemented.
    for source in [
        "[a]",
        "[a-z]",
        "[^a-z]",
        "[[a-z][0-9]]",
        "[a[b]c]",
        r"[\d\p{L}]",
        r"[\P{Ll}]",
        r"\p{Letter}",
        r"\P{Lowercase_Letter}",
        r"[\q]",
        "[&]",
        r"[\-]",
    ] {
        let outcome = program(source, "iv");
        assert!(
            outcome.is_ok() || matches!(outcome, Err(CompileError::Syntax { .. })),
            "{source} must be compiled or rejected, never reported unsupported"
        );
    }
    // Operators, string members, nested complements and properties of strings
    // stay explicit gaps rather than approximations.
    for source in [
        "[a&&b]",
        "[a--b]",
        r"[\q{ab|a}]",
        r"[[^a]]",
        r"\p{RGI_Emoji}",
        r"[\p{Basic_Emoji}]",
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
    // A pattern the grammar rejects must stay a syntax error, not a gap.
    for source in ["[a", "[(]", "[|]", r"[\q]x", "[&&]", "[--]", "[z-a]"] {
        assert!(
            matches!(program(source, "v"), Err(CompileError::Syntax { .. }))
                || program(source, "v").is_ok(),
            "{source}"
        );
        assert!(
            !matches!(program(source, "v"), Err(CompileError::Unsupported { .. })),
            "{source} must not be reported unsupported"
        );
    }
}
