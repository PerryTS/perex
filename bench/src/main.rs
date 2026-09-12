//! Local cross-engine comparison for Perex.
//!
//! Authored, self-contained cases. Same iteration protocol for every engine:
//! compile once outside the timer, then run the warm search loop with reused
//! scratch. Reports ns/iteration and the match outcome so a fast wrong answer
//! is visible as a correctness failure rather than a speed result.

use std::hint::black_box;
use std::mem::size_of;

/// Process CPU time in nanoseconds. Wall clock on a loaded machine measures
/// how often the scheduler ran us, not how much work the engine did; CPU time
/// is what the comparison is about.
fn cpu_nanos() -> u128 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `t` is a valid timespec for the duration of the call.
    unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut t) };
    t.tv_sec as u128 * 1_000_000_000 + t.tv_nsec as u128
}

use perex::compiler::{Node, Range, compile};
use perex::executor::{Frame, Scratch, Undo, find};
use perex::input::Input;
use perex::span::Span;
use perex::Budget;

struct Case {
    id: &'static str,
    pattern: String,
    flags: &'static str,
    subject: String,
    /// Expected: does a match exist anywhere in the subject?
    expect: bool,
    iters: usize,
}

fn cases() -> Vec<Case> {
    let mut v = Vec::new();

    // 1. Short literal hit. Setup cost dominates; this is the known 3x loss.
    v.push(Case {
        id: "short-literal",
        pattern: "needle".into(),
        flags: "",
        subject: "haystack with a needle inside".into(),
        expect: true,
        iters: 200_000,
    });

    // 2. Long literal hit near the end of a ~256 KiB subject. Scan throughput.
    let mut long = "abcdefghijklmnopqrstuvwxyz0123456789".repeat(7281);
    long.push_str("needle");
    v.push(Case {
        id: "long-literal",
        pattern: "needle".into(),
        flags: "",
        subject: long.clone(),
        expect: true,
        iters: 2_000,
    });

    // 3. Long literal miss over the same subject: full scan, no match.
    v.push(Case {
        id: "long-literal-miss",
        pattern: "zzneedlezz".into(),
        flags: "",
        subject: long.clone(),
        expect: false,
        iters: 2_000,
    });

    // 4. Lookbehind over a long subject.
    v.push(Case {
        id: "long-lookbehind",
        pattern: "(?<=0123456789)abc".into(),
        flags: "",
        subject: long.clone(),
        expect: true,
        iters: 2_000,
    });

    // 5. The known algorithmic weakness: the pattern's required text ("END")
    //    appears in the subject, but *before* the prefix that must consume its
    //    way there, so required-text admission accepts and every start position
    //    backtracks through the run of 'a'.
    let mut suffix_before = String::from("END");
    suffix_before.push_str(&"a".repeat(4_000));
    v.push(Case {
        id: "required-suffix-before",
        pattern: "a+END".into(),
        flags: "",
        subject: suffix_before,
        expect: false,
        iters: 5,
    });

    // 6. Required text absent entirely: admission should reject immediately.
    let mut suffix_absent = String::from("");
    suffix_absent.push_str(&"a".repeat(4_000));
    v.push(Case {
        id: "required-suffix-absent",
        pattern: "a+END".into(),
        flags: "",
        subject: suffix_absent,
        expect: false,
        iters: 20,
    });

    // 7. Alternation over a long subject.
    v.push(Case {
        id: "long-choice",
        pattern: "(?:needle|thimble|haystack)".into(),
        flags: "",
        subject: long.clone(),
        expect: true,
        iters: 2_000,
    });

    // 8. Character classes, long subject, miss.
    v.push(Case {
        id: "long-classes-miss",
        pattern: "[A-Z]{4}[0-9]{4}".into(),
        flags: "",
        subject: long.clone(),
        expect: false,
        iters: 500,
    });

    // 9. Short class-based hit (short-operation overhead with a class).
    v.push(Case {
        id: "short-classes",
        pattern: "[a-z]+[0-9]+".into(),
        flags: "",
        subject: "value abc123 end".into(),
        expect: true,
        iters: 200_000,
    });

    // 10. Capture groups on a short subject.
    v.push(Case {
        id: "short-captures",
        pattern: "(\\w+)@(\\w+)\\.com".into(),
        flags: "",
        subject: "mail user@example.com now".into(),
        expect: true,
        iters: 100_000,
    });

    // 11. Case-insensitive short hit.
    v.push(Case {
        id: "short-fold",
        pattern: "NeEdLe".into(),
        flags: "i",
        subject: "haystack with a needle inside".into(),
        expect: true,
        iters: 100_000,
    });

    // Diagnostics: isolate per-character cost by class complexity. Same
    // subject and match shape, differing only in how many ranges a class holds.
    let run = "a".repeat(60) + "!";
    for (id, pattern) in [
        ("diag-char-repeat", "a+!"),
        ("diag-class1-repeat", "[a-z]+!"),
        ("diag-class4-repeat", r"\w+!"),
        ("diag-classneg-repeat", "[^0-9]+!"),
    ] {
        v.push(Case {
            id,
            pattern: pattern.into(),
            flags: "",
            subject: run.clone(),
            expect: true,
            iters: 20_000,
        });
    }

    // Fixed versus marginal cost: identical phase sequence, increasing
    // instruction count, always matching at position zero.
    for (id, n) in [("diag-lit1", 1usize), ("diag-lit2", 2), ("diag-lit4", 4), ("diag-lit8", 8), ("diag-lit16", 16)] {
        v.push(Case {
            id,
            pattern: "a".repeat(n),
            flags: "",
            subject: "a".repeat(n),
            expect: true,
            iters: 200_000,
        });
    }

    // Entry cost: how much a search costs before it does anything useful.
    for (id, pattern, subject, expect) in [
        ("diag-empty-subject", "z", "", false),
        ("diag-one-miss", "z", "a", false),
        ("diag-one-hit", "a", "a", true),
        ("diag-ten-miss", "z", "aaaaaaaaaa", false),
    ] {
        v.push(Case {
            id,
            pattern: pattern.into(),
            flags: "",
            subject: subject.into(),
            expect,
            iters: 300_000,
        });
    }

    // 12. Anchored end miss over a long subject (strict end candidate path).
    v.push(Case {
        id: "long-anchored-hit",
        pattern: "needle$".into(),
        flags: "",
        subject: long.clone(),
        expect: true,
        iters: 20_000,
    });

    v
}

/// Median of `ROUNDS` timed rounds. A single round on a shared machine is too
/// noisy to compare engines; the median keeps one descheduled round from
/// deciding the result.
const ROUNDS: usize = 5;

fn time<F: FnMut() -> bool>(iters: usize, mut f: F) -> (f64, bool) {
    let mut last = false;
    for _ in 0..(iters / 20).max(1) {
        last = f();
    }
    let mut rounds = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let start = cpu_nanos();
        for _ in 0..iters {
            last = black_box(f());
        }
        rounds.push((cpu_nanos() - start) as f64 / iters as f64);
    }
    rounds.sort_by(f64::total_cmp);
    (rounds[ROUNDS / 2], last)
}

/// The driver's own per-iteration cost: the same budget and scratch handles
/// built per call, with no matching. Subtracting this separates the engine's
/// cost from the harness's.
fn run_null(c: &Case) -> Option<(f64, bool)> {
    let mut registers = vec![0usize; 4];
    let mut frames = vec![Frame::default(); 16];
    let mut undo = vec![Undo::default(); 16];
    let mut captures = vec![None::<Span>; 2];
    let subject = Input::utf8(&c.subject);
    let (ns, _) = time(c.iters, || {
        let budget = Budget::new(1_000_000_000);
        let scratch = Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        };
        black_box(&scratch);
        black_box(&budget);
        black_box(&mut captures);
        black_box(subject).len_utf16() == usize::MAX
    });
    Some((ns, c.expect))
}

fn run_perex(c: &Case) -> Option<(f64, bool)> {
    let pattern = Input::utf8(&c.pattern);
    let subject = Input::utf8(&c.subject);
    let mut nodes = vec![Node::default(); pattern.len_utf16() * 3 + 32];
    let mut ranges = vec![Range::default(); pattern.len_utf16() * 12 + 32];
    let mut words = vec![0u32; pattern.len_utf16() * 48 + 128];
    let mut budget = Budget::new(10_000_000);
    let p = compile(
        pattern,
        c.flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut budget,
    )
    .ok()?;
    let mut registers = vec![0; p.register_count()];
    let mut frames = vec![Frame::default(); 16384];
    let mut undo = vec![Undo::default(); 131072];
    let mut captures = vec![None; p.capture_count()];
    let (ns, found) = time(c.iters, || {
        let mut budget = Budget::new(1_000_000_000);
        find(
            black_box(p),
            black_box(subject),
            0,
            Scratch {
                registers: &mut registers,
                frames: &mut frames,
                undo: &mut undo,
            },
            &mut captures,
            &mut budget,
        )
        .unwrap_or(false)
    });
    Some((ns, found))
}

fn run_regress(c: &Case) -> Option<(f64, bool)> {
    let re = regress::Regex::with_flags(&c.pattern, c.flags).ok()?;
    let s = c.subject.as_str();
    let (ns, found) = time(c.iters, || black_box(re.find(black_box(s))).is_some());
    Some((ns, found))
}

fn run_regex(c: &Case) -> Option<(f64, bool)> {
    if c.flags.contains('i') {
        let re = regex::Regex::new(&format!("(?i){}", c.pattern)).ok()?;
        let s = c.subject.as_str();
        let (ns, found) = time(c.iters, || black_box(re.find(black_box(s))).is_some());
        return Some((ns, found));
    }
    let re = regex::Regex::new(&c.pattern).ok()?;
    let s = c.subject.as_str();
    let (ns, found) = time(c.iters, || black_box(re.find(black_box(s))).is_some());
    Some((ns, found))
}

fn run_fancy(c: &Case) -> Option<(f64, bool)> {
    let pat = if c.flags.contains('i') {
        format!("(?i){}", c.pattern)
    } else {
        c.pattern.clone()
    };
    let re = fancy_regex::Regex::new(&pat).ok()?;
    let s = c.subject.as_str();
    let (ns, found) = time(c.iters, || {
        black_box(re.find(black_box(s))).ok().flatten().is_some()
    });
    Some((ns, found))
}

fn main() {
    if std::env::args().any(|a| a == "--emit-cases") {
        // The same cases, for drivers outside this process.
        println!("[");
        let all = cases();
        for (i, c) in all.iter().enumerate() {
            println!(
                "  {{\"id\":{:?},\"pattern\":{:?},\"flags\":{:?},\"subject\":{:?},\"expect\":{},\"iters\":{}}}{}",
                c.id, c.pattern, c.flags, c.subject, c.expect, c.iters,
                if i + 1 == all.len() { "" } else { "," }
            );
        }
        println!("]");
        return;
    }
    let filter = std::env::args().nth(1);
    println!(
        "{:<26} {:>12} {:>12} {:>12} {:>12}   {:>7}",
        "case", "perex ns", "regress ns", "regex ns", "fancy ns", "vs best"
    );
    println!("{}", "-".repeat(96));
    let mut losses = Vec::new();
    for c in cases() {
        if let Some(f) = &filter {
            if !c.id.contains(f.as_str()) {
                continue;
            }
        }
        let px = run_perex(&c);
        if c.id.starts_with("diag-empty") || c.id.starts_with("diag-one") {
            if let Some((null_ns, _)) = run_null(&c) {
                println!("  {:<24} driver-only scaffolding: {:.1} ns", c.id, null_ns);
            }
        }
        let rg = run_regress(&c);
        let rx = run_regex(&c);
        let fy = run_fancy(&c);

        // Correctness screen: a fast wrong answer is a failure, not a result.
        for (name, r) in [
            ("perex", &px),
            ("regress", &rg),
            ("regex", &rx),
            ("fancy", &fy),
        ] {
            if let Some((_, found)) = r {
                if *found != c.expect {
                    println!("  !! {} answered {} on {}, expected {}", name, found, c.id, c.expect);
                }
            }
        }

        let fmt = |r: &Option<(f64, bool)>| match r {
            Some((ns, _)) => format!("{:.1}", ns),
            None => "-".into(),
        };
        let best = [&rg, &rx, &fy]
            .iter()
            .filter_map(|r| r.map(|(ns, _)| ns))
            .fold(f64::INFINITY, f64::min);
        let ratio = match px {
            Some((ns, _)) if best.is_finite() => {
                let r = ns / best;
                if r > 1.10 {
                    losses.push((c.id, r));
                }
                format!("{:.2}x", r)
            }
            _ => "-".into(),
        };
        println!(
            "{:<26} {:>12} {:>12} {:>12} {:>12}   {:>7}",
            c.id,
            fmt(&px),
            fmt(&rg),
            fmt(&rx),
            fmt(&fy),
            ratio
        );
    }
    if !losses.is_empty() {
        println!("\nperex slower than the best comparator by >10% in {} case(s):", losses.len());
        losses.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        for (id, r) in &losses {
            println!("  {:<26} {:.2}x", id, r);
        }
    }
    let _ = size_of::<Frame>();
}
