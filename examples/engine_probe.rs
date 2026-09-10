//! Development-only complete-answer harness. All allocation here is host policy;
//! the library receives borrowed source/subject and explicit scratch/storage.
use perex::{
    Budget,
    compiler::{CompileError, Node, Range, compile},
    executor::{Frame, Scratch, Undo, find},
    input::Input,
    span::Span,
};
use std::io::{self, BufRead, Write};
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
        let mut frames = vec![Frame::default(); 16_384];
        let mut undo = vec![Undo::default(); 131_072];
        let mut captures = vec![None; program.capture_count()];
        let found = find(
            program,
            input,
            f[4].parse()?,
            Scratch {
                registers: &mut registers,
                frames: &mut frames,
                undo: &mut undo,
            },
            &mut captures,
            &mut Budget::new(2_000_000),
        );
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
    Ok(())
}
