//! Development-only cost decomposition; process CPU/RSS are measured externally.
//! Subject construction is setup. Every measured cursor borrows that allocation.
use perex::input::Input;
use std::hint::black_box;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(
        args.len(),
        4,
        "input_cost ascii|bmp|astral validate|forward|backward|seek iterations"
    );
    let fragment = match args[1].as_str() {
        "ascii" => "a",
        "bmp" => "\u{0800}",
        "astral" => "😀",
        _ => panic!("unknown encoding case"),
    };
    let iterations: usize = args[3].parse().expect("numeric iterations");
    let subject = fragment.repeat(262_144 / fragment.len());
    let input = Input::wtf8(subject.as_bytes()).unwrap();
    let mut checksum = 0_u64;
    match args[2].as_str() {
        "validate" => {
            for _ in 0..iterations {
                checksum = checksum.wrapping_add(
                    black_box(Input::wtf8(black_box(subject.as_bytes())).unwrap()).len_utf16()
                        as u64,
                );
            }
        }
        "forward" => {
            for _ in 0..iterations {
                let mut cursor = black_box(input).cursor();
                while let Some(point) = cursor.next_point() {
                    checksum = checksum.wrapping_add(u64::from(point));
                }
                black_box(cursor.position());
            }
        }
        "backward" => {
            for _ in 0..iterations {
                let view = black_box(input);
                let mut cursor = view.cursor_at(view.len_utf16()).unwrap();
                while let Some(point) = cursor.previous_point() {
                    checksum = checksum.wrapping_add(u64::from(point));
                }
                black_box(cursor.position());
            }
        }
        "seek" => {
            for _ in 0..iterations {
                let view = black_box(input);
                let cursor = view.cursor_at(black_box(view.len_utf16() / 2)).unwrap();
                checksum = checksum.wrapping_add(black_box(cursor).position() as u64);
            }
        }
        _ => panic!("unknown operation"),
    }
    println!(
        "bytes={} units={} iterations={iterations} checksum={} input_state_bytes={} cursor_state_bytes={}",
        subject.len(),
        input.len_utf16(),
        black_box(checksum),
        core::mem::size_of::<Input<'_>>(),
        core::mem::size_of::<perex::input::Cursor<'_>>()
    );
}
