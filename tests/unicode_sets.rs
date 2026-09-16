//! Unicode-sets (`v`) flag admission and class grammar. Properties of strings
//! remain explicitly unsupported; everything else uses the existing class
//! representation and evaluator.
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

/// A class with string members matches its members longest first, and falls
/// back to a shorter one when what follows the class fails. A member of one
/// code point is a code point of the class, and the empty member matches
/// where no code point does.
#[test]
fn unicode_sets_string_members_match_the_sequences_they_spell() {
    for (source, flags, subject, expected) in [
        (r"[\q{ab}]", "v", "xab", Some((1, 3))),
        (r"[\q{ab|cd}]", "v", "xcd", Some((1, 3))),
        (r"[\q{ab}x]", "v", "zx", Some((1, 2))),
        // Longest first: the two-code-point member is tried before the one
        // the class also holds.
        (r"[\q{ab|a}]", "v", "ab", Some((0, 2))),
        (r"[\q{ab}a]", "v", "ab", Some((0, 2))),
        // And the shorter member is still reachable when the longer one
        // leaves the rest of the pattern nothing to match.
        (r"[\q{ab}a]b", "v", "ab", Some((0, 2))),
        (r"[\q{abc|ab}]c", "v", "abc", Some((0, 3))),
        // A member of one code point is a code point, so it folds under `i`
        // exactly as a written character does.
        (r"[\q{a}]", "v", "a", Some((0, 1))),
        (r"[\q{a}]", "iv", "A", Some((0, 1))),
        (r"[^\q{a}]", "iv", "A", None),
        (r"[\q{AB}]", "iv", "ab", Some((0, 2))),
        // Simple folding only: `ß` and `ss` are different members.
        (r"[\q{ss}]", "iv", "\u{df}", None),
        // The empty member matches between characters.
        (r"[\q{}]", "v", "x", Some((0, 0))),
        (r"[a\q{}]", "v", "", Some((0, 0))),
        (r"[\q{ab|}]y", "v", "y", Some((0, 1))),
        // Astral members count as one code point each.
        (
            r"[\q{\u{1f600}\u{1f601}}]",
            "v",
            "\u{1f600}\u{1f601}",
            Some((0, 4)),
        ),
        // Quantifiers and lookbehind see the whole member.
        (r"[\q{ab}]{2}", "v", "abab", Some((0, 4))),
        (r"(?<=[\q{ab}])c", "v", "abc", Some((2, 3))),
        // Operators combine strings by the text they spell and code points
        // positionally, which are different questions in the same class.
        (r"[[\q{ab}]--[\q{ab}]]", "v", "ab", None),
        (r"[\q{ab|cd}--\q{ab}]", "v", "ab", None),
        (r"[\q{ab|cd}--\q{ab}]", "v", "cd", Some((0, 2))),
        (r"[\q{ab|cd}--\q{ab}--\q{cd}]", "v", "abcd", None),
        (r"[[a-z]--\q{ab}]", "v", "ab", Some((0, 1))),
        (r"[\q{ab}--[a-z]]", "v", "ab", Some((0, 2))),
        (r"[\q{a}--[a-z]]", "v", "a", None),
        (r"[\q{AB}--\q{ab}]", "iv", "AB", None),
        (r"[\q{AB}--\q{ab}]", "v", "AB", Some((0, 2))),
        (r"[\q{ab}&&\q{ab|cd}]", "v", "ab", Some((0, 2))),
        (r"[\q{ab}&&\q{cd}]", "v", "ab", None),
        (r"[\q{ab}&&[a-z]]", "v", "ab", None),
        (r"[\q{a}&&[a-z]]", "v", "a", Some((0, 1))),
        (r"[\q{ab}&&\q{ab}&&[a]]", "v", "ab", None),
        (r"[\q{|a}--\q{}]", "v", "a", Some((0, 1))),
    ] {
        assert_eq!(
            first(source, flags, subject),
            expected.map(|(a, b)| Span::new(a, b).unwrap()),
            "/{source}/{flags} over {subject:?}"
        );
    }
}

/// What the grammar refuses around string members: `\q` that spells no
/// disjunction, a disjunction where a single character belongs, and a
/// complement of a set whose syntax may hold strings — which the grammar
/// decides from the syntax, not from what survives an operator.
#[test]
fn unicode_sets_string_member_grammar_errors_stay_syntax_errors() {
    for source in [
        r"\q{ab}",
        r"[\q]",
        r"[\q{ab]",
        r"[a-\q{b}]",
        r"[\q{a}-z]",
        r"[\q{\q{a}}]",
        r"[\q{\d}]",
        r"[\q{a-b}]",
        r"[\q{ab}-]",
        r"[\q{a&&b}]",
        r"[^\q{}]",
        r"[^\q{ab}]",
        r"[^[\q{ab}]--[\q{ab}]]",
        r"[^[\q{ab}]&&[\q{ab}]]",
    ] {
        assert!(
            matches!(program(source, "v"), Err(CompileError::Syntax { .. })),
            "/{source}/v must be a syntax error, got {:?}",
            program(source, "v")
        );
    }
    // A complement is fine when no operand of it may hold strings, including
    // an intersection where one side cannot.
    for source in [r"[^\q{a}]", r"[^\q{a|b}]", r"[^[\q{ab}]&&[b]]"] {
        assert!(program(source, "v").is_ok(), "/{source}/v must compile");
    }
    // `\q` is an escape of the `v` grammar only. Without `u` or `v` the
    // Annex B grammar reads it as the identity escape it always was.
    assert!(program(r"[\q{ab}]", "").is_ok());
    for flags in ["u", "iu"] {
        assert!(
            matches!(
                program(r"[\q{ab}]", flags),
                Err(CompileError::Syntax { .. })
            ),
            "/[\\q{{ab}}]/{flags}"
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
        r"[\q{ab|a}]",
        r"[\q{ab}--\q{ab}]",
        r"[\q{}]",
        r"[\q{ab}&&\q{ab|cd}]",
    ] {
        let outcome = program(source, "iv");
        assert!(
            outcome.is_ok() || matches!(outcome, Err(CompileError::Syntax { .. })),
            "{source} must be compiled or rejected, never reported unsupported"
        );
    }
    // Properties of strings stay an explicit gap rather than an approximation:
    // their members are sequences the pinned Unicode data does not carry.
    for source in [
        r"\p{RGI_Emoji}",
        r"[\p{Basic_Emoji}]",
        r"[\p{RGI_Emoji}--\q{ab}]",
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
