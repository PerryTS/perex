use std::cell::{Cell, RefCell};
use std::convert::Infallible;

use perex::{
    Budget,
    binding::{BoundSubject, ImmutableSubject, Subject},
    span::{BoundSpan, ReadError, ReadProgress, Span},
};

#[derive(Debug)]
enum Storage {
    Bytes(Vec<u8>),
    Units(Vec<u16>),
}

#[derive(Debug)]
struct Moving {
    storage: RefCell<Storage>,
    calls: Cell<usize>,
    unavailable: Cell<bool>,
}

impl Moving {
    fn new(storage: Storage) -> Self {
        Self {
            storage: RefCell::new(storage),
            calls: Cell::new(0),
            unavailable: Cell::new(false),
        }
    }

    fn relocate(&self) {
        let mut storage = self.storage.borrow_mut();
        match &mut *storage {
            Storage::Bytes(bytes) => {
                let replacement = bytes.clone();
                assert_ne!(replacement.as_ptr(), bytes.as_ptr());
                bytes.fill(0xff);
                *bytes = replacement;
            }
            Storage::Units(units) => {
                let replacement = units.clone();
                assert_ne!(replacement.as_ptr(), units.as_ptr());
                units.fill(0xffff);
                *units = replacement;
            }
        }
    }
}

impl ImmutableSubject for Moving {
    type Error = &'static str;
    fn with_subject<T>(&self, f: impl FnOnce(Subject<'_>) -> T) -> Result<T, Self::Error> {
        self.calls.set(self.calls.get() + 1);
        if self.unavailable.get() {
            return Err("owner unavailable");
        }
        let storage = self.storage.borrow();
        Ok(match &*storage {
            Storage::Bytes(bytes) => f(Subject::Wtf8(bytes)),
            Storage::Units(units) => f(Subject::Utf16(units)),
        })
    }
}

fn all_spans(storage: Storage, expected: &[u16], direct_seek: bool) {
    let owner = Moving::new(storage);
    let subject = BoundSubject::new(&owner).unwrap();
    for start in 0..=expected.len() {
        for end in start..=expected.len() {
            for quantum in [1, 2, 7, 1000] {
                let span = Span::new(start, end).unwrap();
                let mut reader = BoundSpan::new(&subject, span).unwrap();
                let mut output = Vec::with_capacity(span.len());
                let mut budget = Budget::new(10_000);
                loop {
                    let before = budget.remaining();
                    let status = reader
                        .try_fold(quantum, &mut budget, |unit| {
                            output.push(unit);
                            Ok::<_, Infallible>(())
                        })
                        .unwrap();
                    assert!(before - budget.remaining() <= quantum);
                    if status == ReadProgress::Complete {
                        break;
                    }
                    assert!(budget.remaining() < before, "Pending must make progress");
                    owner.relocate();
                }
                assert_eq!(output, expected[start..end]);
                let seek = if direct_seek || span.is_empty() {
                    0
                } else {
                    start.min(expected.len() - start)
                };
                assert_eq!(10_000 - budget.remaining(), seek + span.len());
                let calls = owner.calls.get();
                let remaining = budget.remaining();
                assert_eq!(
                    reader
                        .try_fold(1, &mut budget, |_| -> Result<(), Infallible> {
                            panic!("completed reader invoked consumer")
                        })
                        .unwrap(),
                    ReadProgress::Complete
                );
                assert_eq!(owner.calls.get(), calls);
                assert_eq!(budget.remaining(), remaining);
            }
        }
    }
}

#[test]
fn every_span_preserves_units_and_bounded_work_after_relocation() {
    let expected = [
        0x61, 0xd83d, 0xde00, 0xd800, 0x62, 0xdc00, 0x00, 0xe9, 0x20ac,
    ];
    all_spans(
        Storage::Bytes(
            b"a\xf0\x9f\x98\x80\xed\xa0\x80b\xed\xb0\x80\0\xc3\xa9\xe2\x82\xac".to_vec(),
        ),
        &expected,
        false,
    );
    all_spans(
        Storage::Bytes(
            b"a\xed\xa0\xbd\xed\xb8\x80\xed\xa0\x80b\xed\xb0\x80\0\xc3\xa9\xe2\x82\xac".to_vec(),
        ),
        &expected,
        false,
    );
    all_spans(Storage::Units(expected.to_vec()), &expected, true);
    all_spans(
        Storage::Bytes(b"abcdefghij".to_vec()),
        &b"abcdefghij".map(u16::from),
        true,
    );
}

#[test]
fn initial_seek_can_pause_without_delivering_units() {
    let text = "é".repeat(100);
    let owner = Moving::new(Storage::Bytes(text.into_bytes()));
    let subject = BoundSubject::new(&owner).unwrap();
    for start in [40, 60] {
        let mut reader = BoundSpan::new(&subject, Span::new(start, start + 1).unwrap()).unwrap();
        let mut budget = Budget::new(41);
        for _ in 0..40 {
            assert_eq!(
                reader
                    .try_fold(1, &mut budget, |_| -> Result<(), Infallible> {
                        panic!("seeking must not deliver a unit")
                    })
                    .unwrap(),
                ReadProgress::Pending
            );
            owner.relocate();
        }
        let mut seen = 0;
        assert_eq!(
            reader
                .try_fold(1, &mut budget, |unit| {
                    assert_eq!(unit, 0xe9);
                    seen += 1;
                    Ok::<_, Infallible>(())
                })
                .unwrap(),
            ReadProgress::Complete
        );
        assert_eq!(seen, 1);
        assert_eq!(budget.remaining(), 0);
    }
}

#[test]
fn exhaustion_consumer_failure_and_unwind_cannot_replay_partial_output() {
    let owner = Moving::new(Storage::Bytes(b"abcdef".to_vec()));
    let subject = BoundSubject::new(&owner).unwrap();
    for mode in [0, 1, 2, 3] {
        let mut reader = BoundSpan::new(&subject, Span::new(1, 5).unwrap()).unwrap();
        let mut budget = Budget::new(if mode == 0 { 1 } else { 100 });
        let mut seen = 0;
        if mode == 3 {
            owner.unavailable.set(true);
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            reader.try_fold(10, &mut budget, |_| {
                seen += 1;
                if mode == 1 {
                    return Err("consumer failed");
                }
                if mode == 2 {
                    panic!("consumer unwound");
                }
                Ok(())
            })
        }));
        match mode {
            0 => assert!(matches!(result, Ok(Err(ReadError::WorkLimit)))),
            1 => assert!(matches!(
                result,
                Ok(Err(ReadError::Consumer("consumer failed")))
            )),
            2 => assert!(result.is_err()),
            _ => assert!(matches!(result, Ok(Err(ReadError::Subject(_))))),
        }
        assert_eq!(seen, if mode == 3 { 0 } else { 1 });
        owner.unavailable.set(false);
        owner.relocate();
        let calls = owner.calls.get();
        assert!(matches!(
            reader.try_fold(1, &mut Budget::new(100), |_| Ok::<_, Infallible>(())),
            Err(ReadError::Failed)
        ));
        assert_eq!(owner.calls.get(), calls);
    }
}

#[test]
fn bounds_empty_spans_and_invalid_quantum_are_explicit() {
    let owner = Moving::new(Storage::Bytes(b"abc".to_vec()));
    let subject = BoundSubject::new(&owner).unwrap();
    assert!(matches!(
        BoundSpan::new(&subject, Span::new(3, 4).unwrap()),
        Err(ReadError::InvalidSpan)
    ));
    assert!(matches!(
        BoundSpan::new(&subject, Span::new(4, 4).unwrap()),
        Err(ReadError::InvalidSpan)
    ));
    let mut reader = BoundSpan::new(&subject, Span::new(3, 3).unwrap()).unwrap();
    let calls = owner.calls.get();
    assert!(matches!(
        reader.try_fold(0, &mut Budget::new(0), |_| Ok::<_, Infallible>(())),
        Err(ReadError::InvalidQuantum)
    ));
    assert_eq!(
        reader
            .try_fold(1, &mut Budget::new(0), |_| -> Result<(), Infallible> {
                panic!("empty span invoked consumer")
            })
            .unwrap(),
        ReadProgress::Complete
    );
    assert_eq!(owner.calls.get(), calls);
}
