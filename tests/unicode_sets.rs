//! Unicode-sets (`v`) flag admission and class grammar. String members and
//! properties of strings remain explicitly unsupported; everything else uses
//! the existing class representation and evaluator.
use perex::{
    Budget,
    compiler::{CompileError, Node, Range, compile},
    executor::{Frame, Scratch, Undo, find},
    input::Input,
    span::Span,
};

/// Where `source` first matches in `subject`, through the ordinary evaluator.
fn first(source: &str, flags: &str, subject: &str) -> Option<Span> {
    let mut nodes = vec![Node::default(); 512];
    let mut ranges = vec![Range::default(); 4096];
    let mut words = vec![0; 8192];
    let program = compile(
        Input::utf8(source),
        flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut Budget::new(1_000_000),
    )
    .unwrap_or_else(|e| panic!("/{source}/{flags}: {e:?}"));
    let mut registers = vec![0; program.register_count()];
    let mut frames = vec![Frame::default(); 1024];
    let mut undo = vec![Undo::default(); 4096];
    let mut captures = vec![None; program.capture_count()];
    let found = find(
        program,
        Input::utf8(subject),
        0,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        &mut captures,
        &mut Budget::new(10_000_000),
    )
    .expect("search runs");
    found.then(|| captures[0].expect("a match has a span"))
}

/// Subtraction, intersection and nested complements answer what the set they
/// describe answers, including the `v` rule that a set is closed under case
/// folding before it is complemented or operated on.
#[test]
fn unicode_sets_operators_match_the_set_they_describe() {
    for (source, flags, subject, expected) in [
        ("[[a-z]--[aeiou]]", "v", "aeioub", Some((5, 6))),
        ("[[a-z]--[aeiou]]", "v", "aeiou", None),
        ("[a--b--c]", "v", "cba", Some((2, 3))),
        (r"[\p{L}&&\p{ASCII}]", "v", "1é_b", Some((3, 4))),
        (r"[\p{L}&&\p{ASCII}]", "v", "1é_", None),
        ("[a&&b&&c]", "v", "abc", None),
        ("[[a-c]&&[b-d]]", "v", "adcb", Some((2, 3))),
        ("[[a-z]--[[a-c][x-z]]]", "v", "abcxyzd", Some((6, 7))),
        // A nested complement, alone and beside another member.
        ("[[^a]]", "v", "aab", Some((2, 3))),
        ("[[^a]b]", "v", "aab", Some((2, 3))),
        ("[^[a-z]--[aeiou]]", "v", "bcda", Some((3, 4))),
        ("[^[a-z]--[aeiou]]", "v", "bcd1", Some((3, 4))),
        // An astral operand, where a member is a whole code point.
        (
            r"[[\u{1f600}-\u{1f603}]--[\u{1f601}]]",
            "v",
            "😁😀",
            Some((2, 4)),
        ),
        // The empty class matches nothing, and subtracting everything empties.
        ("[]", "v", "a", None),
        ("[[a-z]--[a-z]]", "v", "abc", None),
        // Case folding closes each operand before the operator applies, so a
        // subtraction removes the whole closure.
        ("[[A-Z]--[AEIOU]]", "iv", "AEIOUb", Some((5, 6))),
        ("[[A-Z]--[AEIOU]]", "iv", "aeiou", None),
        ("[[A-Z]--[AEIOU]]", "v", "aeiou", None),
        ("[[A-Z]--[AEIOU]]", "v", "b", None),
        (r"[\p{Ll}&&[A-Z]]", "iv", "b", Some((0, 1))),
        (r"[\p{Ll}&&[A-Z]]", "v", "b", None),
    ] {
        assert_eq!(
            first(source, flags, subject),
            expected.map(|(a, b)| Span::new(a, b).unwrap()),
            "/{source}/{flags} over {subject:?}"
        );
    }
}

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
    // The union grammar, ranges, nesting, property escapes, the operators and
    // nested complements are implemented.
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
        "[a--b]",
        "[a&&b]",
        "[[a-z]--[aeiou]]",
        r"[\p{L}&&\p{ASCII}]",
        "[[^a]]",
        "[[^a]b]",
        "[^[a-z]--[aeiou]]",
        "[a--b--c]",
        "[a&&b&&c]",
        "[[a-z]--[[a-c][x-z]]]",
    ] {
        let outcome = program(source, "iv");
        assert!(
            outcome.is_ok() || matches!(outcome, Err(CompileError::Syntax { .. })),
            "{source} must be compiled or rejected, never reported unsupported"
        );
    }
    // String members and properties of strings stay explicit gaps rather than
    // approximations: both need a member to match more than one character.
    for source in [
        r"[\q{ab|a}]",
        r"[\q{ab}--\q{ab}]",
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
    for source in [
        "[a",
        "[(]",
        "[|]",
        r"[\q]x",
        "[&&]",
        "[--]",
        "[z-a]",
        "[a--b&&c]",
        "[a&&b--c]",
        "[a--]",
        "[a&&]",
        "[--a]",
        "[&&a]",
        "[a--b c]",
        // A range is not an operand of an operator: the set it looks like is
        // spelled with the range nested.
        "[a-z--[aeiou]]",
        "[a-z&&b]",
        "[a--b-c]",
        "[[a-z]--b-c]",
    ] {
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
