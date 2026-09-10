//! Development-only exhaustive membership through the actual compiler/VM.
//! Subject storage is two stack units; the core never converts or copies it.
use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{Frame, Scratch, Undo, find},
    input::Input,
};
use std::io::{self, BufRead, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut out = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let name = line?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'='))
        {
            return Err("invalid property name frame".into());
        }
        let pattern = format!("\\p{{{name}}}");
        let mut nodes = [Node::default(); 8];
        let mut ranges = [Range::default(); 1];
        let mut words = [0; 32];
        let p = compile(
            Input::utf8(&pattern),
            "uy",
            &mut nodes,
            &mut ranges,
            &mut words,
            &mut Budget::new(10000),
        )
        .map_err(|e| format!("{name}: {e:?}"))?;
        let mut registers = [0; 2];
        let mut frames = [Frame::default(); 1];
        let mut undo = [Undo::default(); 1];
        let mut captures = [None; 1];
        let mut previous = false;
        let mut boundaries = Vec::new();
        for point in 0u32..=0x10ffff {
            let units = if point < 0x10000 {
                [point as u16, 0]
            } else {
                [
                    0xd800 + ((point - 0x10000) >> 10) as u16,
                    0xdc00 + ((point - 0x10000) & 1023) as u16,
                ]
            };
            let length = if point < 0x10000 { 1 } else { 2 };
            let input = Input::utf16(&units[..length]);
            let found = find(
                p,
                input,
                0,
                Scratch {
                    registers: &mut registers,
                    frames: &mut frames,
                    undo: &mut undo,
                },
                &mut captures,
                &mut Budget::new(100),
            )
            .map_err(|e| format!("{name}/{point:x}: {e:?}"))?;
            if found {
                let span = captures[0].ok_or("missing capture")?;
                if span.start() != 0
                    || span.end() != length
                    || !span
                        .units(input)
                        .unwrap()
                        .eq(units[..length].iter().copied())
                {
                    return Err(format!("{name}/{point:x}: incorrect capture").into());
                }
            }
            if found != previous {
                boundaries.push(point);
                previous = found;
            }
        }
        if previous {
            boundaries.push(0x110000);
        }
        writeln!(out, "{{\"name\":{name:?},\"boundaries\":{boundaries:?}}}")?;
        out.flush()?;
    }
    Ok(())
}
