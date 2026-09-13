//! Development-only cost of one host call into the engine, by stage.
//!
//! A host that searches once per call, as a JavaScript `exec` or `test` does,
//! pays for the same few steps every call: binding the program and subject,
//! constructing a `Search`, advancing it, and reading the captures. This runs
//! those steps the way such a host does — constant-work bindings from a known
//! length and a validation witness, caller-owned scratch reused across calls —
//! and stops after the requested stage, so the difference between consecutive
//! stages is that stage's cost.
//!
//! It reports wall-clock nanoseconds per call. For instruction counts, run it
//! under an external counter such as `perf stat -e instructions` at two
//! iteration counts and divide the difference by the difference in calls, which
//! removes compilation and setup.
//!
//! ```text
//! call_cost bind|construct|search|captures PATTERN FLAGS SUBJECT ITERATIONS [START]
//! ```
use perex::{
    Budget,
    binding::{BoundProgram, BoundResources, BoundSubject},
    compiler::{Node, Range, compile},
    executor::{Frame, Progress, Scratch, Search},
    input::Input,
    span::Span,
};
use std::{hint::black_box, time::Instant};

#[derive(Clone, Copy, PartialEq, PartialOrd)]
enum Stage {
    Bind,
    Construct,
    Search,
    Captures,
}

/// The allowance and quantum Perry's runtime passes, so a call does the same
/// amount of work per advance.
const QUANTUM: usize = 4096;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if !(6..=7).contains(&args.len()) {
        return Err(
            "usage: call_cost bind|construct|search|captures PATTERN FLAGS SUBJECT ITERATIONS [START]"
                .into(),
        );
    }
    let stage = match args[1].as_str() {
        "bind" => Stage::Bind,
        "construct" => Stage::Construct,
        "search" => Stage::Search,
        "captures" => Stage::Captures,
        other => return Err(format!("unknown stage {other}").into()),
    };
    let (pattern, flags, subject) = (&args[2], &args[3], args[4].as_bytes());
    let iterations: usize = args[5].parse()?;
    let start: usize = args.get(6).map_or(Ok(0), |s| s.parse())?;

    let source = Input::utf8(pattern);
    let mut nodes = vec![Node::default(); source.len_utf16() * 3 + 32];
    let mut ranges = vec![Range::default(); source.len_utf16() * 12 + 32];
    let mut words = vec![0; source.len_utf16() * 48 + 128];
    let program = compile(
        source,
        flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut Budget::new(10_000_000),
    )
    .map_err(|e| format!("compile: {e:?}"))?;
    let words = program.words().to_vec();
    let utf16_len = Input::wtf8(subject)?.len_utf16();
    // Validated once, as a host does when it first sees the program and string.
    let witness = BoundProgram::new(words.as_slice(), &mut Budget::new(usize::MAX))
        .map_err(|e| format!("{:?}", e.error))?
        .witness();

    let mut registers = vec![0usize; program.register_count()];
    let mut frames = vec![Frame::default(); 256];
    let mut undo = vec![perex::executor::Undo::default(); 1024];
    let mut captures = vec![None::<Span>; program.capture_count()];
    let mut matched = 0usize;

    let begin = Instant::now();
    for _ in 0..iterations {
        let bound_program = BoundProgram::new_witnessed(black_box(words.as_slice()), witness)
            .map_err(|e| format!("{:?}", e.error))?;
        let bound_subject = BoundSubject::new_counted(black_box(subject), utf16_len)
            .map_err(|e| format!("{:?}", e.error))?;
        if stage == Stage::Bind {
            black_box((&bound_program, &bound_subject));
            continue;
        }
        let resources = BoundResources {
            program: &bound_program,
            subject: &bound_subject,
        };
        let scratch = Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        };
        let mut search = Search::new(&resources, start, scratch, Budget::new(usize::MAX))
            .map_err(|_| "search refused")?;
        if stage == Stage::Construct {
            black_box(&search);
            continue;
        }
        let progress = loop {
            match search.advance(QUANTUM) {
                Ok(Progress::Pending) => {}
                Ok(progress) => break progress,
                Err(_) => return Err("search failed".into()),
            }
        };
        if progress == Progress::Matched {
            matched += 1;
            if stage == Stage::Captures {
                search
                    .copy_captures(&mut captures)
                    .map_err(|e| format!("{e:?}"))?;
                black_box(&captures);
            }
        }
    }
    let nanos = begin.elapsed().as_nanos() as f64 / iterations as f64;
    println!(
        "{{\"stage\":{:?},\"iterations\":{iterations},\"matched\":{matched},\"ns_per_call\":{nanos:.2}}}",
        args[1]
    );
    Ok(())
}
