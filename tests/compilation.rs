//! Which programs the compilation tier could emit code for. See
//! `docs/compilation.md`; nothing is generated yet, and this fixes the contract
//! of the analysis that decides what would be.
use perex::{
    Budget,
    compiler::{Node, Range, compile},
    input::Input,
};

fn compilable(pattern: &str, flags: &str) -> bool {
    let source = Input::utf8(pattern);
    let mut nodes = vec![Node::default(); source.len_utf16() * 3 + 32];
    let mut ranges = vec![Range::default(); source.len_utf16() * 12 + 32];
    let mut words = vec![0u32; source.len_utf16() * 48 + 128];
    let mut budget = Budget::new(10_000_000);
    let program = compile(
        source,
        flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut budget,
    )
    .expect("pattern compiles");
    program.compilable()
}

#[test]
fn admits_the_subset_it_documents() {
    // Characters, classes of ASCII ranges, `.`, and captures around them.
    for pattern in [
        "a",
        "abc",
        "a.c",
        "[a-z]",
        "[a-z0-9_]",
        "[^0-9]",
        "(a)",
        "(a)(b)",
        "(ab)c",
    ] {
        assert!(compilable(pattern, ""), "{pattern} should be admitted");
    }
}

#[test]
fn admits_a_repeat_whether_or_not_it_is_bounded() {
    // What bounds how long generated code runs before the collector may have it
    // is the budget it is given, not the pattern. Requiring a static bound here
    // was measured against the benchmark first and admitted only trivial
    // literals -- which are the cases already closest to V8 -- so the line is
    // drawn at what can be emitted, not at what is bounded.
    for pattern in [
        "a{3}",
        "a{2,5}",
        "[a-z]{1,4}",
        "a{0,2}b",
        "a?",
        "a+",
        "a*",
        "[a-z]+",
        "a{2,}",
        ".*",
        "[a-z]+[0-9]+",
        "a+END",
    ] {
        assert!(compilable(pattern, ""), "{pattern} should be admitted");
    }
}

#[test]
fn refuses_a_repeat_that_resets_captures() {
    // A repeat with a capture inside resets it per iteration and emits
    // `REPEAT_INIT` for that. Admitting it is a separate decision.
    for pattern in ["(a){2}", "(a)+", "(a|b)*"] {
        assert!(!compilable(pattern, ""), "{pattern} should be refused");
    }
}

#[test]
fn admits_folding_anchors_and_a_literal_lookbehind() {
    // A folded ASCII comparison is exact on the storage this tier requires, the
    // anchors are position tests, a word boundary is two membership tests, and a
    // lookbehind whose body is a run of characters is a comparison against the
    // bytes before the position -- which the executor already makes over bytes.
    for (pattern, flags) in [
        ("NeEdLe", "i"),
        ("[a-z]", "i"),
        ("^a", ""),
        ("a$", ""),
        ("^a$", "m"),
        ("\\ba", ""),
        ("(?<=abc)d", ""),
        ("(?<!abc)d", ""),
    ] {
        assert!(
            compilable(pattern, flags),
            "/{pattern}/{flags} should be admitted"
        );
    }
}

#[test]
fn refuses_what_the_subset_excludes() {
    for (pattern, flags) in [
        ("a|b", ""),               // alternation needs branching control flow
        ("(a)\\1", ""),            // backreference
        ("(?=a)b", ""),            // lookahead needs a sub-search
        ("(?!a)b", ""),            // so does a negative one
        ("(?<=[ab])c", ""),        // a lookbehind body that is not a run
        ("(?<=a+)b", ""),          // nor this one
        ("(?<=(a))b", ""),         // nor this one
        ("\\p{L}", "u"),           // property table
        ("\u{e9}", ""),            // non-ASCII character
        ("\u{e9}", "i"),           // and folded, where ASCII arithmetic is not exact
        ("[\u{100}-\u{200}]", ""), // non-ASCII range
    ] {
        assert!(
            !compilable(pattern, flags),
            "/{pattern}/{flags} should be refused"
        );
    }
}

#[test]
fn refusal_is_not_rejection() {
    // Everything the analysis refuses still compiles to a program and is still
    // searched; the tier is an optimization of the interpreter, never a gate on
    // what the engine accepts.
    for (pattern, flags) in [("a+", ""), ("a|b", ""), ("(a)\\1", ""), ("\\p{L}", "u")] {
        let source = Input::utf8(pattern);
        let mut nodes = vec![Node::default(); source.len_utf16() * 3 + 32];
        let mut ranges = vec![Range::default(); source.len_utf16() * 12 + 32];
        let mut words = vec![0u32; source.len_utf16() * 48 + 128];
        let mut budget = Budget::new(10_000_000);
        assert!(
            compile(
                source,
                flags,
                &mut nodes,
                &mut ranges,
                &mut words,
                &mut budget
            )
            .is_ok(),
            "/{pattern}/{flags} still compiles"
        );
    }
}
