//! Compile once, then scan the same mixed UTF-16 subject with reused scratch.
//! Run under perf stat; the checksum covers every returned span, not wall time.
use perex::{
    Budget,
    compiler::{Node, Range, compile},
    dfa::{self, Found},
    executor::{Frame, Scratch, Undo, find},
    input::Input,
};

fn main() {
    #[cfg(feature = "selectable-unicode")]
    perex::install_unicode_properties();
    let args: Vec<_> = std::env::args().collect();
    let case = args.get(1).expect("letters | greek_nd | upper_fold");
    let engine = args.get(2).expect("vm | dfa");
    let (pattern, flags) = match case.as_str() {
        "letters" => (r"\p{L}+", "gu"),
        "greek_nd" => (r"[\p{Script=Greek}\p{Nd}]", "u"),
        "upper_fold" => (r"\p{Lu}", "iu"),
        _ => panic!("unknown case"),
    };
    assert!(engine == "vm" || engine == "dfa");
    // Native UTF-16 storage makes global-loop start seeks constant-time.
    // Repeated byte-input find calls would benchmark prefix rescanning instead.
    let unit: Vec<u16> = "_Abc αβΓΔ 012９ 汉字 ſK 𐐀𐐨 😀!\n".encode_utf16().collect();
    let subject = unit.repeat(65536);
    let input = Input::utf16(&subject);
    let mut words = [0; 4096];
    let program = compile(
        Input::utf8(pattern),
        flags,
        &mut [Node::default(); 256],
        &mut [Range::default(); 512],
        &mut words,
        &mut Budget::new(10_000_000),
    )
    .unwrap();
    let mut registers = vec![0; program.register_count()];
    let mut frames = vec![Frame::default(); 128];
    let mut undo = vec![Undo::default(); 1024];
    let mut captures = vec![None; program.capture_count()];
    let mut cache = vec![0; 65536];
    let (mut count, mut starts, mut ends) = (0u64, 0u64, 0u64);
    let repeats = 3;
    for _ in 0..repeats {
        let mut start = 0;
        let mut budget = Budget::new(1_000_000_000);
        while start < input.len_utf16() {
            let span = if engine == "dfa" {
                match dfa::find(program, input, start, &mut cache, &mut budget).unwrap() {
                    Found::Match(span) => Some(span),
                    Found::NoMatch => None,
                    Found::Declined(why) => panic!("DFA declined: {why:?}"),
                }
            } else if find(
                program,
                input,
                start,
                Scratch {
                    registers: &mut registers,
                    frames: &mut frames,
                    undo: &mut undo,
                },
                &mut captures,
                &mut budget,
            )
            .unwrap()
            {
                captures[0]
            } else {
                None
            };
            let Some(span) = span else {
                break;
            };
            assert!(span.end() > start && span.end() > span.start());
            count += 1;
            starts += span.start() as u64;
            ends += span.end() as u64;
            start = span.end();
        }
    }
    let word_sum: u64 = program.words().iter().map(|&w| u64::from(w)).sum();
    println!(
        "{{\"case\":{case:?},\"engine\":{engine:?},\"bytes\":{},\"utf16\":{},\"repeats\":{repeats},\"matches\":{count},\"starts\":{starts},\"ends\":{ends},\"program_words\":{},\"program_sum\":{word_sum}}}",
        subject.len() * 2,
        input.len_utf16(),
        program.words().len()
    );
}
