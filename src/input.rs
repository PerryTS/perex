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
    ascii: bool,
}

impl<'a> Input<'a> {
    /// Borrow UTF-16 code units, including unpaired surrogates, in constant time.
    pub fn utf16(units: &'a [u16]) -> Self {
        Self {
            storage: Storage::Units(units),
            utf16_len: units.len(),
            ascii: false,
        }
    }

    /// Borrow valid UTF-8. Counts UTF-16 units in one pass without copying.
    pub fn utf8(text: &'a str) -> Self {
        let utf16_len = text.chars().map(char::len_utf16).sum();
        Self {
            storage: Storage::Bytes(text.as_bytes()),
            utf16_len,
            ascii: utf16_len == text.len(),
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
            storage: Storage::Bytes(bytes),
            utf16_len,
            ascii: utf16_len == bytes.len(),
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
            Storage::Bytes(bytes) if self.ascii => Some(bytes),
            _ => None,
        }
    }

    // Byte optimizations must separately prove that their predicate cannot
    // match inside a multibyte encoding. This never constructs a new buffer.
    pub(crate) fn original_bytes(self) -> Option<&'a [u8]> {
        match self.storage {
            Storage::Bytes(bytes) => Some(bytes),
            Storage::Units(_) => None,
        }
    }

    pub(crate) fn seek_work(self, position: usize) -> usize {
        if self.ascii || matches!(self.storage, Storage::Units(_)) {
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
            second_half: false,
        }
    }

    /// Starts at an exact UTF-16 boundary, including between surrogate halves.
    /// Returns `None` beyond the end, without clamping or Unicode normalization.
    ///
    /// O(1) for UTF-16 and ASCII. Other byte inputs walk from the nearer end in
    /// UTF-16 units, without constructing an index. Repeated random seeks need
    /// separate performance evaluation before choosing a host-owned index.
    pub fn cursor_at(self, utf16_offset: usize) -> Option<Cursor<'a>> {
        if utf16_offset > self.utf16_len {
            return None;
        }
        if matches!(self.storage, Storage::Units(_)) || self.ascii {
            return Some(Cursor {
                input: self,
                offset: utf16_offset,
                utf16_offset,
                second_half: false,
            });
        }
        let mut cursor = self.cursor();
        if utf16_offset <= self.utf16_len / 2 {
            while cursor.utf16_offset < utf16_offset {
                cursor.next_unit();
            }
        } else {
            cursor.offset = match self.storage {
                Storage::Bytes(bytes) => bytes.len(),
                Storage::Units(_) => unreachable!(),
            };
            cursor.utf16_offset = self.utf16_len;
            while cursor.utf16_offset > utf16_offset {
                cursor.previous_unit();
            }
        }
        Some(cursor)
    }
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
    // Byte offset for Bytes; unit offset for Units. When second_half is true,
    // this points to the four-byte scalar containing the next low surrogate.
    offset: usize,
    utf16_offset: usize,
    second_half: bool,
}

// Execution-local checkpoints contain no subject pointer. Only the evaluator
// may restore them, and only on the same immutable input borrow that made them.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Mark {
    offset: usize,
    units: usize,
    half: bool,
}

impl Cursor<'_> {
    pub(crate) fn mark(self) -> Mark {
        Mark {
            offset: self.offset,
            units: self.utf16_offset,
            half: self.second_half,
        }
    }
    pub(crate) fn restore(&mut self, mark: Mark) {
        self.offset = mark.offset;
        self.utf16_offset = mark.units;
        self.second_half = mark.half;
    }

    pub fn position(self) -> usize {
        self.utf16_offset
    }

    pub fn next_unit(&mut self) -> Option<u16> {
        let unit = match self.input.storage {
            Storage::Units(units) => {
                let unit = *units.get(self.offset)?;
                self.offset += 1;
                unit
            }
            Storage::Bytes(bytes) => {
                if self.offset == bytes.len() {
                    return None;
                }
                let (point, width) = decode_valid(bytes, self.offset);
                if point <= 0xffff {
                    self.offset += width;
                    point as u16
                } else if self.second_half {
                    self.second_half = false;
                    self.offset += width;
                    low_surrogate(point)
                } else {
                    self.second_half = true;
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
            Storage::Units(units) => {
                self.offset -= 1;
                units[self.offset]
            }
            Storage::Bytes(bytes) => {
                if self.second_half {
                    self.second_half = false;
                    high_surrogate(decode_valid(bytes, self.offset).0)
                } else {
                    self.offset -= 1;
                    while is_continuation(bytes[self.offset]) {
                        self.offset -= 1;
                    }
                    let point = decode_valid(bytes, self.offset).0;
                    if point > 0xffff {
                        self.second_half = true;
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
            Storage::Units(_) => self.next_unit()?,
            Storage::Bytes(bytes) => {
                if self.offset == bytes.len() {
                    return None;
                }
                let (point, width) = decode_valid(bytes, self.offset);
                self.offset += width;
                if self.second_half {
                    self.second_half = false;
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
            Storage::Units(_) => self.previous_unit()?,
            Storage::Bytes(bytes) => {
                if self.second_half {
                    self.second_half = false;
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
