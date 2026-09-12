//! The example in `README.md`, kept here so `cargo check --all-targets` keeps
//! it compiling. A crate's front page is the first thing anyone runs.
use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{Frame, Scratch, Undo, find},
    input::Input,
    span::Span,
};

fn main() -> Result<(), String> {
    let pattern = Input::utf8(r"(\w+)@(\w+)\.com");
    let mut nodes = vec![Node::default(); 256];
    let mut ranges = vec![Range::default(); 512];
    let mut words = vec![0u32; 2048];
    let mut budget = Budget::new(1_000_000);
    let program = compile(
        pattern,
        "",
        &mut nodes,
        &mut ranges,
        &mut words,
        &mut budget,
    )
    .map_err(|e| format!("{e:?}"))?;

    let mut registers = vec![0usize; program.register_count()];
    let mut frames = vec![Frame::default(); 256];
    let mut undo = vec![Undo::default(); 1024];
    let mut captures = vec![None::<Span>; program.capture_count()];
    let mut budget = Budget::new(1_000_000);

    let found = find(
        program,
        Input::utf8("mail user@example.com now"),
        0,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        &mut captures,
        &mut budget,
    )
    .map_err(|e| format!("{e:?}"))?;

    assert!(found);
    assert_eq!(captures[1].map(|s| (s.start(), s.end())), Some((5, 9)));
    println!("matched {:?}", captures[0].map(|s| (s.start(), s.end())));
    Ok(())
}
