//! ASCII storage and UTF-16 storage must observe the same literal opcodes,
//! including complete captures across pause sizes. Admission can use different
//! pause sequences for different original encodings; this test compares answers.
use perex::{
    Budget,
    compiler::{Node, Range, compile},
    executor::{ExecError, Frame, Progress, Resources, Scratch, Search, SearchError, Undo},
    input::Input,
    program::Program,
    span::Span,
};

struct Views<'a> {
    program: Program<'a>,
    input: Input<'a>,
}
impl Resources for Views<'_> {
    type Error = core::convert::Infallible;
    fn with_views<T>(&self, f: impl FnOnce(Program<'_>, Input<'_>) -> T) -> Result<T, Self::Error> {
        Ok(f(self.program, self.input))
    }
}

type Observation = (Result<Progress, ExecError>, usize, usize);
fn observations(
    program: Program<'_>,
    input: Input<'_>,
    start: usize,
    work: usize,
    quantum: usize,
) -> (Vec<Observation>, Vec<Option<Span>>) {
    let views = Views { program, input };
    let mut registers = vec![usize::MAX; program.register_count()];
    let mut frames = vec![Frame::default(); 128];
    let mut undo = vec![Undo::default(); 1024];
    let mut search = Search::new(
        &views,
        start,
        Scratch {
            registers: &mut registers,
            frames: &mut frames,
            undo: &mut undo,
        },
        Budget::new(work),
    )
    .unwrap();
    let mut observations = Vec::new();
    let mut captures = vec![None; program.capture_count()];
    loop {
        let result = match search.advance(quantum) {
            Ok(progress) => Ok(progress),
            Err(SearchError::Execution(error)) => Err(error),
            Err(SearchError::Resource(never)) => match never {},
        };
        observations.push((result, search.remaining_work(), search.position().utf16()));
        assert!(
            observations.len() <= work + 2,
            "search made no bounded progress"
        );
        if result == Ok(Progress::Matched) {
            search.copy_captures(&mut captures).unwrap();
        }
        if result != Ok(Progress::Pending) {
            break;
        }
    }
    (observations, captures)
}

#[test]
fn literal_chains_preserve_complete_captures_across_storage_and_pauses() {
    let patterns = [
        "^abcdef$",
        "^(abc)(def)$",
        "^(ab|abc)(def)$",
        "(abc)(def)",
        "^(abcdef|abc)def$",
        "^(ab)+(cdef)$",
        "^(abc)(?=def)def$",
        "(?<=(abc))(def)",
        "^(abc)(?!def)xyz$",
        "^(abc)\\1$",
        "^(a*)(bcdef)$",
        "^abé$",
        "^a\\u{1f600}$",
        "^abc$",
    ];
    let subjects = [
        "", "a", "abc", "abcdef", "abcabc", "abcxyz", "ababcdef", "abcdef!", "xabcdef",
    ];
    for source in patterns {
        for flags in ["", "u", "i", "iu", "y"] {
            let mut nodes = vec![Node::default(); 1024];
            let mut ranges = vec![Range::default(); 4096];
            let mut words = vec![0; 8192];
            let program = compile(
                Input::utf8(source),
                flags,
                &mut nodes,
                &mut ranges,
                &mut words,
                &mut Budget::new(1_000_000),
            )
            .unwrap();
            for subject in subjects {
                let units: Vec<_> = subject.encode_utf16().collect();
                for start in [0, 1, subject.len(), subject.len() + 1] {
                    for quantum in [1, 2, 3, 7, 17, 4096, usize::MAX] {
                        {
                            let work = 4096;
                            let ascii =
                                observations(program, Input::utf8(subject), start, work, quantum);
                            let units =
                                observations(program, Input::utf16(&units), start, work, quantum);
                            assert_eq!(
                                (ascii.0.last().unwrap().0, ascii.1),
                                (units.0.last().unwrap().0, units.1),
                                "/{source}/{flags} subject={subject:?} start={start} work={work} quantum={quantum}"
                            );
                        }
                    }
                }
            }
        }
    }
}
