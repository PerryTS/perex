//! Captures read off the automaton against the evaluator's: every group of
//! every match, from every start, on one-pass and ambiguous programs alike.
//! See `docs/dfa.md`, "Captures".
use perex::{
    Budget,
    compiler::{Node, Range, compile},
    dfa::{self, Decline, Found},
    executor::{Frame, Scratch, Undo, find},
    input::Input,
    program::Program,
    span::Span,
};

fn program(pattern: &str, flags: &str) -> Option<Vec<u32>> {
    let source = Input::utf8(pattern);
    let mut nodes = vec![Node::default(); source.len_utf16() * 4 + 64];
    let mut ranges = vec![Range::default(); source.len_utf16() * 12 + 64];
    let mut words = vec![0; source.len_utf16() * 48 + 256];
    compile(
        source,
        flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut Budget::new(10_000_000),
    )
    .ok()
    .map(|p| p.words().to_vec())
}

fn view(words: &[u32]) -> Program<'_> {
    Program::from_words(words, &mut Budget::new(10_000_000)).unwrap()
}

type Groups = Vec<Option<Span>>;

fn evaluator(words: &[u32], subject: &[u8], start: usize) -> Option<Groups> {
    let p = view(words);
    let input = Input::wtf8(subject).unwrap();
    let mut registers = vec![0; p.register_count()];
    let mut frames = vec![Frame::default(); 4096];
    let mut undo = vec![Undo::default(); 65536];
    let mut captures = vec![None; p.capture_count()];
    let found = find(
        p,
        input,
        start,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        &mut captures,
        &mut Budget::new(50_000_000),
    )
    .unwrap();
    found.then_some(captures)
}

#[derive(Default, Debug)]
struct Tally {
    answered: usize,
    declined: usize,
}

/// Compare `dfa::captures` with the evaluator at every start of every subject.
/// A match that begins at `begin` is searched anchored there.
fn check(pattern: &str, flags: &str, subjects: &[&str], cache_words: usize, tally: &mut Tally) {
    let Some(words) = program(pattern, flags) else {
        return;
    };
    let p = view(&words);
    if !dfa::eligible(p) {
        return;
    }
    let mut cache = vec![0u32; cache_words];
    for subject in subjects {
        let len = subject.encode_utf16().count();
        for start in 0..=len {
            let Some(expected) = evaluator(&words, subject.as_bytes(), start) else {
                continue;
            };
            let begin = expected[0].unwrap().start();
            let input = Input::wtf8(subject.as_bytes()).unwrap();
            let mut out = vec![None; p.capture_count()];
            let found = dfa::captures(
                p,
                input,
                begin,
                &mut cache,
                &mut Budget::new(50_000_000),
                &mut out,
            )
            .unwrap();
            match found {
                Found::Match(span) => {
                    assert_eq!(
                        Some(span),
                        expected[0],
                        "/{pattern}/{flags} on {subject:?} from {start}"
                    );
                    assert_eq!(
                        out, expected,
                        "/{pattern}/{flags} on {subject:?} from {start}"
                    );
                    tally.answered += 1;
                }
                Found::Declined(
                    Decline::Captures | Decline::Disabled | Decline::Storage { .. },
                ) => {
                    tally.declined += 1;
                }
                other => panic!("/{pattern}/{flags} on {subject:?} from {start}: {other:?}"),
            }
        }
    }
}

const SUBJECTS: &[&str] = &[
    "",
    "a",
    "ab",
    "abc abd",
    "aaa",
    "xaby ab",
    "abcd",
    "abcbcd",
    "a\nb\r\nc",
    "foo bar_baz 12",
    "2024-10-28T12:30:45 [info] user=42 action=login latency=17ms",
    "ababababa",
    "AbC aBc",
    "  x  ",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab",
    "é ß",
];

const CASES: &[(&str, &str)] = &[
    ("(a)", ""),
    ("(a)|b", ""),
    ("(?:(a)|b)+", ""),
    ("(?:(a)|(b))+", ""),
    ("(a*)*", ""),
    ("(a*)+", ""),
    ("(a|ab)(c|bcd)(d*)", ""),
    ("a(b)?c?", ""),
    ("(a)?(b)?", ""),
    ("(a+?)(b*)", ""),
    ("^(\\w+) (\\w+)$", ""),
    ("\\b(\\w)(\\w*)\\b", ""),
    ("((a)|(b))*", ""),
    ("(?:(a)|(b)){2}", ""),
    ("(a){2,3}", ""),
    ("(a(b)?)+", ""),
    ("(x)*y", ""),
    ("(a)\\b", ""),
    ("(a)$", "m"),
    ("(\\d+)-(\\d+)", ""),
    ("(a)(b)", "y"),
    ("(b)", "y"),
    ("(A)(b)", "i"),
    ("(.)(.)", "s"),
    (
        "^(\\d{4})-(\\d{2})-(\\d{2})T[\\d:]+ \\[(\\w+)\\] user=(\\d+) action=(\\w+) latency=(\\d+)ms$",
        "",
    ),
    (
        "(.)(.)(.)(.)(.)(.)(.)(.)(.)(.)(.)(.)(.)(.)(.)(.)(.)(.)(.)(.)",
        "",
    ),
    (
        "(a)?(a)?(a)?(a)?(a)?(a)?(a)?(a)?(a)?(a)?(a)?(a)?(a)?(a)?(a)?(a)?(a)?(a)?",
        "",
    ),
    ("(?:a|(b))*c|(d)", ""),
    ("(a*?)(a*)", ""),
    ("(?:(a)|b)*?c", ""),
];

#[test]
fn captures_agree_on_every_case() {
    let mut tally = Tally::default();
    for &(pattern, flags) in CASES {
        check(pattern, flags, SUBJECTS, 1 << 16, &mut tally);
    }
    assert!(tally.answered > 200, "{tally:?}");
}

/// A cache too small to hold the automaton clears while a capture search runs;
/// registers and answers survive the clears.
#[test]
fn captures_survive_clears() {
    let mut tally = Tally::default();
    for &(pattern, flags) in CASES {
        let Some(words) = program(pattern, flags) else {
            continue;
        };
        let Some(minimum) = dfa::minimum_words(view(&words)) else {
            continue;
        };
        for extra in [0, 64, 300] {
            check(pattern, flags, SUBJECTS, minimum + extra, &mut tally);
        }
    }
    assert!(tally.answered > 100, "{tally:?}");
}

/// Programs whose transitions keep two paths alive are declined, not answered.
#[test]
fn ambiguous_programs_decline() {
    for (pattern, subject) in [("(a|ab)(c|bcd)(d*)", "abcd"), ("(a*)(a*)b", "aaab")] {
        let words = program(pattern, "").unwrap();
        let p = view(&words);
        let mut cache = vec![0u32; 1 << 16];
        let mut out = vec![None; p.capture_count()];
        let input = Input::utf8(subject);
        let found =
            dfa::captures(p, input, 0, &mut cache, &mut Budget::new(1 << 20), &mut out).unwrap();
        assert_eq!(found, Found::Declined(Decline::Captures), "/{pattern}/");
        assert!(out.iter().all(Option::is_none), "out untouched");
    }
}

fn random_groups(seed: u64, rounds: usize) -> Vec<(String, &'static str)> {
    let atoms = [
        "a", "b", "c", ".", "[ab]", "[^b]", "\\w", "\\W", "\\b", "\\s", "\\d", "[a-z]", "\\n", "x",
    ];
    let quantifiers = [
        "", "", "", "*", "+", "?", "*?", "+?", "??", "{2}", "{1,2}", "{0,3}?", "{2,}",
    ];
    let flag_sets = ["", "i", "m", "s", "y", "my"];
    let mut x = seed;
    let mut next = |n: usize| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x % n as u64) as usize
    };
    let mut result = Vec::new();
    for _ in 0..rounds {
        let mut pattern = String::new();
        let alternatives = 1 + next(3);
        for alternative in 0..alternatives {
            if alternative > 0 {
                pattern.push('|');
            }
            for _ in 0..1 + next(4) {
                let capture = next(3) != 0;
                let nested = next(6) == 0;
                if capture {
                    pattern.push('(');
                }
                if nested {
                    pattern.push_str("(?:");
                    pattern.push_str(atoms[next(atoms.len())]);
                    pattern.push('|');
                    pattern.push_str(atoms[next(atoms.len())]);
                    pattern.push(')');
                } else {
                    pattern.push_str(atoms[next(atoms.len())]);
                }
                if capture {
                    pattern.push(')');
                }
                let q = quantifiers[next(quantifiers.len())];
                if !q.is_empty() && !capture && pattern.ends_with("\\b") {
                    continue;
                }
                pattern.push_str(q);
            }
        }
        result.push((pattern, flag_sets[next(flag_sets.len())]));
    }
    result
}

#[test]
fn random_patterns_with_groups_agree_with_the_evaluator() {
    let subjects = [
        "",
        "abc",
        "aab bca\ncab",
        "x1_2 abx",
        "bbbbaaaa",
        "abababcabc",
        "a b c  ab",
    ];
    let mut tally = Tally::default();
    for (pattern, flags) in random_groups(0x9e37_79b9_7f4a_7c15, 4000) {
        check(&pattern, flags, &subjects, 1 << 15, &mut tally);
    }
    assert!(tally.answered > 2000, "{tally:?}");
    eprintln!("random capture patterns: {tally:?}");
}

/// A cache resized to the `required_words` a thrashing program reported does
/// not clear on the same subjects.
#[test]
fn required_words_is_enough() {
    let pattern = "^(\\d{4})-(\\d{2})-(\\d{2})T[\\d:]+ \\[(\\w+)\\] user=(\\d+) action=(\\w+) latency=(\\d+)ms$";
    let words = program(pattern, "").unwrap();
    let p = view(&words);
    let subjects: Vec<String> = (0..200)
        .map(|k| {
            format!(
                "20{:02}-{:02}-{:02}T{:02}:{:02}:{:02} [{}] user={} action={} latency={}ms",
                10 + k % 15,
                1 + k % 12,
                1 + k % 28,
                k % 24,
                k % 60,
                (k * 7) % 60,
                ["info", "warn", "error", "debug"][k % 4],
                k * 31,
                ["login", "logout", "view", "buy_item"][k % 4],
                k * 3
            )
        })
        .collect();
    let minimum = dfa::minimum_words(p).unwrap();
    let mut cache = vec![0u32; minimum + 256];
    let mut out = vec![None; p.capture_count()];
    for _ in 0..20 {
        for subject in &subjects {
            let input = Input::utf8(subject);
            let _ = dfa::captures(p, input, 0, &mut cache, &mut Budget::new(1 << 24), &mut out)
                .unwrap();
        }
    }
    // A thrashing cache reports a size; the host reallocates, resets and
    // searches again, and each report is larger than the storage it follows.
    let mut grows = 0;
    loop {
        let stats = dfa::stats(&cache).unwrap();
        if !stats.disabled {
            break;
        }
        assert!(stats.required_words > cache.len(), "{stats:?}");
        cache = vec![0u32; stats.required_words];
        grows += 1;
        assert!(grows <= 8, "{stats:?}");
        for _ in 0..20 {
            for subject in &subjects {
                let input = Input::utf8(subject);
                let _ = dfa::captures(p, input, 0, &mut cache, &mut Budget::new(1 << 24), &mut out)
                    .unwrap();
            }
        }
    }
    assert!(grows >= 1);
    // The grown cache holds the automaton: a fresh pass never clears.
    dfa::reset(&mut cache);
    for subject in &subjects {
        let input = Input::utf8(subject);
        let found =
            dfa::captures(p, input, 0, &mut cache, &mut Budget::new(1 << 24), &mut out).unwrap();
        assert!(matches!(found, Found::Match(_)), "{found:?}");
    }
    let stats = dfa::stats(&cache).unwrap();
    assert_eq!(stats.clears, 0, "{stats:?}");
}
