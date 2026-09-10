//! Development-only compiler/warm-search cost driver. Run identical harness
//! source against exact engine revisions; external tools measure process CPU/RSS.
use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{Frame, Scratch, Undo, find},
    input::Input,
};
use std::{hint::black_box, mem::size_of, time::Instant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 7 {
        return Err("usage: engine_cost compile|find PATTERN_FILE FLAGS_FILE SUBJECT_FILE ITERATIONS WORK_BUDGET".into());
    }
    let pattern_bytes = std::fs::read(&args[2])?;
    let flags = std::fs::read_to_string(&args[3])?;
    let subject_bytes = std::fs::read(&args[4])?;
    let pattern = Input::wtf8(&pattern_bytes)?;
    let subject = Input::wtf8(&subject_bytes)?;
    let iterations: usize = args[5].parse()?;
    let allowance: usize = args[6].parse()?;
    if iterations == 0 {
        return Err("zero iterations".into());
    }
    let mut nodes = vec![Node::default(); pattern.len_utf16() * 3 + 32];
    let mut ranges = vec![Range::default(); pattern.len_utf16() * 12 + 32];
    let mut words = vec![0; pattern.len_utf16() * 48 + 128];
    let compile_bytes = nodes.len() * size_of::<Node>() + ranges.len() * size_of::<Range>();
    let program_capacity = words.len() * 4;
    let mut budget = Budget::new(10_000_000);
    let p = compile(
        pattern,
        &flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut budget,
    )
    .map_err(|e| format!("compile: {e:?}"))?;
    let program_bytes = p.size_bytes();
    let captures_count = p.capture_count();
    let registers_count = p.register_count();
    let mut checksum = 0usize;
    let mut max_work = 0;
    let (nanos, found) = if args[1] == "compile" {
        let start = Instant::now();
        for _ in 0..iterations {
            let mut budget = Budget::new(allowance);
            let p = compile(
                black_box(pattern),
                black_box(&flags),
                &mut nodes,
                &mut ranges,
                &mut words,
                &mut budget,
            )
            .map_err(|e| format!("compile: {e:?}"))?;
            checksum = checksum.wrapping_add(black_box(p.words()).len());
            max_work = max_work.max(allowance - budget.remaining());
        }
        (start.elapsed().as_nanos(), false)
    } else if args[1] == "find" {
        drop(nodes);
        drop(ranges);
        let mut registers = vec![0; registers_count];
        let mut frames = vec![Frame::default(); 16384];
        let mut undo = vec![Undo::default(); 131072];
        let mut captures = vec![None; captures_count];
        let mut last = false;
        let start = Instant::now();
        for _ in 0..iterations {
            let mut budget = Budget::new(allowance);
            last = find(
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
            .map_err(|e| format!("find: {e:?}"))?;
            checksum = checksum.wrapping_add(usize::from(black_box(last)));
            for span in black_box(&captures).iter().flatten() {
                checksum = checksum.wrapping_add(span.start()).wrapping_add(span.end());
            }
            max_work = max_work.max(allowance - budget.remaining());
        }
        (start.elapsed().as_nanos(), last)
    } else {
        return Err("unknown mode".into());
    };
    println!(
        "{{\"mode\":\"{}\",\"iterations\":{},\"nanoseconds\":{},\"found\":{},\"checksum\":{},\"max_work\":{},\"program_bytes\":{},\"program_capacity_bytes\":{},\"compile_scratch_bytes\":{},\"register_bytes\":{},\"frame_capacity_bytes\":{},\"undo_capacity_bytes\":{}}}",
        args[1],
        iterations,
        nanos,
        found,
        checksum,
        max_work,
        program_bytes,
        program_capacity,
        compile_bytes,
        registers_count * size_of::<usize>(),
        16384 * size_of::<Frame>(),
        131072 * size_of::<Undo>()
    );
    Ok(())
}
