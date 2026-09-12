//! Development-only: which benchmark patterns the compilation tier could emit
//! code for. See `docs/compilation.md`.
use perex::{
    Budget,
    compiler::{Node, Range, compile},
    input::Input,
};
fn main() {
    let cases: &[(&str, &str)] = &[
        ("needle", ""),
        ("(?<=0123456789)abc", ""),
        ("a+END", ""),
        ("(?:needle|thimble|haystack)", ""),
        ("[a-z]+[0-9]+", ""),
        (r"(\w+)@(\w+)\.com", ""),
        ("NeEdLe", "i"),
        ("a", ""),
        ("aaaa", ""),
        ("z", ""),
        ("a+!", ""),
        ("[a-z]+!", ""),
        (r"\w+!", ""),
        ("[^0-9]+!", ""),
        ("needle$", ""),
    ];
    let (mut yes, mut no) = (0, 0);
    for (pat, fl) in cases {
        let src = Input::utf8(pat);
        let mut n = vec![Node::default(); src.len_utf16() * 3 + 32];
        let mut r = vec![Range::default(); src.len_utf16() * 12 + 32];
        let mut w = vec![0u32; src.len_utf16() * 48 + 128];
        let mut b = Budget::new(10_000_000);
        let p = compile(src, fl, &mut n, &mut r, &mut w, &mut b).unwrap();
        if p.compilable() {
            yes += 1
        } else {
            no += 1
        }
        println!(
            "{:<30} {}",
            pat,
            if p.compilable() {
                "QUALIFIES"
            } else {
                "falls back"
            }
        );
    }
    println!("\n{yes} qualify, {no} fall back");
}
