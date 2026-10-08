//! Development-only complete-answer harness. All allocation here is host policy;
//! the library receives borrowed source/subject and explicit scratch/storage.
use perex::{
    Budget,
    compiler::{CompileError, Node, Range, compile},
    dfa,
    executor::{Frame, Scratch, Undo, find},
    input::Input,
    span::Span,
};
use std::io::{self, BufRead, Write};
#[path = "support/resumption.rs"]
mod resumable;
fn capture(out: &mut impl Write, span: Option<Span>, input: Input<'_>) -> io::Result<()> {
    if let Some(span) = span {
        write!(
            out,
            "{{\"span\":[{},{}],\"units\":[",
            span.start(),
            span.end()
        )?;
        for (j, unit) in span.units(input).unwrap().enumerate() {
            if j != 0 {
                write!(out, ",")?;
            }
            write!(out, "{unit}")?;
        }
        write!(out, "]}}")
    } else {
        write!(out, "null")
    }
}
fn bytes(hex: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if !hex.len().is_multiple_of(2) || !hex.is_ascii() {
        return Err("invalid hex".into());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(Into::into))
        .collect()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    // The execution allowance is a harness setting, not a semantic limit. A
    // pattern that needs more than the default is a cost result, not a wrong
    // answer, so a corpus with deliberately expensive patterns can raise it
    // and still see an explicit outcome rather than an unbounded run.
    let mut work = 2_000_000;
    let mut next = args.next();
    if next.as_deref() == Some("--work") {
        work = args
            .next()
            .ok_or("missing work allowance")?
            .parse::<usize>()?;
        if work == 0 {
            return Err("work allowance must be positive".into());
        }
        next = args.next();
    }
    let options = match next.as_deref() {
        None => None,
        Some("--quantum") => {
            let quantum = args.next().ok_or("missing quantum")?.parse::<usize>()?;
            if quantum == 0 {
                return Err("quantum must be positive".into());
            }
            let mut relocate = false;
            let mut grow = false;
            let mut near = false;
            let mut run = false;
            for arg in args.by_ref() {
                match arg.as_str() {
                    "--relocate" if !relocate => relocate = true,
                    "--grow" if !grow => grow = true,
                    "--near" if !near => near = true,
                    "--run" if !run => run = true,
                    _ => {
                        return Err("expected unique --relocate, --grow, --near or --run".into());
                    }
                }
            }
            Some(resumable::Options {
                quantum,
                relocate,
                grow,
                near,
                run,
            })
        }
        _ => {
            return Err(
                "expected [--work N] [--quantum N [--relocate] [--grow] [--near] [--run]]".into(),
            );
        }
    };
    if args.next().is_some() {
        return Err("extra argument".into());
    }
    // `PEREX_DFA` answers every eligible case with the lazy automaton as well
    // and reports any disagreement as an outcome of its own, so every harness
    // that runs this probe compares the automaton too: `1` with a roomy cache,
    // `small` with the least storage each program accepts plus a little, which
    // clears and thrashes, or a number of words.
    let automaton = std::env::var("PEREX_DFA")
        .ok()
        .filter(|mode| !mode.is_empty());
    let mut cache: Vec<u32> = Vec::new();
    let mut cached_program: Vec<u32> = Vec::new();
    let (mut eligible, mut answered) = (0usize, 0usize);
    let mut out = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let line = line?;
        let f: Vec<_> = line.split('\t').collect();
        if f.len() != 5
            || !f[0]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':'))
        {
            return Err("invalid frame".into());
        }
        let id = f[0];
        let source = bytes(f[1])?;
        let subject = bytes(f[3])?;
        let source = Input::wtf8(&source)?;
        let input = Input::wtf8(&subject)?;
        let mut nodes = vec![Node::default(); source.len_utf16() * 3 + 32];
        let mut ranges = vec![Range::default(); source.len_utf16() * 12 + 32];
        let mut words = vec![0; source.len_utf16() * 48 + 128];
        let mut budget = Budget::new(10_000_000);
        let program = match compile(
            source,
            f[2],
            &mut nodes,
            &mut ranges,
            &mut words,
            &mut budget,
        ) {
            Ok(p) => p,
            Err(CompileError::Syntax { .. }) => {
                writeln!(out, "{{\"id\":{id:?},\"outcome\":\"syntax-error\"}}")?;
                continue;
            }
            Err(CompileError::Unsupported { feature, .. }) => {
                writeln!(
                    out,
                    "{{\"id\":{id:?},\"outcome\":\"unsupported\",\"feature\":{feature:?}}}"
                )?;
                continue;
            }
            Err(e) => {
                writeln!(
                    out,
                    "{{\"id\":{id:?},\"outcome\":\"compile-error\",\"error\":\"{e:?}\"}}"
                )?;
                continue;
            }
        };
        let mut registers = vec![0; program.register_count()];
        let grow = options.is_some_and(|options| options.grow);
        let mut frames = vec![Frame::default(); if grow { 0 } else { 16_384 }];
        let mut undo = vec![Undo::default(); if grow { 0 } else { 131_072 }];
        let mut captures = vec![None; program.capture_count()];
        let scratch = Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        };
        let mut budget = Budget::new(work);
        let start = f[4].parse()?;
        let found = if let Some(options) = options {
            resumable::find(
                program,
                &subject,
                start,
                scratch,
                &mut captures,
                &mut budget,
                options,
            )
        } else {
            find(program, input, start, scratch, &mut captures, &mut budget)
        };
        // Where the evaluator ran out of work, the automaton's answer stands
        // in for it: no match, or the evaluator's own captures from the start
        // the automaton found. Node then checks that answer.
        let found = match found {
            Err(perex::executor::ExecError::WorkLimit)
                if automaton.is_some() && dfa::eligible(program) =>
            {
                let mut cache = vec![0; dfa::minimum_words(program).unwrap().max(1 << 16)];
                match dfa::find(program, input, start, &mut cache, &mut Budget::new(work)) {
                    Ok(dfa::Found::NoMatch) => Ok(false),
                    Ok(dfa::Found::Match(span)) => {
                        let mut registers = vec![0; program.register_count()];
                        let mut frames = vec![Frame::default(); 16_384];
                        let mut undo = vec![Undo::default(); 131_072];
                        let rerun = find(
                            program,
                            input,
                            span.start(),
                            Scratch {
                                registers: &mut registers,
                                frames: &mut frames,
                                undo: &mut undo,
                            },
                            &mut captures,
                            &mut Budget::new(work),
                        );
                        match rerun {
                            Ok(true) if captures[0] == Some(span) => Ok(true),
                            Ok(_) => {
                                writeln!(
                                    out,
                                    "{{\"id\":{id:?},\"outcome\":\"dfa-mismatch\",\"rerun\":\"{rerun:?}\"}}"
                                )?;
                                continue;
                            }
                            Err(error) => Err(error),
                        }
                    }
                    other => {
                        if std::env::var_os("PEREX_DFA_DEBUG").is_some() {
                            eprintln!("{id}: evaluator out of work, automaton {other:?}");
                        }
                        found
                    }
                }
            }
            found => found,
        };
        if let Some(size) = automaton.as_deref()
            && let Some(minimum) = dfa::minimum_words(program)
            && let Ok(vm) = found
        {
            let words = match size {
                "small" => minimum + 256,
                "1" => minimum.max(1 << 16),
                number => number.parse::<usize>()?.max(minimum),
            };
            // One cache per program: a different program starts a new one.
            if cached_program != program.words() || cache.len() != words {
                cache = vec![0; words];
                cached_program = program.words().to_vec();
            }
            let mut budget = Budget::new(work);
            let tested = dfa::is_match(program, input, start, &mut cache, &mut budget);
            let mut budget = Budget::new(work);
            let bounds = dfa::find(program, input, start, &mut cache, &mut budget);
            eligible += 1;
            answered += usize::from(!matches!(bounds, Ok(dfa::Found::Declined(_))));
            // A declined call is not an answer: the cache may also disable
            // itself between the two calls. Every answer must be the
            // evaluator's.
            let tested_agrees = match tested {
                Ok(dfa::Tested::Declined(_)) => true,
                Ok(dfa::Tested::Match) => vm,
                Ok(dfa::Tested::NoMatch) => !vm,
                // Exhausting the allowance is not an answer either.
                Err(perex::executor::ExecError::WorkLimit) => true,
                Err(_) => false,
            };
            let bounds_agree = match bounds {
                Ok(dfa::Found::Declined(_)) => true,
                Ok(dfa::Found::NoMatch) => !vm,
                Ok(dfa::Found::Match(span)) => {
                    // Captures come from the evaluator started at the
                    // automaton's start: they must be the same match.
                    let mut again = vec![None; program.capture_count()];
                    let mut registers = vec![0; program.register_count()];
                    let mut frames = vec![Frame::default(); 16_384];
                    let mut undo = vec![Undo::default(); 131_072];
                    let rerun = find(
                        program,
                        input,
                        span.start(),
                        Scratch {
                            registers: &mut registers,
                            frames: &mut frames,
                            undo: &mut undo,
                        },
                        &mut again,
                        &mut Budget::new(work),
                    );
                    vm && captures[0] == Some(span) && rerun == Ok(true) && again == captures
                }
                Err(perex::executor::ExecError::WorkLimit) => true,
                Err(_) => false,
            };
            let agreed = tested_agrees && bounds_agree;
            if !agreed {
                writeln!(
                    out,
                    "{{\"id\":{id:?},\"outcome\":\"dfa-mismatch\",\"vm\":{vm},\"test\":\"{tested:?}\",\"find\":\"{bounds:?}\"}}"
                )?;
                continue;
            }
        }
        match found {
            Ok(false) => writeln!(out, "{{\"id\":{id:?},\"outcome\":\"no-match\"}}")?,
            Err(error) => writeln!(
                out,
                "{{\"id\":{id:?},\"outcome\":\"execution-error\",\"error\":\"{error:?}\"}}"
            )?,
            Ok(true) => {
                write!(out, "{{\"id\":{id:?},\"outcome\":\"match\",\"captures\":[")?;
                for (i, span) in captures.iter().enumerate() {
                    if i != 0 {
                        write!(out, ",")?;
                    }
                    capture(&mut out, *span, input)?;
                }
                write!(out, "],\"groups\":")?;
                if program.name_count() == 0 {
                    writeln!(out, "null}}")?;
                } else {
                    let mut groups: Vec<_> = program.named_groups().collect();
                    groups.sort_by(|a, b| a.name_units().cmp(b.name_units()));
                    write!(out, "[")?;
                    for (i, group) in groups.into_iter().enumerate() {
                        if i != 0 {
                            write!(out, ",")?;
                        }
                        write!(out, "{{\"name\":\"")?;
                        for unit in group.name_units() {
                            write!(out, "\\u{unit:04x}")?;
                        }
                        write!(out, "\",\"capture\":")?;
                        let span = group
                            .capture_indices()
                            .iter()
                            .find_map(|&i| captures[i as usize]);
                        capture(&mut out, span, input)?;
                        write!(out, "}}")?;
                    }
                    writeln!(out, "]}}")?;
                }
            }
        }
    }
    if automaton.is_some() {
        eprintln!("dfa: eligible {eligible} answered {answered}");
    }
    Ok(())
}
