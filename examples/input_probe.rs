//! Development-only line protocol for the independent Node cursor oracle.
//! Input: id<TAB>byte hex<TAB>comma-separated UTF-16 unit hex.
use perex::input::Input;
use std::io::{self, BufRead, Write};

fn optional(value: Option<u32>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

fn emit(out: &mut impl Write, id: &str, encoding: &str, input: Input<'_>) -> io::Result<()> {
    for at in 0..=input.len_utf16() {
        let base = input.cursor_at(at).unwrap();
        let mut cursor = base;
        let next_unit = cursor.next_unit().map(u32::from);
        let next_unit_position = cursor.position();
        cursor = base;
        let previous_unit = cursor.previous_unit().map(u32::from);
        let previous_unit_position = cursor.position();
        cursor = base;
        let next_point = cursor.next_point();
        let next_point_position = cursor.position();
        cursor = base;
        let previous_point = cursor.previous_point();
        let previous_point_position = cursor.position();
        cursor = base;
        cursor.normalize_unicode_start();
        writeln!(
            out,
            "{id}\t{encoding}\t{}\t{at}\t{}\t{next_unit_position}\t{}\t{previous_unit_position}\t{}\t{next_point_position}\t{}\t{previous_point_position}\t{}",
            input.len_utf16(),
            optional(next_unit),
            optional(previous_unit),
            optional(next_point),
            optional(previous_point),
            cursor.position()
        )?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut out = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let line = line?;
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 3 || fields[1].len() % 2 != 0 {
            return Err("expected id, even byte hex, and unit hex".into());
        }
        let bytes = (0..fields[1].len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&fields[1][at..at + 2], 16))
            .collect::<Result<Vec<_>, _>>()?;
        let units = if fields[2].is_empty() {
            Vec::new()
        } else {
            fields[2]
                .split(',')
                .map(|unit| u16::from_str_radix(unit, 16))
                .collect::<Result<Vec<_>, _>>()?
        };
        emit(&mut out, fields[0], "bytes", Input::wtf8(&bytes)?)?;
        emit(&mut out, fields[0], "units", Input::utf16(&units))?;
        if let Ok(text) = core::str::from_utf8(&bytes) {
            emit(&mut out, fields[0], "str", Input::utf8(text))?;
        }
    }
    Ok(())
}
