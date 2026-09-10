use perex::input::{EncodingError, Input};
use perex::span::Span;

fn point_at(units: &[u16], at: usize) -> Option<(u32, usize)> {
    let &first = units.get(at)?;
    match units.get(at + 1) {
        Some(&second)
            if (0xd800..0xdc00).contains(&first) && (0xdc00..0xe000).contains(&second) =>
        {
            Some((
                0x10000 + ((u32::from(first) - 0xd800) * 1024) + u32::from(second) - 0xdc00,
                2,
            ))
        }
        _ => Some((u32::from(first), 1)),
    }
}

fn point_before(units: &[u16], at: usize) -> Option<(u32, usize)> {
    if at == 0 {
        return None;
    }
    if at >= 2
        && let Some((point, 2)) = point_at(&units[..at], at - 2)
    {
        return Some((point, 2));
    }
    Some((u32::from(units[at - 1]), 1))
}

// Independent test encoding from numeric code points, including surrogate values.
fn encode(point: u32, bytes: &mut Vec<u8>) {
    if point < 0x80 {
        bytes.push(point as u8);
    } else if point < 0x800 {
        bytes.extend([0xc0 | (point >> 6) as u8, 0x80 | (point & 63) as u8]);
    } else if point < 0x10000 {
        bytes.extend([
            0xe0 | (point >> 12) as u8,
            0x80 | ((point >> 6) & 63) as u8,
            0x80 | (point & 63) as u8,
        ]);
    } else {
        bytes.extend([
            0xf0 | (point >> 18) as u8,
            0x80 | ((point >> 12) & 63) as u8,
            0x80 | ((point >> 6) & 63) as u8,
            0x80 | (point & 63) as u8,
        ]);
    }
}

fn encode_units(units: &[u16], combine: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut at = 0;
    while at < units.len() {
        let (point, step) = if combine {
            point_at(units, at).unwrap()
        } else {
            (u32::from(units[at]), 1)
        };
        encode(point, &mut bytes);
        at += step;
    }
    bytes
}

fn assert_walks(input: Input<'_>, expected: &[u16]) {
    assert_eq!(input.len_utf16(), expected.len());
    assert_eq!(input.is_empty(), expected.is_empty());
    assert!(input.cursor_at(expected.len() + 1).is_none());
    assert!(input.cursor_at(usize::MAX).is_none());
    for at in 0..=expected.len() {
        let cursor = input.cursor_at(at).unwrap();
        assert_eq!(cursor.position(), at);
        let mut forward = cursor;
        for (index, &unit) in expected.iter().enumerate().skip(at) {
            assert_eq!(forward.next_unit(), Some(unit));
            assert_eq!(forward.position(), index + 1);
            assert_eq!(forward.previous_unit(), Some(unit));
            assert_eq!(forward.position(), index);
            assert_eq!(forward.next_unit(), Some(unit));
        }
        assert_eq!(forward.next_unit(), None);
        assert_eq!(forward.next_unit(), None);
        assert_eq!(forward.position(), expected.len());

        let mut backward = cursor;
        for index in (0..at).rev() {
            assert_eq!(backward.previous_unit(), Some(expected[index]));
            assert_eq!(backward.position(), index);
            assert_eq!(backward.next_unit(), Some(expected[index]));
            assert_eq!(backward.position(), index + 1);
            assert_eq!(backward.previous_unit(), Some(expected[index]));
        }
        assert_eq!(backward.previous_unit(), None);
        assert_eq!(backward.previous_unit(), None);
        assert_eq!(backward.position(), 0);

        let mut forward = cursor;
        let mut index = at;
        while let Some((point, step)) = point_at(expected, index) {
            assert_eq!(forward.next_point(), Some(point));
            index += step;
            assert_eq!(forward.position(), index);
        }
        assert_eq!(forward.next_point(), None);
        let mut backward = cursor;
        let mut index = at;
        while let Some((point, step)) = point_before(expected, index) {
            assert_eq!(backward.previous_point(), Some(point));
            index -= step;
            assert_eq!(backward.position(), index);
        }
        assert_eq!(backward.previous_point(), None);

        let mut normalized = cursor;
        let mid_pair = at > 0
            && at < expected.len()
            && (0xd800..0xdc00).contains(&expected[at - 1])
            && (0xdc00..0xe000).contains(&expected[at]);
        assert_eq!(normalized.normalize_unicode_start(), mid_pair);
        assert_eq!(normalized.position(), at - usize::from(mid_pair));
        assert!(!normalized.normalize_unicode_start());
    }
}

#[test]
fn all_boundary_encodings_walk_in_both_directions() {
    let units = [
        0, 0x7f, 0x80, 0x7ff, 0x800, 0xd7ff, 0xd800, 0, 0xdc00, 0xe000, 0xffff, 0xd800, 0xdc00,
        0xdbff, 0xdfff, 0xd83d, 0xde00, 0xd800, 0xd800, 0xdc00, 0xdc00, 0x61,
    ];
    assert_walks(Input::utf16(&units), &units);
    for combine in [false, true] {
        let bytes = encode_units(&units, combine);
        assert_walks(Input::wtf8(&bytes).unwrap(), &units);
    }
    assert_walks(
        Input::utf8("\0\u{7f}\u{80}\u{7ff}\u{800}\u{ffff}😀\u{10ffff}"),
        &[
            0, 0x7f, 0x80, 0x7ff, 0x800, 0xffff, 0xd83d, 0xde00, 0xdbff, 0xdfff,
        ],
    );
}

#[test]
fn empty_and_ascii_inputs_keep_exact_byte_borrows() {
    for text in ["", "\0abc\x7f"] {
        let expected: Vec<u16> = text.encode_utf16().collect();
        for input in [Input::utf8(text), Input::wtf8(text.as_bytes()).unwrap()] {
            assert_eq!(input.ascii_bytes().unwrap().as_ptr(), text.as_ptr());
            assert_walks(input, &expected);
        }
        assert_walks(Input::utf16(&expected), &expected);
    }
    assert!(Input::utf8("é").ascii_bytes().is_none());
    assert!(Input::utf16(&[65]).ascii_bytes().is_none());
}

#[test]
fn every_code_point_and_surrogate_value_round_trips() {
    let mut bytes = Vec::with_capacity(4);
    for point in 0..=0x10ffff {
        bytes.clear();
        encode(point, &mut bytes);
        let input = Input::wtf8(&bytes).unwrap();
        let mut cursor = input.cursor();
        assert_eq!(cursor.next_point(), Some(point));
        assert_eq!(cursor.position(), if point > 0xffff { 2 } else { 1 });
        assert_eq!(cursor.next_point(), None);
        assert_eq!(cursor.previous_point(), Some(point));
        assert_eq!(cursor.position(), 0);
        assert_eq!(cursor.previous_point(), None);
        if let Some(scalar) = char::from_u32(point) {
            let mut buffer = [0; 2];
            let expected = scalar.encode_utf16(&mut buffer);
            let mut unit_cursor = input.cursor();
            for &unit in expected.iter() {
                assert_eq!(unit_cursor.next_unit(), Some(unit));
            }
            for &unit in expected.iter().rev() {
                assert_eq!(unit_cursor.previous_unit(), Some(unit));
            }
        }
    }
}

#[test]
fn structured_surrogate_sequences_match_the_unit_reference() {
    // Every four-unit word over this boundary alphabet, in canonical and
    // separate-surrogate encodings. Exercises pairing and direction reversals.
    let alphabet = [0x61, 0x800, 0xd800, 0xdbff, 0xdc00, 0xdfff];
    for mut code in 0..alphabet.len().pow(4) {
        let mut units = [0; 4];
        for unit in &mut units {
            *unit = alphabet[code % alphabet.len()];
            code /= alphabet.len();
        }
        assert_walks(Input::utf16(&units), &units);
        for combine in [true, false] {
            assert_walks(Input::wtf8(&encode_units(&units, combine)).unwrap(), &units);
        }
    }
}

#[test]
fn malformed_bytes_fail_without_replacement_characters() {
    let invalid: &[&[u8]] = &[
        &[0x80],
        &[0xbf],
        &[0xc0, 0x80],
        &[0xc1, 0xbf],
        &[0xc2],
        &[0xc2, 0x41],
        &[0xe0, 0x80, 0x80],
        &[0xe0, 0x9f, 0xbf],
        &[0xe1],
        &[0xe1, 0x80],
        &[0xe1, 0x80, 0x41],
        &[0xed, 0xa0],
        &[0xf0, 0x80, 0x80, 0x80],
        &[0xf0, 0x8f, 0xbf, 0xbf],
        &[0xf1],
        &[0xf1, 0x80],
        &[0xf1, 0x80, 0x80],
        &[0xf1, 0x80, 0x80, 0x41],
        &[0xf4, 0x90, 0x80, 0x80],
        &[0xf5, 0x80, 0x80, 0x80],
        &[0xff],
    ];
    for bytes in invalid {
        assert_eq!(
            Input::wtf8(bytes).unwrap_err(),
            EncodingError { byte_offset: 0 }
        );
        let mut prefixed = b"ok".to_vec();
        prefixed.extend_from_slice(bytes);
        assert_eq!(
            Input::wtf8(&prefixed).unwrap_err(),
            EncodingError { byte_offset: 2 }
        );
    }
}

#[test]
fn all_short_encodings_agree_with_strict_utf8_validation() {
    // Surrogate encodings need three bytes, so this entire domain equals UTF-8.
    for first in 0..=255 {
        let bytes = [first];
        assert_eq!(
            Input::wtf8(&bytes).is_ok(),
            core::str::from_utf8(&bytes).is_ok()
        );
        for second in 0..=255 {
            let bytes = [first, second];
            assert_eq!(
                Input::wtf8(&bytes).is_ok(),
                core::str::from_utf8(&bytes).is_ok()
            );
        }
    }
}

#[test]
fn cursors_can_be_recreated_after_old_storage_is_destroyed() {
    let units = [0x61, 0xd83d, 0xde00, 0xd800, 0x62];
    for at in 0..=units.len() {
        let mut old = encode_units(&units, true);
        let new = old.clone();
        assert_ne!(old.as_ptr(), new.as_ptr());
        let saved_position = Input::wtf8(&old).unwrap().cursor_at(at).unwrap().position();
        // Rust ends the old borrow before mutation. Poisoning and freeing that
        // buffer would expose an accidental retained base in the resumed walk.
        old.fill(0xff);
        drop(old);
        let mut resumed = Input::wtf8(&new)
            .unwrap()
            .cursor_at(saved_position)
            .unwrap();
        for &unit in &units[at..] {
            assert_eq!(resumed.next_unit(), Some(unit));
        }
        assert_eq!(resumed.next_unit(), None);
    }
}

#[test]
fn nested_cursors_and_checkpoints_have_independent_state() {
    let subject = Input::utf8("a😀b");
    let mut outer = subject.cursor();
    assert_eq!(outer.next_unit(), Some(0x61));
    let checkpoint = outer;
    let mut inner = subject.cursor_at(3).unwrap();
    assert_eq!(inner.previous_point(), Some(0x1f600));
    assert_eq!(outer.next_unit(), Some(0xd83d));
    outer = checkpoint;
    assert_eq!(outer.next_point(), Some(0x1f600));
    assert_eq!(inner.next_point(), Some(0x1f600));
    assert_eq!(outer.position(), 3);
}

#[test]
fn capture_spans_preserve_empty_unset_and_half_pairs_without_copying() {
    assert_eq!(Span::new(3, 2), None);
    let input = Input::utf8("😀x");
    assert!(Span::new(0, 4).unwrap().units(input).is_none());
    assert!(
        Span::new(usize::MAX, usize::MAX)
            .unwrap()
            .units(input)
            .is_none()
    );
    let high = Span::new(0, 1).unwrap();
    let low = Span::new(1, 2).unwrap();
    let empty = Span::new(1, 1).unwrap();
    assert_eq!(high.start(), 0);
    assert_eq!(high.end(), 1);
    assert_eq!(high.len(), 1);
    assert!(!high.is_empty());
    assert_eq!(high.units(input).unwrap().collect::<Vec<_>>(), [0xd83d]);
    assert_eq!(low.units(input).unwrap().collect::<Vec<_>>(), [0xde00]);
    assert!(empty.is_empty());
    assert_ne!(Some(empty), None);
    let mut units = low.units(input).unwrap();
    assert_eq!(units.size_hint(), (1, Some(1)));
    assert_eq!(units.next(), Some(0xde00));
    assert_eq!(units.len(), 0);
    assert_eq!(units.next(), None);
    assert_eq!(units.next(), None);
    assert_eq!(empty.units(input).unwrap().next(), None);
    assert_eq!(Span::new(3, 3).unwrap().units(input).unwrap().next(), None);
}
