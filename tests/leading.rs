//! The leading-literal descriptor (program word 9) and the start positions it
//! removes. The descriptor is re-derived by `Program::from_words`, so a program
//! whose word disagrees with its instructions cannot be executed at all; these
//! checks cover the derivation, that rejection, and complete match answers.
use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{Frame, Scratch, Undo, find},
    input::Input,
    program::{Program, ProgramError},
    span::Span,
};

/// Word 9's low byte is the leading run; its top bit is the separate
/// forward-admission claim, which these checks are not about.
fn leading(source: &str, flags: &str) -> u32 {
    words(source, flags)[9] & 255
}

fn words(source: &str, flags: &str) -> Vec<u32> {
    let mut nodes = vec![Node::default(); source.len() * 3 + 32];
    let mut ranges = vec![Range::default(); source.len() * 12 + 32];
    let mut storage = vec![0; source.len() * 48 + 128];
    compile(
        Input::utf8(source),
        flags,
        &mut nodes,
        &mut ranges,
        &mut storage,
        &mut Budget::new(2_000_000),
    )
    .unwrap()
    .words()
    .to_vec()
}

fn run(words: &[u32], input: Input<'_>, start: usize, work: usize) -> Option<Vec<Option<Span>>> {
    let p = Program::from_words(words, &mut Budget::new(100_000)).unwrap();
    let mut registers = vec![0; p.register_count()];
    let mut frames = vec![Frame::default(); 1024];
    let mut undo = vec![Undo::default(); 8192];
    let mut captures = vec![None; p.capture_count()];
    find(
        p,
        input,
        start,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        &mut captures,
        &mut Budget::new(work),
    )
    .unwrap()
    .then_some(captures)
}

#[test]
fn leading_descriptor_claims_only_unconditional_ascii_characters() {
    // A straight-line run of entry bookkeeping and ASCII characters.
    assert_eq!(leading("needle", ""), 6);
    assert_eq!(leading("ab", ""), 2);
    // Interior capture bookkeeping ends the contiguous run; the shorter claim
    // is still correct, because those three characters are consumed first.
    assert_eq!(leading("(abc)d", ""), 3);
    // A branch of literal runs is claimed as an alternation, whose branches are
    // read from the instructions rather than stored.
    for source in ["(?:ab|cd)ef", "ab|cd", "(?:ab|cd|ef)", "(?:abc|de)"] {
        assert_eq!(leading(source, ""), 1, "/{source}/");
    }
    // A branch needs every alternative to be a literal run of its own.
    for source in [
        "(?:a|cd)ef",
        "(?:ab|[c-d]d)",
        "(?:ab|c+d)",
        "(?:ab|\u{e9}f)",
    ] {
        assert_eq!(leading(source, ""), 0, "/{source}/");
    }
    for (source, flags) in [
        ("a", ""),          // One character is already the word 7 claim.
        ("a+END", ""),      // A repeat can consume before the literal.
        ("^abc", ""),       // An assertion instruction ends the run.
        ("[ab]cd", ""),     // A class is not a literal character.
        ("NeEdLe", "i"),    // Folded characters are not a byte claim.
        ("\u{e9}tude", ""), // Non-ASCII cannot be compared against bytes.
        ("a?bc", ""),       // An optional prefix can consume nothing.
    ] {
        assert_eq!(leading(source, flags), 0, "/{source}/{flags}");
    }
}

#[test]
fn a_descriptor_disagreeing_with_its_instructions_is_rejected() {
    let valid = words("needle", "");
    assert_eq!(valid[9] & 255, 6);
    for claim in [0, 1, 2, 5, 7, 32, 33, u32::MAX] {
        let mut corrupt = valid.clone();
        corrupt[9] = claim;
        assert_eq!(
            Program::from_words(&corrupt, &mut Budget::new(1_000_000)).unwrap_err(),
            ProgramError::Invalid,
            "claim {claim}"
        );
    }
    // A program whose instructions change must carry the matching claim.
    let mut shortened = words("(abc)d", "");
    assert_eq!(shortened[9] & 255, 3);
    shortened[9] = 4;
    assert_eq!(
        Program::from_words(&shortened, &mut Budget::new(1_000_000)).unwrap_err(),
        ProgramError::Invalid
    );
}

#[test]
fn skipped_starts_preserve_complete_answers() {
    let w = words("needle", "");
    // Positions that share the first character but not the literal are the
    // ones the descriptor removes; the surviving answer must be unchanged.
    let mut subject = "nnnneedlxnedle nee ".repeat(40);
    subject.push_str("needle tail");
    let at = subject.find("needle ").unwrap();
    let found = run(&w, Input::utf8(&subject), 0, 1_000_000).unwrap();
    assert_eq!(found[0], Span::new(at, at + 6));

    // A subject whose remaining bytes cannot hold the literal has no match,
    // including when its first characters match a prefix of it.
    for tail in ["", "n", "ne", "need", "needl"] {
        let subject = format!("xxxx{tail}");
        assert!(
            run(&w, Input::utf8(&subject), 0, 1_000_000).is_none(),
            "{tail}"
        );
    }

    // Every start offset reports the same answer the general search would.
    let subject = "needle needle";
    for start in 0..=subject.len() {
        let found = run(&w, Input::utf8(subject), start, 1_000_000);
        let expected = subject[start..].find("needle").map(|i| start + i);
        assert_eq!(
            found.map(|c| c[0].unwrap().start()),
            expected,
            "start {start}"
        );
    }
}

#[test]
fn the_descriptor_does_not_change_sticky_or_captured_answers() {
    // Interior capture bookkeeping ends the run after the first group.
    let sticky = words("(ne)(edle)", "y");
    assert_eq!(sticky[9] & 255, 2);
    let subject = "xneedle needle";
    assert!(run(&sticky, Input::utf8(subject), 0, 1_000_000).is_none());
    let captures = run(&sticky, Input::utf8(subject), 1, 1_000_000).unwrap();
    assert_eq!(captures[0], Span::new(1, 7));
    assert_eq!(captures[1], Span::new(1, 3));
    assert_eq!(captures[2], Span::new(3, 7));
    // A sticky start that shares only the first character still reports no match.
    assert!(run(&sticky, Input::utf8("xnxxxxxx needle"), 1, 1_000_000).is_none());
}

#[test]
fn non_ascii_storage_keeps_the_general_start_search() {
    let w = words("needle", "");
    // The byte claim only applies to ASCII storage. These subjects reach the
    // mixed and UTF-16 paths and must still produce identical answers.
    let mixed = "\u{e9}\u{1f600}nnneedle needle";
    let at = mixed.find("needle").unwrap();
    let found = run(&w, Input::utf8(mixed), 0, 1_000_000).unwrap();
    // Spans are UTF-16 offsets, so the astral prefix counts as two units.
    assert_eq!(
        found[0].unwrap().start(),
        mixed[..at].encode_utf16().count()
    );

    let units: Vec<u16> = "\u{1f600}nneedle".encode_utf16().collect();
    let found = run(&w, Input::utf16(&units), 0, 1_000_000).unwrap();
    assert_eq!(found[0], Span::new(3, 9));
}
