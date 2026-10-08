//! Alternation is laid out flat, and a branch whose first arm can only begin
//! with characters from a known set skips that arm, and its frame, when the
//! next character is outside it. Programs are anchored by a stored, validated
//! claim. A trial clears only its capture registers. See `docs/branch.md`.
use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{ExecError, Frame, Scratch, Undo, find},
    input::Input,
    program::{Program, ProgramError},
    span::Span,
};

fn words(source: &str, flags: &str) -> Vec<u32> {
    let mut nodes = vec![Node::default(); source.len() * 3 + 64];
    let mut ranges = vec![Range::default(); source.len() * 12 + 64];
    let mut storage = vec![0; source.len() * 48 + 256];
    compile(
        Input::utf8(source),
        flags,
        &mut nodes,
        &mut ranges,
        &mut storage,
        &mut Budget::new(2_000_000),
    )
    .unwrap_or_else(|e| panic!("/{source}/{flags}: {e:?}"))
    .words()
    .to_vec()
}

type Answer = Result<Option<Vec<Option<Span>>>, ExecError>;

/// A search's complete answer from `start`, with registers that start out
/// holding `garbage` rather than anything the engine wrote.
fn run(words: &[u32], subject: &str, start: usize, garbage: usize) -> Answer {
    let p = Program::from_words(words, &mut Budget::new(1_000_000)).unwrap();
    let mut registers = vec![garbage; p.register_count()];
    let mut frames = vec![Frame::default(); 4096];
    let mut undo = vec![Undo::default(); 32768];
    let mut captures = vec![None; p.capture_count()];
    let mut budget = Budget::new(10_000_000);
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
        &mut budget,
    )
    .map(|found| found.then(|| captures.clone()))
}

fn branches(words: &[u32]) -> usize {
    (0..words[4] as usize)
        .filter(|pc| words[11 + pc * 3] == 31)
        .count()
}

/// Each alternative, at every depth, written between empty lookaheads, so its
/// arm begins with an assertion, so no branch carries a set: the same
/// answers, decided by trying every arm.
fn unfiltered(source: &str) -> String {
    const EMPTY: &str = "(?=)";
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::from("(?:");
    out.push_str(EMPTY);
    let mut i = 0;
    let mut class = false;
    while i < chars.len() {
        let c = chars[i];
        // A lookbehind runs its alternatives backwards, so each one gets the
        // assertion at its end as well as at its start.
        if !class && matches!(c, '|' | ')') {
            out.push_str(EMPTY);
        }
        out.push(c);
        i += 1;
        match c {
            '\\' => {
                if i < chars.len() {
                    out.push(chars[i]);
                    i += 1;
                }
            }
            '[' if !class => class = true,
            ']' if class => class = false,
            '|' if !class => out.push_str(EMPTY),
            '(' if !class => {
                if chars.get(i) == Some(&'?') {
                    // Copy the group's prefix: `?:`, `?=`, `?!`, `?<=`, `?<!`
                    // or `?<name>`.
                    out.push('?');
                    i += 1;
                    if chars[i] == '<' && !matches!(chars[i + 1], '=' | '!') {
                        while chars[i] != '>' {
                            out.push(chars[i]);
                            i += 1;
                        }
                        out.push('>');
                        i += 1;
                    } else if chars[i] == '<' {
                        out.push('<');
                        out.push(chars[i + 1]);
                        i += 2;
                    } else {
                        out.push(chars[i]);
                        i += 1;
                    }
                }
                out.push_str(EMPTY);
            }
            _ => {}
        }
    }
    out.push_str(EMPTY);
    out.push(')');
    out
}

const PATTERNS: &[(&str, &str)] = &[
    ("ab|cd|ef|gh", ""),
    ("a|b|c|d|e|f|g|h|i|j|k", ""),
    ("(a)|(b)|(c)", ""),
    ("(a)|b(c)|(d)e", "d"),
    ("x|xy|xyz", ""),
    ("xyz|xy|x", ""),
    ("[0-9]+|[a-z]+|_", ""),
    (
        "[#*0-9]\\uFE0F?\\u20E3|[\\xA9\\xAE]\\uFE0F?|\\uD83C[\\uDDE6-\\uDDFF]|\\u26D3",
        "g",
    ),
    (
        "[#*0-9]\\uFE0F?\\u20E3|[\\xA9\\xAE]\\uFE0F?|\\u{1F1E6}|\\u26D3",
        "u",
    ),
    ("K|s|é|\\u212A", "i"),
    ("K|s|é|\\u212A", "iu"),
    ("k|\\u017F|x", "iu"),
    ("\\u212A|q", "iu"),
    ("(?<=a|bc)d|e", ""),
    ("(?<=x|[0-9]y)z", ""),
    ("(?<!a|b)c|d", ""),
    ("(?=a|b)\\w|c", ""),
    ("^(?:foo|bar)$|baz", "m"),
    ("\\bfoo|bar\\b|\\Bq", ""),
    ("(?:a|)b", ""),
    ("(?:|a)b", ""),
    ("a|$", ""),
    ("$|a", ""),
    ("(?:a|b)*c|d", ""),
    ("(?:ab|a)(?:bc|c)", ""),
    ("(a|ab)(c|bcd)(d*)", ""),
    ("(?<n>a)|(?<n>b)", ""),
    ("(a)\\1|b", ""),
    ("[^a]|a", ""),
    ("\\p{Lu}|[a-c]", "u"),
    ("\\d|\\s|\\w", ""),
    ("é|中|😀|a", "u"),
    ("é|中|😀|a", ""),
    ("(?:x|y|z){2,3}w|v", ""),
    ("(?:(a)|(b))+", ""),
    ("token|tok|t", "y"),
];

const SUBJECTS: &[&str] = &[
    "",
    "a",
    "b",
    "e",
    "z",
    "1",
    "#",
    "#\u{20E3}",
    "1\u{FE0F}\u{20E3}",
    "\u{A9}",
    "\u{1F1E6}",
    "\u{26D3}",
    "k",
    "K",
    "\u{212A}",
    "s",
    "\u{17F}",
    "S",
    "é",
    "É",
    "中",
    "😀",
    "abcd",
    "bcd",
    "xyz",
    "xy",
    "x",
    "cd ef gh",
    "__9",
    "foo",
    "bar",
    "baz",
    "foo\nbar",
    "ad",
    "bcd",
    "ac",
    "abbcd",
    "aa",
    "ba",
    "xyzw",
    "xyxw",
    "v",
    "token",
    "tokenx",
    "ab",
    " ",
    "-",
    "1a_",
];

#[test]
fn branches_answer_as_trying_every_arm() {
    let mut filtered = 0;
    for &(source, flags) in PATTERNS {
        let w = words(source, flags);
        let plain = words(&unfiltered(source), flags);
        assert_eq!(branches(&plain), 0, "/{}/", unfiltered(source));
        filtered += branches(&w);
        for subject in SUBJECTS {
            let units = subject.encode_utf16().count();
            for start in 0..=units + 1 {
                assert_eq!(
                    run(&w, subject, start, usize::MAX),
                    run(&plain, subject, start, usize::MAX),
                    "/{source}/{flags} on {subject:?} from {start}"
                );
            }
        }
    }
    assert!(filtered > 30, "{filtered} filtered branches");
}

#[test]
fn alternation_is_flat_and_ordered() {
    // One frame live per attempted alternative and one jump to the end, in
    // source order.
    let w = words("ab|cd|ef|gh", "");
    let at = |pc: usize| [w[11 + pc * 3], w[12 + pc * 3], w[13 + pc * 3]];
    assert_eq!(at(1)[0], 31);
    assert_eq!(at(2), [1, 'a' as u32, 0]);
    assert_eq!(at(4), [6, 15, 0]);
    assert_eq!(at(12), [6, 15, 0]);
    assert_eq!(at(1)[2], 5);
    assert_eq!(at(5)[0], 31);
    assert_eq!(at(6), [1, 'c' as u32, 0]);
    assert_eq!(at(13), [1, 'g' as u32, 0]);
    // Leftmost alternative wins even when a later one is longer.
    let w = words("x|xy|xyz", "");
    assert_eq!(
        run(&w, "xyz", 0, usize::MAX),
        Ok(Some(vec![Span::new(0, 1)]))
    );
}

#[test]
fn repeat_registers_need_no_clearing() {
    // Repeat counters start as whatever the host's scratch held; every repeat
    // writes its own before reading them, so answers do not depend on it.
    for (source, flags) in [
        ("(?:ab){2,3}c", ""),
        ("a{1,3}?b", ""),
        ("(?:(a)|b){2,}?(c)", ""),
        ("(?:x(?:y{0,2}z)+)*w", ""),
        ("(a*)+b", ""),
        ("\\uFE0F?\\u20E3|[0-9]\\uFE0F?", ""),
        ("((a)|b)+", "d"),
        ("(?:a?){3,5}", ""),
    ] {
        let w = words(source, flags);
        for subject in [
            "",
            "abababc",
            "aab",
            "aaab",
            "bac",
            "acbc",
            "xyyzxzw",
            "xzw",
            "w",
            "aaaab",
            "b",
            "1\u{FE0F}",
            "\u{20E3}",
            "abba",
            "aaaaaa",
        ] {
            let expected = run(&w, subject, 0, usize::MAX);
            for garbage in [0, 1, 2, 3, 7, 1000, usize::MAX - 1] {
                assert_eq!(
                    run(&w, subject, 0, garbage),
                    expected,
                    "/{source}/ on {subject:?} with {garbage}"
                );
            }
        }
    }
}

#[test]
fn validation_rederives_masks_and_the_anchored_claim() {
    let w = words("ab|cd|[0-9]x", "");
    assert!(Program::from_words(&w, &mut Budget::new(100_000)).is_ok());
    let first = (0..w[4] as usize).find(|pc| w[11 + pc * 3] == 31).unwrap();
    for bit in 0..32 {
        let mut bad = w.clone();
        bad[12 + first * 3] ^= 1 << bit;
        assert_eq!(
            Program::from_words(&bad, &mut Budget::new(100_000)).unwrap_err(),
            ProgramError::Invalid,
            "bit {bit}"
        );
    }
    // A set every character is in is not a claim; such a branch is a `SPLIT`.
    let mut bad = w.clone();
    bad[12 + first * 3] = u32::MAX;
    assert!(Program::from_words(&bad, &mut Budget::new(100_000)).is_err());

    let anchored = words("^a|^b", "");
    let free = words("^a|b", "");
    assert_ne!(anchored[7] & (1 << 24), 0);
    assert_eq!(free[7] & (1 << 24), 0);
    for (program, flip) in [(anchored, true), (free, false)] {
        let mut bad = program.clone();
        bad[7] ^= 1 << 24;
        assert_eq!(
            Program::from_words(&bad, &mut Budget::new(100_000)).unwrap_err(),
            ProgramError::Invalid,
            "anchored {flip}"
        );
        let mut reserved = program.clone();
        reserved[7] |= 1 << 25;
        assert!(Program::from_words(&reserved, &mut Budget::new(100_000)).is_err());
    }
}

#[test]
fn anchored_claim_still_limits_starts() {
    let w = words("^(?:ab|cd)", "");
    assert_eq!(run(&w, "xab", 0, usize::MAX), Ok(None));
    assert_eq!(
        run(&w, "cdab", 0, usize::MAX),
        Ok(Some(vec![Span::new(0, 2)]))
    );
    let w = words("^(?:ab|cd)", "m");
    assert_eq!(
        run(&w, "x\ncd", 0, usize::MAX),
        Ok(Some(vec![Span::new(2, 4)]))
    );
}
