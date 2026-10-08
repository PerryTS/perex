//! The lazy automaton: eligibility, answers against the evaluator, the cache's
//! cap, clearing, thrashing and relocation. See `docs/dfa.md`.
use perex::{
    Budget,
    compiler::{Node, Range, compile},
    dfa::{self, Decline, Found, Tested},
    executor::{Frame, Scratch, Undo, find, is_match},
    input::Input,
    program::Program,
    span::Span,
};

fn program(pattern: &str, flags: &str) -> Vec<u32> {
    let source = Input::utf8(pattern);
    let mut nodes = vec![Node::default(); source.len_utf16() * 4 + 64];
    let mut ranges = vec![Range::default(); source.len_utf16() * 12 + 64];
    let mut words = vec![0; source.len_utf16() * 48 + 256];
    let p = compile(
        source,
        flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut Budget::new(10_000_000),
    )
    .unwrap_or_else(|e| panic!("/{pattern}/{flags}: {e:?}"));
    p.words().to_vec()
}

fn view(words: &[u32]) -> Program<'_> {
    Program::from_words(words, &mut Budget::new(10_000_000)).unwrap()
}

/// The evaluator's answer: group zero.
fn evaluator(words: &[u32], subject: &[u8], start: usize) -> Option<Span> {
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
    let tested = is_match(
        p,
        input,
        start,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        &mut Budget::new(50_000_000),
    )
    .unwrap();
    assert_eq!(found, tested);
    found.then(|| captures[0].unwrap())
}

/// The automaton's answers, both of them, checked against each other.
fn automaton(words: &[u32], subject: &[u8], start: usize, cache: &mut [u32]) -> Option<Span> {
    let p = view(words);
    let input = Input::wtf8(subject).unwrap();
    let found = dfa::find(p, input, start, cache, &mut Budget::new(50_000_000)).unwrap();
    let tested = dfa::is_match(p, input, start, cache, &mut Budget::new(50_000_000)).unwrap();
    match (found, tested) {
        (Found::Match(span), Tested::Match) => Some(span),
        (Found::NoMatch, Tested::NoMatch) => None,
        other => panic!("{other:?}"),
    }
}

fn agree(pattern: &str, flags: &str, subjects: &[&str]) {
    let words = program(pattern, flags);
    assert!(dfa::eligible(view(&words)), "/{pattern}/{flags} ineligible");
    let mut cache = vec![0u32; 1 << 16];
    for subject in subjects {
        let len = subject.encode_utf16().count();
        for start in 0..=len + 1 {
            let expected = evaluator(&words, subject.as_bytes(), start);
            let actual = automaton(&words, subject.as_bytes(), start, &mut cache);
            assert_eq!(
                actual, expected,
                "/{pattern}/{flags} on {subject:?} from {start}"
            );
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
    "AbC",
    "a\nb\r\nc",
    "foo bar_baz 12",
    "\u{2028}x\u{2029}",
    "é ß ſ K k",
    "😀a😀",
    "\u{1F600}\u{1F601}",
    "ababababa",
    "cdcdgh",
    "  x  ",
];

#[test]
fn literals_classes_and_alternation() {
    for (pattern, flags) in [
        ("a", ""),
        ("ab|cd|ef|gh", ""),
        ("[a-c]+", ""),
        ("[^a]", ""),
        ("a.c", ""),
        ("a.c", "s"),
        ("(a|ab)(c|bcd)", ""),
        ("a|ab", ""),
        ("ab|a", ""),
        ("x*", ""),
        ("x*?", ""),
        ("(?:ab)*", ""),
        ("(?:a|b)*?b", ""),
        ("[😀-😁]", "u"),
        (".", "u"),
        (".", ""),
        ("\\p{L}+", "u"),
        ("\\P{L}", "u"),
        ("\\p{L}[a-z]", "v"),
        ("k", "iu"),
        ("[k-l]", "iu"),
        ("ß", "i"),
        ("[a-z]+", "i"),
        ("\\w+", "iu"),
        ("\\p{Lu}", "iu"),
        ("\\p{Lu}+", "i"),
        ("[^\\p{Ll}]", "iu"),
        ("\\P{Ll}", "iu"),
        ("[\\p{Lu}\\d]", "iu"),
    ] {
        agree(pattern, flags, SUBJECTS);
    }
}

#[test]
fn assertions() {
    for (pattern, flags) in [
        ("^a", ""),
        ("^a", "m"),
        ("b$", ""),
        ("b$", "m"),
        ("^$", "m"),
        ("\\b", ""),
        ("\\B", ""),
        ("\\ba\\w*\\b", ""),
        ("\\bk", "iu"),
        ("\\b\\w", "iu"),
        ("(?:^|\\s)x", ""),
        ("$|a", ""),
        ("^\\p{Default_Ignorable_Code_Point}$", "u"),
    ] {
        agree(pattern, flags, SUBJECTS);
    }
}

#[test]
fn repeats_count_and_check_empty_iterations() {
    for (pattern, flags) in [
        ("a{2}", ""),
        ("a{2,3}", ""),
        ("a{2,}", ""),
        ("a{0,2}?b", ""),
        ("(?:a|b){2,4}", ""),
        ("(?:ab|a){1,3}?", ""),
        ("(a*)*b", ""),
        ("(a*)+", ""),
        ("(?:a?)*", ""),
        ("(?:a?){2}", ""),
        ("(?:a?){2,}?", ""),
        ("(?:\\b|a)*", ""),
        ("(?:^)*a", ""),
        ("(?:$|a)+", ""),
        ("(?:(?:a|)b?)*c", ""),
        ("(?:a{0,2}){2}x", ""),
        ("((?:a|b)*?)(?:c|d)", ""),
        ("(?:x?)*?y", ""),
    ] {
        agree(pattern, flags, SUBJECTS);
    }
}

#[test]
fn sticky_and_anchored_search_one_start() {
    for (pattern, flags) in [("a", "y"), ("b|ab", "y"), ("^a", ""), (".", "uy")] {
        agree(pattern, flags, SUBJECTS);
    }
}

#[test]
fn ineligible_programs_are_declined() {
    for (pattern, flags) in [
        ("(a)\\1", ""),
        ("a(?=b)", ""),
        ("(?<=a)b", ""),
        ("\\p{RGI_Emoji}", "v"),
        ("a{2000}", ""),
    ] {
        let words = program(pattern, flags);
        let p = view(&words);
        assert!(!dfa::eligible(p), "/{pattern}/{flags}");
        assert_eq!(dfa::minimum_words(p), None);
        let mut cache = vec![0; 1 << 12];
        assert_eq!(
            dfa::is_match(p, Input::utf8("ab"), 0, &mut cache, &mut Budget::new(1000)),
            Ok(Tested::Declined(Decline::Ineligible))
        );
    }
}

#[test]
fn eligibility_is_validated_both_ways() {
    let mut words = program("ab|c", "");
    assert_ne!(words[7] & (1 << 25), 0);
    words[7] &= !(1 << 25);
    assert!(Program::from_words(&words, &mut Budget::new(1_000_000)).is_err());
    let mut words = program("(a)\\1", "");
    words[7] |= 1 << 25;
    assert!(Program::from_words(&words, &mut Budget::new(1_000_000)).is_err());
}

#[test]
fn storage_below_the_minimum_is_declined() {
    let words = program("ab|cd", "");
    let p = view(&words);
    let minimum = dfa::minimum_words(p).unwrap();
    let mut small = vec![0; minimum - 1];
    assert_eq!(
        dfa::is_match(p, Input::utf8("cd"), 0, &mut small, &mut Budget::new(1000)),
        Ok(Tested::Declined(Decline::Storage {
            required_words: minimum
        }))
    );
    let mut exact = vec![0; minimum];
    assert_eq!(
        dfa::find(p, Input::utf8("xcd"), 0, &mut exact, &mut Budget::new(1000)),
        Ok(Found::Match(Span::new(1, 3).unwrap()))
    );
}

/// A pattern whose states are the last nine characters seen: 512 states over
/// a random subject, far more than a minimum cache holds.
fn many_states() -> Vec<u32> {
    program("a[ab]{8}c", "")
}

fn random_ab(len: usize, seed: u64) -> String {
    let mut x = seed;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            match x % 3 {
                0 => 'a',
                1 => 'b',
                _ => 'c',
            }
        })
        .collect()
}

#[test]
fn a_full_cache_clears_and_answers_stay_exact() {
    let words = many_states();
    let p = view(&words);
    let mut cache = vec![0; dfa::minimum_words(p).unwrap() + 2048];
    for seed in 1..40 {
        let subject = random_ab(400, seed);
        let expected = evaluator(&words, subject.as_bytes(), 0);
        let found = dfa::find(
            p,
            Input::utf8(&subject),
            0,
            &mut cache,
            &mut Budget::new(1 << 30),
        )
        .unwrap();
        match found {
            Found::Match(span) => assert_eq!(Some(span), expected),
            Found::NoMatch => assert_eq!(None, expected),
            Found::Declined(Decline::Disabled) => break,
            other => panic!("{other:?}"),
        }
    }
    let stats = dfa::stats(&cache).unwrap();
    assert!(stats.clears > 0, "{stats:?}");
    assert!(stats.peak_words <= cache.len());
}

#[test]
fn thrashing_disables_the_cache_until_reset() {
    let words = many_states();
    let p = view(&words);
    let minimum = dfa::minimum_words(p).unwrap();
    assert!(!dfa::stats(&[0; 4]).is_some_and(|s| s.disabled));
    let mut cache = vec![0; minimum];
    let subject = random_ab(20_000, 7).replace('c', "a");
    let mut disabled = false;
    for _ in 0..50 {
        match dfa::is_match(
            p,
            Input::utf8(&subject),
            0,
            &mut cache,
            &mut Budget::new(1 << 30),
        )
        .unwrap()
        {
            Tested::Declined(Decline::Disabled) => {
                disabled = true;
                break;
            }
            Tested::Match | Tested::NoMatch => {}
            other => panic!("{other:?}"),
        }
    }
    assert!(disabled, "{:?}", dfa::stats(&cache));
    assert!(dfa::stats(&cache).unwrap().disabled);
    dfa::reset(&mut cache);
    assert!(dfa::stats(&cache).is_none());
    assert!(!matches!(
        dfa::is_match(
            p,
            Input::utf8("abbbbbbbbc"),
            0,
            &mut cache,
            &mut Budget::new(1 << 20)
        ),
        Ok(Tested::Declined(_))
    ));
}

#[test]
fn the_cache_moves_between_calls() {
    let words = program("(?:\\bfoo|bar)+\\d{1,3}", "i");
    let mut cache = vec![0; 1 << 14];
    for (i, subject) in ["x FOObar12 y", "barfoo999", "foo", "nope bar1"]
        .iter()
        .enumerate()
    {
        let expected = evaluator(&words, subject.as_bytes(), 0);
        let actual = automaton(&words, subject.as_bytes(), 0, &mut cache);
        assert_eq!(actual, expected, "{subject}");
        let moved = cache.clone();
        cache.fill(0xdead_beef);
        cache = moved;
        if i == 2 {
            // A cache built for a different program is rebuilt, not reused.
            let other = program("x", "");
            assert_eq!(
                dfa::find(
                    view(&other),
                    Input::utf8("ax"),
                    0,
                    &mut cache,
                    &mut Budget::new(1000)
                ),
                Ok(Found::Match(Span::new(1, 2).unwrap()))
            );
        }
    }
}

/// Long subjects, searched again and again from several starts, so the
/// steady path runs with its skip to a byte a match can begin with.
#[test]
fn long_subjects_on_the_steady_path() {
    let text = "the quick brown fox jumps over the lazy dog 0123 456-7890 café naïve xyz ";
    let ascii: String = text.chars().filter(char::is_ascii).collect();
    let subjects = [
        ascii.repeat(4),
        text.repeat(3),
        format!("{}zq", "a".repeat(100)),
        format!("{}qz{}", "y".repeat(70), "x".repeat(40)),
        format!("{}😀Q", "0123456789 ".repeat(8)),
        format!("{}é😀Q", "0123456789 ".repeat(8)),
        // A match after two-, three- and four-byte characters, whose
        // position is counted only when the match is found.
        format!("😀é中{}😀 555-0199 ka", "the quick brown fox ".repeat(3)),
        // Folded letters outside ASCII after a long run.
        format!("{}ſK \u{212A}A KA a\u{212A}", "lorem ipsum ".repeat(6)),
        // Characters outside ASCII that a class admits, after a long run.
        format!("{}é5 ü7 z9", "lorem ipsum ".repeat(6)),
    ];
    for (pattern, flags) in [
        ("zq|qz|xj", ""),
        ("[x-z]", ""),
        ("\\d{3}-\\d{4}", ""),
        ("a[bc]|dog", ""),
        ("o\\w*", ""),
        ("é|z", "u"),
        ("[q-z]{2}", "i"),
        // The descriptor admits A-F; 😀 can begin a match too.
        ("(?:((?:😀|[A-F])))+Q", ""),
        ("(?:((?:😀|[A-F])))+Q", "u"),
        ("(?:é|[x-z])+", ""),
        // A leading pair over storage that is not ASCII: exact bytes, and a
        // folded run whose `k` also matches U+212A.
        ("ka|ab", ""),
        ("ka", "iu"),
        ("sk", "iu"),
        ("ak", "iu"),
        // A first character that may be non-ASCII keeps the skip from
        // passing over non-ASCII characters.
        ("[^a-z ]\\d", ""),
        ("[à-ÿ]\\d", ""),
        ("[x-zé]\\d", ""),
        ("\\p{L}\\d", "u"),
    ] {
        let words = program(pattern, flags);
        let mut cache = vec![0u32; 1 << 15];
        for subject in &subjects {
            let len = subject.encode_utf16().count();
            for start in [0, 1, 7, 33, len / 2, len] {
                for _ in 0..2 {
                    let expected = evaluator(&words, subject.as_bytes(), start);
                    let actual = automaton(&words, subject.as_bytes(), start, &mut cache);
                    assert_eq!(actual, expected, "/{pattern}/{flags} from {start}");
                }
            }
        }
    }
}

#[test]
fn an_exhausted_budget_is_an_error_not_an_answer() {
    let words = program("z", "");
    let p = view(&words);
    let mut cache = vec![0; 1 << 12];
    let subject = "a".repeat(1000);
    assert_eq!(
        dfa::is_match(
            p,
            Input::utf8(&subject),
            0,
            &mut cache,
            &mut Budget::new(100)
        ),
        Err(perex::executor::ExecError::WorkLimit)
    );
}

/// Random eligible patterns against the evaluator, over byte, ASCII and
/// surrogate-bearing subjects, from every start.
#[test]
fn random_patterns_agree_with_the_evaluator() {
    let atoms = [
        "a", "b", "c", ".", "[ab]", "[^b]", "\\w", "\\W", "\\b", "\\B", "^", "$", "\\s", "\\d",
        "k", "[a-z]", "😀", "\\n",
    ];
    let quantifiers = [
        "", "", "", "*", "+", "?", "*?", "+?", "??", "{2}", "{1,2}", "{0,3}?", "{2,}",
    ];
    let flag_sets = ["", "i", "m", "s", "u", "iu", "mu", "y", "my"];
    let subjects = [
        "",
        "abc",
        "aab bca\ncab",
        "Kk ſ 1_2",
        "😀a😀b\u{2028}",
        "bbbbaaaa",
    ];
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = |n: usize| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x % n as u64) as usize
    };
    let mut cache = vec![0u32; 1 << 15];
    for _ in 0..1500 {
        let mut pattern = String::new();
        let alternatives = 1 + next(3);
        for alternative in 0..alternatives {
            if alternative > 0 {
                pattern.push('|');
            }
            for _ in 0..1 + next(4) {
                if next(5) == 0 {
                    pattern.push_str("(?:");
                    pattern.push_str(atoms[next(atoms.len())]);
                    pattern.push('|');
                    pattern.push_str(atoms[next(atoms.len())]);
                    pattern.push_str(atoms[next(atoms.len())]);
                    pattern.push(')');
                } else {
                    pattern.push_str(atoms[next(atoms.len())]);
                }
                let q = quantifiers[next(quantifiers.len())];
                // `^`, `$` and `\b` take quantifiers only as groups.
                if !q.is_empty()
                    && (pattern.ends_with(['^', '$'])
                        || pattern.ends_with("\\b")
                        || pattern.ends_with("\\B"))
                {
                    continue;
                }
                pattern.push_str(q);
            }
        }
        let flags = flag_sets[next(flag_sets.len())];
        let source = Input::utf8(&pattern);
        let mut nodes = vec![Node::default(); source.len_utf16() * 4 + 64];
        let mut ranges = vec![Range::default(); source.len_utf16() * 12 + 64];
        let mut words = vec![0; source.len_utf16() * 48 + 256];
        let Ok(p) = compile(
            source,
            flags,
            &mut nodes,
            &mut ranges,
            &mut words,
            &mut Budget::new(1 << 24),
        ) else {
            continue;
        };
        let words = p.words().to_vec();
        if !dfa::eligible(view(&words)) {
            continue;
        }
        dfa::reset(&mut cache);
        for subject in subjects {
            let len = subject.encode_utf16().count();
            for start in 0..=len {
                let expected = evaluator(&words, subject.as_bytes(), start);
                let actual = automaton(&words, subject.as_bytes(), start, &mut cache);
                assert_eq!(
                    actual, expected,
                    "/{pattern}/{flags} on {subject:?} from {start}"
                );
            }
        }
    }
}
