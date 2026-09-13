//! Execute what Perex's compilation tier emits, and time it.
//!
//! Perex emits machine code into a buffer and never maps, protects or calls
//! anything. This does those three things, which is the host's half of the
//! split in `docs/compilation.md`: the unsafe is here, where a runtime already
//! manages memory, and `#![forbid(unsafe_code)]` stays on the engine.
//!
//! It checks every answer against the interpreter before timing anything. A
//! faster wrong answer is not a result.
use perex::compiler::{compile, Node, Range};
use perex::executor::{find, Frame, Scratch, Undo};
use perex::input::Input;
use perex::native::emit::{emit_search, supported, EXHAUSTED, NO_MATCH};
use perex::native::verify::Facts;
use perex::span::Span;
use perex::Budget;

/// Backward branches one call to generated code may take before it gives the
/// search back to the interpreter. Each executes at most the program's length
/// in instructions, so this is what bounds a call however the pattern and the
/// subject combine.
const ALLOWANCE: usize = 1 << 20;

type Entry = extern "C" fn(*const u8, usize, usize, *mut usize, usize) -> isize;

/// A page of memory that was written and is now executable.
struct Executable {
    page: *mut libc::c_void,
    size: usize,
}

impl Executable {
    /// Copy `code` into a fresh mapping and make it executable.
    ///
    /// Written and executable are never both permitted: the mapping is
    /// writable while it is filled and executable afterwards, which is what
    /// W^X means and what a host is expected to do here.
    fn new(code: &[u8]) -> Self {
        let size = code.len().next_multiple_of(16384).max(16384);
        unsafe {
            let page = libc::mmap(
                core::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            );
            assert!(page != libc::MAP_FAILED, "mmap failed");
            core::ptr::copy_nonoverlapping(code.as_ptr(), page as *mut u8, code.len());
            assert_eq!(
                libc::mprotect(page, size, libc::PROT_READ | libc::PROT_EXEC),
                0,
                "mprotect failed"
            );
            // An instruction cache that still holds what this memory used to be
            // would execute something else entirely.
            clear_cache(page as *mut u8, code.len());
            Self { page, size }
        }
    }

    /// The entry point, as something safe to call.
    fn entry(&self) -> Entry {
        unsafe { core::mem::transmute(self.page) }
    }
}

impl Drop for Executable {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.page, self.size);
        }
    }
}

#[cfg(target_arch = "aarch64")]
fn clear_cache(at: *mut u8, len: usize) {
    unsafe {
        unsafe extern "C" {
            fn __clear_cache(start: *mut libc::c_char, end: *mut libc::c_char);
        }
        __clear_cache(at as *mut libc::c_char, at.add(len) as *mut libc::c_char);
    }
}

#[cfg(not(target_arch = "aarch64"))]
fn clear_cache(_at: *mut u8, _len: usize) {}

struct Case {
    id: &'static str,
    pattern: &'static str,
    flags: &'static str,
    subject: String,
    iters: usize,
}

fn cases() -> Vec<Case> {
    let long = "abcdefghijklmnopqrstuvwxyz0123456789".repeat(7281) + "needle";
    let run = "a".repeat(60) + "!";
    let mut v = vec![
        Case { id: "short-literal", pattern: "needle", flags: "",
               subject: "haystack with a needle inside".into(), iters: 200_000 },
        Case { id: "short-fold", pattern: "NeEdLe", flags: "i",
               subject: "haystack with a needle inside".into(), iters: 100_000 },
        Case { id: "short-classes", pattern: "[a-z]+[0-9]+", flags: "",
               subject: "value abc123 end".into(), iters: 100_000 },
        Case { id: "diag-one-hit", pattern: "a", flags: "", subject: "a".into(), iters: 300_000 },
        Case { id: "diag-one-miss", pattern: "z", flags: "", subject: "a".into(), iters: 300_000 },
        Case { id: "diag-ten-miss", pattern: "z", flags: "",
               subject: "aaaaaaaaaa".into(), iters: 300_000 },
        Case { id: "diag-empty-subject", pattern: "z", flags: "",
               subject: String::new(), iters: 300_000 },
        Case { id: "long-literal", pattern: "needle", flags: "",
               subject: long.clone(), iters: 2_000 },
        Case { id: "short-captures", pattern: r"(\w+)@(\w+)\.com", flags: "",
               subject: "mail user@example.com now".into(), iters: 100_000 },
        Case { id: "long-two-classes", pattern: "[0-9]+[A-Z]+", flags: "",
               subject: long.clone(), iters: 200 },
        Case { id: "long-anchored-hit", pattern: "needle$", flags: "",
               subject: long.clone(), iters: 20_000 },
        Case { id: "long-lookbehind", pattern: "(?<=0123456789)abc", flags: "",
               subject: long.clone(), iters: 20_000 },
    ];
    for (id, n) in [("diag-lit1", 1usize), ("diag-lit2", 2), ("diag-lit4", 4),
                    ("diag-lit8", 8), ("diag-lit16", 16)] {
        v.push(Case { id, pattern: Box::leak("a".repeat(n).into_boxed_str()), flags: "",
                      subject: "a".repeat(n), iters: 300_000 });
    }
    for (id, pattern) in [("diag-char-repeat", "a+!"), ("diag-class1-repeat", "[a-z]+!"),
                          ("diag-class4-repeat", "\\w+!"), ("diag-classneg-repeat", "[^0-9]+!")] {
        v.push(Case { id, pattern, flags: "", subject: run.clone(), iters: 100_000 });
    }
    v
}

/// Process CPU time, for the same reason the other drivers use it: a loaded
/// machine makes wall clock measure scheduling rather than work.
fn cpu_nanos() -> u64 {
    unsafe {
        let mut t = core::mem::zeroed::<libc::timespec>();
        libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut t);
        t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
    }
}

fn time(iters: usize, mut run: impl FnMut() -> isize) -> f64 {
    for _ in 0..(iters / 20).max(1) {
        std::hint::black_box(run());
    }
    let mut best = f64::MAX;
    for _ in 0..5 {
        let start = cpu_nanos();
        for _ in 0..iters {
            std::hint::black_box(run());
        }
        let took = (cpu_nanos() - start) as f64 / iters as f64;
        best = best.min(took);
    }
    best
}

fn main() {
    println!("{:<24}{:>12}{:>12}{:>10}   answers", "case", "interp ns", "native ns", "ratio");
    println!("{}", "-".repeat(72));
    for case in cases() {
        let source = Input::utf8(case.pattern);
        let mut nodes = vec![Node::default(); source.len_utf16() * 3 + 32];
        let mut ranges = vec![Range::default(); source.len_utf16() * 12 + 32];
        let mut words = vec![0u32; source.len_utf16() * 48 + 128];
        let mut budget = Budget::new(10_000_000);
        let program = compile(source, case.flags, &mut nodes, &mut ranges, &mut words, &mut budget)
            .expect("pattern compiles");
        if !supported(program) {
            println!("{:<24}{:>12}{:>12}{:>10}   falls back to the interpreter",
                     case.id, "-", "-", "-");
            continue;
        }
        let mut code = vec![0u8; 65536];
        let mut facts = vec![Facts::default(); code.len() / 4];
        let length = emit_search(program, &mut code, &mut facts).expect("code is generated");
        let executable = Executable::new(&code[..length]);
        let entry = executable.entry();

        let subject = Input::utf8(&case.subject);
        let bytes = case.subject.as_bytes();
        let count = program.register_count();

        // Every answer, checked against the interpreter before anything is
        // timed. A faster wrong answer is a correctness failure.
        let mut agree = true;
        for start in 0..=case.subject.len().min(64) {
            let mut native = vec![usize::MAX; count.max(2)];
            let answer = entry(bytes.as_ptr(), bytes.len(), start, native.as_mut_ptr(), usize::MAX);
            // A budget too small to finish must say so rather than answer.
            for budget in [0, 1, 16] {
                let mut scratch = vec![usize::MAX; count.max(2)];
                let limited = entry(bytes.as_ptr(), bytes.len(), start, scratch.as_mut_ptr(), budget);
                if limited != EXHAUSTED && limited != answer {
                    agree = false;
                    eprintln!("{}: start {start}: budget {budget} answered {limited}, not {answer}",
                              case.id);
                }
            }
            let mut registers = vec![0usize; count];
            let mut frames = vec![Frame::default(); 4096];
            let mut undo = vec![Undo::default(); 16384];
            let mut captures = vec![None::<Span>; program.capture_count()];
            let mut budget = Budget::new(100_000_000);
            let found = find(program, subject, start,
                             Scratch { registers: &mut registers, frames: &mut frames, undo: &mut undo },
                             &mut captures, &mut budget).expect("search runs");
            if (answer >= 0) != found || (!found && answer != NO_MATCH) {
                agree = false;
                eprintln!("{}: start {start}: native {answer}, interpreted {found}", case.id);
                break;
            }
            if found {
                for (index, span) in captures.iter().enumerate() {
                    let Some(span) = span else { continue };
                    if native[index * 2] != span.start() || native[index * 2 + 1] != span.end() {
                        agree = false;
                        eprintln!("{}: start {start}: capture {index} differs", case.id);
                        break;
                    }
                }
            }
            if !agree { break; }
        }

        let mut registers = vec![0usize; count];
        let mut frames = vec![Frame::default(); 4096];
        let mut undo = vec![Undo::default(); 16384];
        let mut captures = vec![None::<Span>; program.capture_count()];
        let interpreted = time(case.iters, || {
            let mut budget = Budget::new(1_000_000_000);
            find(program, subject, 0,
                 Scratch { registers: &mut registers, frames: &mut frames, undo: &mut undo },
                 &mut captures, &mut budget).map(isize::from).unwrap_or(-1)
        });
        // What a host would run: the generated code within its allowance, and
        // the interpreter when that runs out.
        let mut native = vec![usize::MAX; count.max(2)];
        let mut exhausted = 0usize;
        let compiled = time(case.iters, || {
            match entry(bytes.as_ptr(), bytes.len(), 0, native.as_mut_ptr(), ALLOWANCE) {
                EXHAUSTED => {
                    exhausted += 1;
                    let mut budget = Budget::new(1_000_000_000);
                    find(program, subject, 0,
                         Scratch { registers: &mut registers, frames: &mut frames, undo: &mut undo },
                         &mut captures, &mut budget).map(isize::from).unwrap_or(-1)
                }
                answer => answer,
            }
        });
        println!("{:<24}{:>12.1}{:>12.1}{:>9.2}x   {}{}", case.id, interpreted, compiled,
                 compiled / interpreted, if agree { "agree" } else { "DIFFER" },
                 if exhausted > 0 { ", allowance ran out" } else { "" });
    }
}
