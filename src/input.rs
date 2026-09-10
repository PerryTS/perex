//! Borrowed, lossless subject access. Positions always count UTF-16 code units.
//!
//! No constructor or cursor operation allocates. Byte input is validated once
//! when constructing a view; execution walks that view directly. In particular,
//! an astral UTF-8 character can be traversed as two individual surrogate units.

/// Invalid UTF-8/WTF-8, with the byte offset of the offending sequence's lead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncodingError {
    pub byte_offset: usize,
}

impl core::fmt::Display for EncodingError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "invalid WTF-8 at byte {}", self.byte_offset)
    }
}

impl core::error::Error for EncodingError {}

#[derive(Clone, Copy, Debug)]
enum Storage<'a> {
    Ascii(&'a [u8]),
    Bytes(&'a [u8]),
    Units(&'a [u16]),
}

/// A validated borrow of a subject, with no conversion buffer or owned storage.
///
/// Dropping the view releases only the borrow. The host retains ownership and
/// must end all such borrows before relocating or mutating the backing storage.
#[derive(Clone, Copy, Debug)]
pub struct Input<'a> {
    storage: Storage<'a>,
    utf16_len: usize,
}

impl<'a> Input<'a> {
    /// Borrow UTF-16 code units, including unpaired surrogates, in constant time.
    pub fn utf16(units: &'a [u16]) -> Self {
        Self {
            storage: Storage::Units(units),
            utf16_len: units.len(),
        }
    }

    /// Borrow valid UTF-8. Counts UTF-16 units in one pass without copying.
    pub fn utf8(text: &'a str) -> Self {
        let utf16_len = text.chars().map(char::len_utf16).sum();
        Self {
            storage: if utf16_len == text.len() {
                Storage::Ascii(text.as_bytes())
            } else {
                Storage::Bytes(text.as_bytes())
            },
            utf16_len,
        }
    }

    /// Validate and borrow UTF-8 extended with surrogate encodings.
    ///
    /// Accepts all canonical WTF-8 and also separately encoded adjacent high/low
    /// surrogates (as in CESU-8). Their logical UTF-16 units are identical to a
    /// four-byte scalar encoding. Rejects overlong, truncated, stray-continuation,
    /// and out-of-range encodings; never repairs input or inserts U+FFFD.
    /// Validation/counting is O(bytes) and uses constant storage.
    pub fn wtf8(bytes: &'a [u8]) -> Result<Self, EncodingError> {
        let mut offset = 0;
        let mut utf16_len = 0;
        while offset < bytes.len() {
            let (point, width) = decode_checked(bytes, offset)?;
            offset += width;
            utf16_len += if point > 0xffff { 2 } else { 1 };
        }
        Ok(Self {
            storage: if utf16_len == bytes.len() {
                Storage::Ascii(bytes)
            } else {
                Storage::Bytes(bytes)
            },
            utf16_len,
        })
    }

    pub fn len_utf16(self) -> usize {
        self.utf16_len
    }

    pub fn is_empty(self) -> bool {
        self.utf16_len == 0
    }

    /// Exposes the original bytes only when every code unit is ASCII.
    /// An evaluator may search this slice directly, retaining UTF-16 coordinates.
    /// UTF-16 storage returns `None`, even when all its units happen to be ASCII.
    pub fn ascii_bytes(self) -> Option<&'a [u8]> {
        match self.storage {
            Storage::Ascii(bytes) => Some(bytes),
            _ => None,
        }
    }

    // Byte optimizations must separately prove that their predicate cannot
    // match inside a multibyte encoding. This never constructs a new buffer.
    pub(crate) fn original_bytes(self) -> Option<&'a [u8]> {
        match self.storage {
            Storage::Ascii(bytes) | Storage::Bytes(bytes) => Some(bytes),
            Storage::Units(_) => None,
        }
    }

    pub(crate) fn shape(self) -> (u8, usize, usize) {
        let (kind, len) = match self.storage {
            Storage::Ascii(bytes) => (0, bytes.len()),
            Storage::Bytes(bytes) => (1, bytes.len()),
            Storage::Units(units) => (2, units.len()),
        };
        (kind, len, self.utf16_len)
    }

    // Only owner-bound validation may use this. The owner preserves the exact
    // immutable bytes from initial validation; shape is a cheap guard, not a
    // transferable validity certificate for arbitrary slices.
    pub(crate) fn reborrow_bytes(bytes: &'a [u8], layout: (u8, usize, usize)) -> Option<Self> {
        let (kind, len, utf16_len) = layout;
        if bytes.len() != len {
            return None;
        }
        let storage = match kind {
            0 => Storage::Ascii(bytes),
            1 => Storage::Bytes(bytes),
            _ => return None,
        };
        Some(Self { storage, utf16_len })
    }

    // Resumption is internal and requires a Resources owner that keeps the
    // exact immutable representation alive. Cheap checks reject incompatible
    // layouts; they do not prove that unrelated slices have identical contents.
    pub(crate) fn resume_cursor(self, mark: Mark) -> Option<Cursor<'a>> {
        if mark.units > self.utf16_len {
            return None;
        }
        let offset = mark.offset & !HALF;
        let half = mark.offset & HALF != 0;
        match self.storage {
            Storage::Ascii(bytes) => {
                if half || offset != mark.units || offset > bytes.len() {
                    return None;
                }
            }
            Storage::Units(units) => {
                if half || offset != mark.units || offset > units.len() {
                    return None;
                }
            }
            Storage::Bytes(bytes) => {
                if offset > bytes.len()
                    || (offset == bytes.len()) != (mark.units == self.utf16_len)
                    || (offset == 0 && !half && mark.units != 0)
                    || (offset < bytes.len() && is_continuation(bytes[offset]))
                    || (half && (offset == bytes.len() || decode_valid(bytes, offset).0 <= 0xffff))
                {
                    return None;
                }
            }
        }
        let mut cursor = self.cursor();
        cursor.restore(mark);
        Some(cursor)
    }

    pub(crate) fn seek_work(self, position: usize) -> usize {
        if !matches!(self.storage, Storage::Bytes(_)) {
            1
        } else {
            position.min(self.utf16_len - position).saturating_add(1)
        }
    }

    pub fn cursor(self) -> Cursor<'a> {
        Cursor {
            input: self,
            offset: 0,
            utf16_offset: 0,
        }
    }

    /// Starts at an exact UTF-16 boundary, including between surrogate halves.
    /// Returns `None` beyond the end, without clamping or Unicode normalization.
    ///
    /// O(1) for UTF-16 and ASCII. Other byte inputs walk from the nearer end in
    /// UTF-16 units, without constructing an index. Repeated random seeks need
    /// separate performance evaluation before choosing a host-owned index.
    #[inline]
    pub fn cursor_at(self, utf16_offset: usize) -> Option<Cursor<'a>> {
        if utf16_offset > self.utf16_len {
            return None;
        }
        let offset = match self.storage {
            Storage::Bytes(bytes) if utf16_offset != 0 => {
                if utf16_offset == self.utf16_len {
                    bytes.len()
                } else {
                    seek_byte_offset(bytes, self.utf16_len, utf16_offset)
                }
            }
            _ => utf16_offset,
        };
        Some(Cursor {
            input: self,
            offset,
            utf16_offset,
        })
    }
}

// Keep the decoder loop out of the constant-time ASCII, UTF-16 and endpoint
// paths. Its register saves otherwise burden even calls that do not scan.
#[inline(never)]
fn seek_byte_offset(bytes: &[u8], utf16_len: usize, utf16_offset: usize) -> usize {
    // Decode each stored scalar once. Advancing unit-by-unit would decode
    // a four-byte scalar twice and repeatedly pack a temporary half-pair
    // position even when the destination is after the complete scalar.
    let mut offset = 0;
    let mut units = 0;
    if utf16_offset <= utf16_len / 2 {
        while units < utf16_offset {
            let (point, width) = decode_valid(bytes, offset);
            let count = if point > 0xffff { 2 } else { 1 };
            if count > utf16_offset - units {
                offset = pack_offset(offset, true);
                break;
            }
            offset += width;
            units += count;
        }
    } else {
        offset = bytes.len();
        units = utf16_len;
        while units > utf16_offset {
            offset -= 1;
            while is_continuation(bytes[offset]) {
                offset -= 1;
            }
            let point = decode_valid(bytes, offset).0;
            let count = if point > 0xffff { 2 } else { 1 };
            if count > units - utf16_offset {
                offset = pack_offset(offset, true);
                break;
            }
            units -= count;
        }
    }
    offset
}

/// A scoped cursor. Clone/copy it for constant-time checkpoints while the same
/// subject remains borrowed. It must not survive a relocation of that subject.
/// Across relocation save integer positions, release the cursor, and seek on a
/// newly borrowed input. That initial re-seek may be linear for non-ASCII bytes.
///
/// The backing bytes cannot be changed while a cursor is still in use:
///
/// ```compile_fail
/// use perex::input::Input;
/// let mut bytes = *b"abc";
/// let mut cursor = Input::wtf8(&bytes).unwrap().cursor();
/// bytes[0] = b'z';
/// assert_eq!(cursor.next_unit(), Some(u16::from(b'a')));
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Cursor<'a> {
    input: Input<'a>,
    // Byte offset for Bytes; unit offset for Units/Ascii. HALF marks a position
    // between the units of the four-byte scalar at the remaining offset bits.
    offset: usize,
    utf16_offset: usize,
}

// Execution-local checkpoints contain no subject pointer. Only the evaluator
// may restore them, and only on the same immutable input borrow that made them.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Mark {
    // A valid u8/u16 slice occupies at most isize::MAX bytes. Its endpoint
    // therefore leaves this bit free, without imposing a new input size limit.
    offset: usize,
    units: usize,
}
const HALF: usize = 1usize << (usize::BITS - 1);

fn pack_offset(offset: usize, half: bool) -> usize {
    debug_assert_eq!(offset & HALF, 0);
    offset | if half { HALF } else { 0 }
}

impl Cursor<'_> {
    pub(crate) fn mark(self) -> Mark {
        Mark {
            offset: self.offset,
            units: self.utf16_offset,
        }
    }
    pub(crate) fn restore(&mut self, mark: Mark) {
        self.offset = mark.offset;
        self.utf16_offset = mark.units;
    }

    pub fn position(self) -> usize {
        self.utf16_offset
    }

    pub fn next_unit(&mut self) -> Option<u16> {
        let unit = match self.input.storage {
            Storage::Ascii(bytes) => {
                let unit = u16::from(*bytes.get(self.offset)?);
                self.offset += 1;
                unit
            }
            Storage::Units(units) => {
                let unit = *units.get(self.offset)?;
                self.offset += 1;
                unit
            }
            Storage::Bytes(bytes) => {
                let offset = self.offset & !HALF;
                if offset == bytes.len() {
                    return None;
                }
                let (point, width) = decode_valid(bytes, offset);
                if point <= 0xffff {
                    self.offset = offset + width;
                    point as u16
                } else if self.offset & HALF != 0 {
                    self.offset = offset + width;
                    low_surrogate(point)
                } else {
                    self.offset = pack_offset(offset, true);
                    high_surrogate(point)
                }
            }
        };
        self.utf16_offset += 1;
        Some(unit)
    }

    pub fn previous_unit(&mut self) -> Option<u16> {
        if self.utf16_offset == 0 {
            return None;
        }
        let unit = match self.input.storage {
            Storage::Ascii(bytes) => {
                self.offset -= 1;
                u16::from(bytes[self.offset])
            }
            Storage::Units(units) => {
                self.offset -= 1;
                units[self.offset]
            }
            Storage::Bytes(bytes) => {
                if self.offset & HALF != 0 {
                    self.offset &= !HALF;
                    high_surrogate(decode_valid(bytes, self.offset).0)
                } else {
                    self.offset -= 1;
                    while is_continuation(bytes[self.offset]) {
                        self.offset -= 1;
                    }
                    let point = decode_valid(bytes, self.offset).0;
                    if point > 0xffff {
                        self.offset = pack_offset(self.offset, true);
                        low_surrogate(point)
                    } else {
                        point as u16
                    }
                }
            }
        };
        self.utf16_offset -= 1;
        Some(unit)
    }

    /// Consume one Unicode code point, preserving an unpaired surrogate as its
    /// numeric value. Pairing happens in logical UTF-16, independent of encoding.
    /// At the middle of a pair this consumes just the low surrogate; use
    /// `normalize_unicode_start` separately for RegExp search initialization.
    pub fn next_point(&mut self) -> Option<u32> {
        let first = match self.input.storage {
            Storage::Ascii(bytes) => {
                let point = u32::from(*bytes.get(self.offset)?);
                self.offset += 1;
                self.utf16_offset += 1;
                return Some(point);
            }
            Storage::Units(units) => {
                let first = *units.get(self.offset)?;
                self.offset += 1;
                self.utf16_offset += 1;
                if is_high(first)
                    && let Some(&second) = units.get(self.offset)
                    && is_low(second)
                {
                    self.offset += 1;
                    self.utf16_offset += 1;
                    return Some(combine_pair(first, second));
                }
                return Some(u32::from(first));
            }
            Storage::Bytes(bytes) => {
                let offset = self.offset & !HALF;
                if offset == bytes.len() {
                    return None;
                }
                let (point, width) = decode_valid(bytes, offset);
                let half = self.offset & HALF != 0;
                self.offset = offset + width;
                if half {
                    self.utf16_offset += 1;
                    return Some(u32::from(low_surrogate(point)));
                }
                if point > 0xffff {
                    // Consume a complete scalar once, without splitting it into
                    // surrogate units only to decode and combine it again.
                    self.utf16_offset += 2;
                    return Some(point);
                }
                self.utf16_offset += 1;
                point as u16
            }
        };
        if is_high(first) {
            let after_first = *self;
            if let Some(second) = self.next_unit()
                && is_low(second)
            {
                return Some(combine_pair(first, second));
            }
            *self = after_first;
        }
        Some(u32::from(first))
    }

    /// Consume one Unicode code point backwards. At the middle of a pair this
    /// consumes just the high surrogate. Ordinary Unicode matching starts on
    /// point boundaries; the exact-unit cursor also serves non-Unicode patterns.
    pub fn previous_point(&mut self) -> Option<u32> {
        if self.utf16_offset == 0 {
            return None;
        }
        let last = match self.input.storage {
            Storage::Ascii(bytes) => {
                self.offset -= 1;
                self.utf16_offset -= 1;
                return Some(u32::from(bytes[self.offset]));
            }
            Storage::Units(units) => {
                self.offset -= 1;
                self.utf16_offset -= 1;
                let last = units[self.offset];
                if is_low(last) && self.offset > 0 {
                    let first = units[self.offset - 1];
                    if is_high(first) {
                        self.offset -= 1;
                        self.utf16_offset -= 1;
                        return Some(combine_pair(first, last));
                    }
                }
                return Some(u32::from(last));
            }
            Storage::Bytes(bytes) => {
                if self.offset & HALF != 0 {
                    self.offset &= !HALF;
                    self.utf16_offset -= 1;
                    return Some(u32::from(high_surrogate(
                        decode_valid(bytes, self.offset).0,
                    )));
                }
                self.offset -= 1;
                while is_continuation(bytes[self.offset]) {
                    self.offset -= 1;
                }
                let point = decode_valid(bytes, self.offset).0;
                if point > 0xffff {
                    self.utf16_offset -= 2;
                    return Some(point);
                }
                self.utf16_offset -= 1;
                point as u16
            }
        };
        if is_low(last) {
            let before_last = *self;
            if let Some(first) = self.previous_unit()
                && is_high(first)
            {
                return Some(combine_pair(first, last));
            }
            *self = before_last;
        }
        Some(u32::from(last))
    }

    /// Move back one unit only when positioned between a high/low pair.
    /// ECMAScript Unicode-mode RegExp initialization uses the containing code
    /// point for such a start. This is not empty-match AdvanceStringIndex.
    /// Returns whether the position changed.
    pub fn normalize_unicode_start(&mut self) -> bool {
        let mut before = *self;
        let mut after = *self;
        if let Some(high) = before.previous_unit()
            && is_high(high)
            && let Some(low) = after.next_unit()
            && is_low(low)
        {
            *self = before;
            return true;
        }
        false
    }
}

fn is_high(unit: u16) -> bool {
    (0xd800..=0xdbff).contains(&unit)
}

fn is_low(unit: u16) -> bool {
    (0xdc00..=0xdfff).contains(&unit)
}

fn high_surrogate(point: u32) -> u16 {
    0xd800 | ((point - 0x10000) >> 10) as u16
}

fn low_surrogate(point: u32) -> u16 {
    0xdc00 | ((point - 0x10000) & 0x3ff) as u16
}

fn combine_pair(high: u16, low: u16) -> u32 {
    0x10000 + (u32::from(high - 0xd800) << 10) + u32::from(low - 0xdc00)
}

fn is_continuation(byte: u8) -> bool {
    byte & 0xc0 == 0x80
}

fn decode_checked(bytes: &[u8], offset: usize) -> Result<(u32, usize), EncodingError> {
    let error = EncodingError {
        byte_offset: offset,
    };
    let lead = bytes[offset];
    let (width, minimum, mask) = match lead {
        0..=0x7f => return Ok((u32::from(lead), 1)),
        0xc2..=0xdf => (2, 0x80, 0x1f),
        0xe0..=0xef => (3, 0x800, 0x0f),
        0xf0..=0xf4 => (4, 0x10000, 0x07),
        _ => return Err(error),
    };
    let sequence = bytes
        .get(offset..)
        .and_then(|tail| tail.get(..width))
        .ok_or(error)?;
    let mut point = u32::from(lead & mask);
    for &byte in &sequence[1..] {
        if !is_continuation(byte) {
            return Err(error);
        }
        point = (point << 6) | u32::from(byte & 0x3f);
    }
    if point < minimum || point > 0x10ffff {
        return Err(error);
    }
    Ok((point, width))
}

// Only called on a validated view at a lead-byte boundary. Kept separate from
// validation so advancing a cursor does not repeat encoding checks.
fn decode_valid(bytes: &[u8], offset: usize) -> (u32, usize) {
    let lead = bytes[offset];
    if lead < 0x80 {
        return (u32::from(lead), 1);
    }
    let (width, mask) = if lead < 0xe0 {
        (2, 0x1f)
    } else if lead < 0xf0 {
        (3, 0x0f)
    } else {
        (4, 0x07)
    };
    let mut point = u32::from(lead & mask);
    for &byte in &bytes[offset + 1..offset + width] {
        point = (point << 6) | u32::from(byte & 0x3f);
    }
    (point, width)
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;

    #[test]
    fn packing_preserves_every_offset_bit_up_to_a_valid_slice_endpoint() {
        for half in [false, true] {
            for offset in [0, 1, HALF - 2, HALF - 1]
                .into_iter()
                .chain((0..usize::BITS - 1).map(|bit| 1usize << bit))
            {
                let mark = Mark {
                    offset: pack_offset(offset, half),
                    units: offset,
                };
                assert_eq!(mark.offset & !HALF, offset);
                assert_eq!(mark.offset & HALF != 0, half);
                assert_eq!(mark.units, offset);
            }
        }
        assert_eq!(
            core::mem::size_of::<Mark>(),
            2 * core::mem::size_of::<usize>()
        );
    }

    #[test]
    fn restoring_each_boundary_preserves_forward_and_reverse_surrogate_reads() {
        let units = [0x61, 0xd83d, 0xde00, 0x62];
        let cesu = [0x61, 0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80, 0x62];
        let ascii = [0x61, 0x62, 0x63];
        for (input, expected) in [
            (Input::utf8("a😀b"), units.as_slice()),
            (Input::wtf8(&cesu).unwrap(), units.as_slice()),
            (Input::utf16(&units), units.as_slice()),
            (Input::utf8("abc"), ascii.as_slice()),
        ] {
            for at in 0..=expected.len() {
                let mut cursor = input.cursor_at(at).unwrap();
                let mark = cursor.mark();
                while cursor.next_point().is_some() {}
                cursor.restore(mark);
                assert_eq!(cursor.position(), at);
                for &unit in &expected[at..] {
                    assert_eq!(cursor.next_unit(), Some(unit));
                }
                assert_eq!(cursor.next_unit(), None);
                cursor.restore(mark);
                for &unit in expected[..at].iter().rev() {
                    assert_eq!(cursor.previous_unit(), Some(unit));
                }
                assert_eq!(cursor.previous_unit(), None);
                cursor.restore(mark);
                for point in core::char::decode_utf16(expected[at..].iter().copied()) {
                    let value =
                        point.map_or_else(|e| u32::from(e.unpaired_surrogate()), |c| c as u32);
                    assert_eq!(cursor.next_point(), Some(value));
                }
                assert_eq!(cursor.next_point(), None);
                let mut points = [0; 4];
                let mut length = 0;
                for point in core::char::decode_utf16(expected[..at].iter().copied()) {
                    points[length] =
                        point.map_or_else(|e| u32::from(e.unpaired_surrogate()), |c| c as u32);
                    length += 1;
                }
                cursor.restore(mark);
                for &point in points[..length].iter().rev() {
                    assert_eq!(cursor.previous_point(), Some(point));
                }
                assert_eq!(cursor.previous_point(), None);
            }
        }
    }
}
