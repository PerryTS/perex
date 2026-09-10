//! Development-only scratch-policy driver. All policies use the same Search.
//! Counters measure owned buffer payload, excluding allocator metadata/RSS.
use perex::{
    Budget,
    binding::{BoundProgram, BoundResources, BoundSubject},
    compiler::{Node, Range, compile},
    executor::{
        ExecError, Frame, Progress, Resources, Scratch, ScratchOwner, Search, SearchError, Undo,
        find,
    },
    input::Input,
    span::Span,
};
use std::{cell::Cell, hint::black_box, mem::size_of, time::Instant};

const MAX_FRAMES: usize = 16_384;
const MAX_UNDO: usize = 131_072;
const RETAIN_LIMIT: usize = 65_536;

#[derive(Default)]
struct Accounting {
    live: Cell<usize>,
    peak: Cell<usize>,
    allocated: Cell<usize>,
    allocations: Cell<usize>,
    freed: Cell<usize>,
    frees: Cell<usize>,
    replacements: Cell<usize>,
    transferred: Cell<usize>,
}
struct Buffers<'a> {
    registers: Vec<usize>,
    frames: Vec<Frame>,
    undo: Vec<Undo>,
    accounting: &'a Accounting,
}
impl<'a> Buffers<'a> {
    fn new(registers: usize, frames: usize, undo: usize, accounting: &'a Accounting) -> Self {
        let result = Self {
            registers: vec![0; registers],
            frames: vec![Frame::default(); frames],
            undo: vec![Undo::default(); undo],
            accounting,
        };
        let bytes = result.bytes();
        accounting.live.set(accounting.live.get() + bytes);
        accounting
            .peak
            .set(accounting.peak.get().max(accounting.live.get()));
        accounting.allocated.set(accounting.allocated.get() + bytes);
        accounting
            .allocations
            .set(accounting.allocations.get() + result.allocations());
        result
    }
    fn bytes(&self) -> usize {
        self.registers.capacity() * size_of::<usize>()
            + self.frames.capacity() * size_of::<Frame>()
            + self.undo.capacity() * size_of::<Undo>()
    }
    fn allocations(&self) -> usize {
        usize::from(self.registers.capacity() != 0)
            + usize::from(self.frames.capacity() != 0)
            + usize::from(self.undo.capacity() != 0)
    }
}
impl Drop for Buffers<'_> {
    fn drop(&mut self) {
        let a = self.accounting;
        a.live.set(a.live.get() - self.bytes());
        a.freed.set(a.freed.get() + self.bytes());
        a.frees.set(a.frees.get() + self.allocations());
    }
}
impl ScratchOwner for Buffers<'_> {
    fn scratch(&mut self) -> Scratch<'_> {
        Scratch {
            registers: &mut self.registers,
            frames: &mut self.frames,
            undo: &mut self.undo,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Policy {
    Fixed,
    Fresh,
    Reuse,
    Capped,
}
impl Policy {
    fn initial(self) -> (usize, usize) {
        if self == Self::Fixed {
            (MAX_FRAMES, MAX_UNDO)
        } else {
            (0, 0)
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
struct Answer {
    found: bool,
    captures: Vec<Option<Span>>,
    work: usize,
}
struct Run {
    nanos: u128,
    checksum: usize,
    max_work: usize,
    retained: usize,
    answer: Answer,
}

fn run<R: Resources>(
    resources: &R,
    shape: (usize, usize),
    policy: Policy,
    iterations: usize,
    allowance: usize,
    accounting: &Accounting,
    expected: Option<&Answer>,
) -> Result<Run, String>
where
    R::Error: std::fmt::Debug,
{
    let (registers, captures) = shape;
    let mut captures = vec![None; captures];
    let mut buffers = None;
    let mut checksum = 0usize;
    let mut max_work = 0;
    let mut last = false;
    let mut last_work = 0;
    let mut retained = 0;
    let start = Instant::now();
    for _ in 0..iterations {
        let current = buffers.take().unwrap_or_else(|| {
            let (frames, undo) = policy.initial();
            Buffers::new(registers, frames, undo, accounting)
        });
        let (mut frame_capacity, mut undo_capacity) = (current.frames.len(), current.undo.len());
        let mut search = Search::new(black_box(resources), 0, current, Budget::new(allowance))
            .map_err(|e| format!("new: {e:?}"))?;
        loop {
            match search.advance(usize::MAX) {
                Ok(Progress::Matched) => {
                    last = true;
                    break;
                }
                Ok(Progress::NoMatch) => {
                    last = false;
                    break;
                }
                Ok(Progress::Pending) => {
                    return Err("uninterrupted search unexpectedly paused".into());
                }
                Err(SearchError::Execution(error @ (ExecError::Frames | ExecError::Undo))) => {
                    if policy == Policy::Fixed {
                        return Err(format!("fixed: {error:?}"));
                    }
                    let need = search.required_scratch();
                    // Grow only the exhausted capacity, with the same hard caps
                    // as fixed execution. Account for the old/new overlap.
                    if need.frames > frame_capacity {
                        frame_capacity = need.frames.next_power_of_two();
                    }
                    if need.undo > undo_capacity {
                        undo_capacity = need.undo.next_power_of_two();
                    }
                    if frame_capacity > MAX_FRAMES || undo_capacity > MAX_UNDO {
                        return Err(format!("capacity cap: {error:?}"));
                    }
                    let next = Buffers::new(registers, frame_capacity, undo_capacity, accounting);
                    let live_frames = need.frames - usize::from(error == ExecError::Frames);
                    let live_undo = need.undo - usize::from(error == ExecError::Undo);
                    accounting.transferred.set(
                        accounting.transferred.get()
                            + registers * size_of::<usize>()
                            + live_frames * size_of::<Frame>()
                            + live_undo * size_of::<Undo>(),
                    );
                    search = search
                        .rebuffer(next)
                        .map_err(|e| format!("rebuffer: {:?}", e.error))?;
                    accounting
                        .replacements
                        .set(accounting.replacements.get() + 1);
                }
                Err(error) => return Err(format!("advance: {error:?}")),
            }
        }
        captures.fill(None);
        if last {
            search
                .copy_captures(&mut captures)
                .map_err(|e| format!("captures: {e:?}"))?;
        }
        last_work = allowance - search.remaining_work();
        max_work = max_work.max(last_work);
        if let Some(expected) = expected
            && (last != expected.found
                || captures != expected.captures
                || last_work != expected.work)
        {
            return Err(format!(
                "{policy:?} mismatch: {last} {captures:?} {last_work}; expected {expected:?}"
            ));
        }
        checksum = checksum.wrapping_add(usize::from(black_box(last)));
        for span in black_box(&captures).iter().flatten() {
            checksum = checksum.wrapping_add(span.start()).wrapping_add(span.end());
        }
        let returned = search.into_buffers();
        if policy == Policy::Fresh || (policy == Policy::Capped && returned.bytes() > RETAIN_LIMIT)
        {
            drop(returned);
        } else {
            buffers = Some(returned);
        }
        retained = accounting.live.get();
    }
    // Explicit final cleanup belongs to the measured operation lifetime.
    drop(buffers);
    let nanos = start.elapsed().as_nanos();
    assert_eq!(accounting.live.get(), 0);
    assert_eq!(accounting.allocated.get(), accounting.freed.get());
    assert_eq!(accounting.allocations.get(), accounting.frees.get());
    Ok(Run {
        nanos,
        checksum,
        max_work,
        retained,
        answer: Answer {
            found: last,
            captures,
            work: last_work,
        },
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 7 {
        return Err("usage: scratch_cost verify|fixed|fresh|reuse|capped PATTERN_FILE FLAGS_FILE SUBJECT_FILE ITERATIONS WORK_BUDGET".into());
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
    let compile_bytes =
        nodes.capacity() * size_of::<Node>() + ranges.capacity() * size_of::<Range>();
    let program_capacity = words.capacity() * size_of::<u32>();
    let p = compile(
        pattern,
        &flags,
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut Budget::new(10_000_000),
    )
    .map_err(|e| format!("compile: {e:?}"))?;
    drop(nodes);
    drop(ranges);
    let program_bytes = p.size_bytes();
    let shape = (p.register_count(), p.capture_count());
    let program = BoundProgram::new(p.words(), &mut Budget::new(10_000_000))
        .map_err(|e| format!("bind program: {:?}", e.error))?;
    let subject_binding = BoundSubject::new(subject_bytes.as_slice())
        .map_err(|e| format!("bind subject: {:?}", e.error))?;
    let resources = BoundResources {
        program: &program,
        subject: &subject_binding,
    };
    if args[1] == "verify" {
        let a = Accounting::default();
        let mut buffers = Buffers::new(shape.0, MAX_FRAMES, MAX_UNDO, &a);
        let mut captures = vec![None; shape.1];
        let mut budget = Budget::new(allowance);
        let found = find(p, subject, 0, buffers.scratch(), &mut captures, &mut budget)
            .map_err(|e| format!("reference find: {e:?}"))?;
        let expected = Answer {
            found,
            captures,
            work: allowance - budget.remaining(),
        };
        drop(buffers);
        for policy in [Policy::Fixed, Policy::Fresh, Policy::Reuse, Policy::Capped] {
            run(
                &resources,
                shape,
                policy,
                iterations,
                allowance,
                &Accounting::default(),
                Some(&expected),
            )?;
        }
        println!(
            "{{\"verified\":true,\"policies\":4,\"iterations\":{iterations},\"found\":{found},\"work\":{}}}",
            expected.work
        );
        return Ok(());
    }
    let policy = match args[1].as_str() {
        "fixed" => Policy::Fixed,
        "fresh" => Policy::Fresh,
        "reuse" => Policy::Reuse,
        "capped" => Policy::Capped,
        _ => return Err("unknown mode".into()),
    };
    let a = Accounting::default();
    let r = run(&resources, shape, policy, iterations, allowance, &a, None)?;
    let captures: Vec<_> = r
        .answer
        .captures
        .iter()
        .map(|span| match span {
            Some(span) => format!("[{},{}]", span.start(), span.end()),
            None => "null".into(),
        })
        .collect();
    println!(
        concat!(
            "{{\"mode\":\"{}\",\"iterations\":{},\"nanoseconds\":{},\"found\":{},",
            "\"checksum\":{},\"max_work\":{},\"captures\":[{}],\"program_bytes\":{},",
            "\"program_capacity_bytes\":{},\"compile_scratch_bytes\":{},\"capture_capacity_bytes\":{},",
            "\"peak_owned_scratch_bytes\":{},\"retained_scratch_bytes\":{},\"final_owned_scratch_bytes\":{},",
            "\"allocated_scratch_bytes\":{},\"freed_scratch_bytes\":{},\"allocations\":{},\"frees\":{},",
            "\"replacements\":{},\"transferred_metadata_bytes\":{},\"retention_limit_bytes\":{}}}"
        ),
        args[1],
        iterations,
        r.nanos,
        r.answer.found,
        r.checksum,
        r.max_work,
        captures.join(","),
        program_bytes,
        program_capacity,
        compile_bytes,
        r.answer.captures.capacity() * size_of::<Option<Span>>(),
        a.peak.get(),
        r.retained,
        a.live.get(),
        a.allocated.get(),
        a.freed.get(),
        a.allocations.get(),
        a.frees.get(),
        a.replacements.get(),
        a.transferred.get(),
        if policy == Policy::Capped {
            RETAIN_LIMIT
        } else {
            usize::MAX
        }
    );
    Ok(())
}
