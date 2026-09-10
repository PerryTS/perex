//! Development-only cost decomposition; process CPU/RSS are measured externally.
//! Subject construction is setup. Every measured cursor borrows that allocation.
use perex::input::Input;
use std::hint::black_box;
use std::time::Instant;

enum Subject {
    Bytes(Vec<u8>),
    Units(Vec<u16>),
}
impl Subject {
    fn input(&self) -> Input<'_> {
        match self {
            Self::Bytes(bytes) => Input::wtf8(bytes).unwrap(),
            Self::Units(units) => Input::utf16(units),
        }
    }
    fn bytes(&self) -> usize {
        match self {
            Self::Bytes(bytes) => bytes.len(),
            Self::Units(units) => units.len() * 2,
        }
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(
        args.len(),
        4,
        "input_cost ascii|bmp|astral|cesu|surrogate|utf16-ascii|utf16-astral validate|forward|backward|seek iterations"
    );
    // Each variant constructs its original storage directly. UTF-16 cases do
    // not convert a byte subject into another representation for traversal.
    let subject = match args[1].as_str() {
        "ascii" => Subject::Bytes(vec![b'a'; 262_144]),
        "bmp" => Subject::Bytes([0xe0, 0xa0, 0x80].repeat(262_144 / 3)),
        "astral" => Subject::Bytes([0xf0, 0x9f, 0x98, 0x80].repeat(262_144 / 4)),
        "cesu" => Subject::Bytes([0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80].repeat(262_144 / 6)),
        "surrogate" => Subject::Bytes([0xed, 0xa0, 0x80].repeat(262_144 / 3)),
        "utf16-ascii" => Subject::Units(vec![97; 262_144 / 2]),
        "utf16-astral" => Subject::Units([0xd83d, 0xde00].repeat(262_144 / 4)),
        _ => panic!("unknown encoding case"),
    };
    let iterations: usize = args[3].parse().expect("numeric iterations");
    let input = subject.input();
    let mut checksum = 0_u64;
    let start = Instant::now();
    match args[2].as_str() {
        "validate" => {
            for _ in 0..iterations {
                checksum = checksum
                    .wrapping_add(black_box(black_box(&subject).input()).len_utf16() as u64);
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
        "bytes={} units={} iterations={iterations} checksum={} input_state_bytes={} cursor_state_bytes={} loop_ns={}",
        subject.bytes(),
        input.len_utf16(),
        black_box(checksum),
        core::mem::size_of::<Input<'_>>(),
        core::mem::size_of::<perex::input::Cursor<'_>>(),
        start.elapsed().as_nanos()
    );
}
