use perex::{
    Budget,
    compiler::{CompileError, Node, Range, compile, prepare},
    executor::{Frame, Scratch, Undo, find},
    input::Input,
    program::{Program, ProgramError},
    span::Span,
};

#[test]
fn source_and_flags_can_be_poisoned_and_freed_before_final_allocation() {
    let mut nodes = vec![Node::default(); 256];
    let mut ranges = vec![Range::default(); 256];
    let mut budget = Budget::new(100_000);
    let plan = {
        let mut source = b"(?<x>".to_vec();
        source.extend_from_slice(&[0xed, 0xa0, 0x80]); // original lone high surrogate
        source.extend_from_slice(b")\\k<x>");
        let mut flags = String::from("u");
        let plan = prepare(
            Input::wtf8(&source).unwrap(),
            &flags,
            &mut nodes,
            &mut ranges,
            &mut budget,
        )
        .unwrap();
        source.fill(0xff);
        flags.clear();
        drop(source);
        drop(flags);
        plan
    };
    let required = plan.required_words();
    let mut output = vec![0; required];
    let p = plan.emit(&mut output).unwrap();
    assert_eq!(p.words().len(), required);
    // Program's lifetime must not keep any arena or compile-budget borrow.
    nodes.fill(Node::default());
    ranges.fill(Range::default());
    drop(nodes);
    drop(ranges);
    let _spent = 100_000 - budget.remaining();
    let subject = [0xed, 0xa0, 0x80, 0xed, 0xa0, 0x80];
    let mut registers = vec![0; p.register_count()];
    let mut frames = vec![Frame::default(); 32];
    let mut undo = vec![Undo::default(); 64];
    let mut captures = vec![None; p.capture_count()];
    assert!(
        find(
            p,
            Input::wtf8(&subject).unwrap(),
            0,
            Scratch {
                registers: &mut registers,
                frames: &mut frames,
                undo: &mut undo,
            },
            &mut captures,
            &mut Budget::new(1000),
        )
        .unwrap()
    );
    assert_eq!(captures, [Span::new(0, 2), Span::new(0, 1)]);
}

#[test]
fn final_program_outlives_all_compile_scratch() {
    let mut output = vec![0; 2048];
    let program = {
        let mut nodes = vec![Node::default(); 256];
        let mut ranges = vec![Range::default(); 256];
        let mut budget = Budget::new(100_000);
        let plan = prepare(
            Input::utf8("(?:(?<x>a)|(?<x>b))+\\k<x>"),
            "u",
            &mut nodes,
            &mut ranges,
            &mut budget,
        )
        .unwrap();
        let required = plan.required_words();
        let result = plan.emit(&mut output).unwrap();
        assert_eq!(result.words().len(), required);
        result
    };
    assert_eq!(program.capture_count(), 3);
    assert!(Program::from_words(program.words(), &mut Budget::new(100_000)).is_ok());
}

#[test]
fn dropping_a_plan_releases_scratch_without_replenishing_work() {
    let mut nodes = [Node::default(); 64];
    let mut ranges = [Range::default(); 64];
    let mut budget = Budget::new(1000);
    let plan = prepare(Input::utf8("a+"), "", &mut nodes, &mut ranges, &mut budget).unwrap();
    let remaining = plan.remaining_work();
    assert!(remaining < 1000);
    drop(plan);
    nodes.fill(Node::default());
    ranges.fill(Range::default());
    assert_eq!(budget.remaining(), remaining);
}

#[test]
fn storage_failure_consumes_plan_and_never_publishes_a_header() {
    for capacity in [0, 1, 3] {
        let mut nodes = [Node::default(); 32];
        let mut ranges = [Range::default(); 32];
        let mut budget = Budget::new(1000);
        let plan = prepare(Input::utf8("ab"), "", &mut nodes, &mut ranges, &mut budget).unwrap();
        let required_words = plan.required_words();
        let mut output = vec![0x5052_5831; capacity];
        assert_eq!(
            plan.emit(&mut output).unwrap_err(),
            CompileError::ProgramStorage { required_words }
        );
        if capacity != 0 {
            assert_eq!(output[0], 0);
            assert!(output[1..].iter().all(|&word| word == 0x5052_5831));
        }
        assert_eq!(
            Program::from_words(&output, &mut Budget::new(1000)).unwrap_err(),
            ProgramError::Invalid
        );
        nodes.fill(Node::default());
        ranges.fill(Range::default());
    }
}

#[test]
fn work_budget_spans_both_phases_and_late_failure_invalidates_output() {
    let source = "(?<x>a{2,4})(?i:b)\\k<x>";
    let mut nodes = [Node::default(); 256];
    let mut ranges = [Range::default(); 256];
    let mut output = [0; 2048];
    let mut budget = Budget::new(100_000);
    let expected = compile(
        Input::utf8(source),
        "u",
        &mut nodes,
        &mut ranges,
        &mut output,
        &mut budget,
    )
    .unwrap()
    .words()
    .to_vec();
    let needed = 100_000 - budget.remaining();
    for allowance in 0..=needed + 1 {
        let mut budget = Budget::new(allowance);
        output.fill(0x5052_5831);
        let result = match prepare(
            Input::utf8(source),
            "u",
            &mut nodes,
            &mut ranges,
            &mut budget,
        ) {
            Ok(plan) => {
                assert!(plan.remaining_work() < allowance);
                let result = plan.emit(&mut output).map(|p| p.words().to_vec());
                if result.is_err() {
                    assert_eq!(output[0], 0);
                }
                result
            }
            Err(error) => Err(error),
        };
        if allowance < needed {
            assert_eq!(result, Err(CompileError::WorkLimit));
            assert_eq!(budget.remaining(), 0);
        } else {
            assert_eq!(result.unwrap(), expected);
            assert_eq!(budget.remaining(), allowance - needed);
            assert!(output[expected.len()..].iter().all(|&v| v == 0x5052_5831));
        }
    }
    // The existing one-call API has the same late-failure publication rule.
    output.fill(0x5052_5831);
    assert_eq!(
        compile(
            Input::utf8(source),
            "u",
            &mut nodes,
            &mut ranges,
            &mut output,
            &mut Budget::new(needed - 1),
        )
        .unwrap_err(),
        CompileError::WorkLimit
    );
    assert_eq!(output[0], 0);
}
