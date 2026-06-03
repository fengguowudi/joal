//! Minimal BEP-3 [bencode] implementation.
//!
//! # Why not pure `serde_bencode`?
//!
//! `info_hash` (BEP-3) is the **SHA-1 of the raw bytes of the `info` dictionary
//! as they appear in the `.torrent` file** — not a re-encoded version. Decoding
//! to a data structure and re-encoding risks losing byte-for-byte fidelity
//! (dictionary key ordering, integer formatting, trailing zeroes, etc.), which
//! would produce an `info_hash` that differs from what every real BitTorrent
//! tracker and client expects.
//!
//! This module therefore ships a tiny recursive-descent **scanner** that walks
//! a bencode payload, tracks byte offsets, and can hand back a subslice of the
//! original input. Paired with [`Value`] for decoding tracker responses and
//! small auxiliary files, it fully replaces the Java project's use of
//! `com.turn.ttorrent.bcodec` with ~300 lines of safe, zero-copy Rust.
//!
//! [bencode]: https://wiki.theory.org/BitTorrentSpecification#Bencoding

use std::borrow::Cow;
use std::collections::BTreeMap;

/// All errors that can be returned while parsing bencode data.
#[derive(Debug, thiserror::Error)]
pub enum BencodeError {
    #[error("unexpected end of input at offset {0}")]
    UnexpectedEof(usize),
    #[error("invalid byte {byte:#x} at offset {offset}: expected {expected}")]
    Unexpected {
        offset: usize,
        byte: u8,
        expected: &'static str,
    },
    #[error("byte-string length at offset {offset} is not valid ASCII digits")]
    InvalidLength { offset: usize },
    #[error("integer at offset {offset} is not a valid signed decimal")]
    InvalidInteger { offset: usize },
    #[error("integer at offset {offset} has a leading zero or `-0`")]
    IllegalIntegerEncoding { offset: usize },
    #[error("byte-string at offset {offset} runs past end of input")]
    TruncatedString { offset: usize },
    #[error("dictionary key at offset {offset} is not a byte string")]
    DictKeyNotString { offset: usize },
    #[error("dictionary keys are not in strictly ascending order at offset {offset}")]
    DictUnordered { offset: usize },
    #[error("top-level bencode value at offset {offset} is not a dictionary")]
    TopLevelNotDict { offset: usize },
    #[error("required key `{key}` missing from bencode dictionary")]
    MissingKey { key: &'static str },
    #[error("bencode value for key `{key}` has wrong type (expected {expected})")]
    WrongType {
        key: &'static str,
        expected: &'static str,
    },
    #[error("trailing bytes after bencode value at offset {offset}")]
    TrailingBytes { offset: usize },
}

/// Owned representation of an arbitrary bencode value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Integer(i64),
    ByteString(Vec<u8>),
    List(Vec<Value>),
    /// Dictionaries are stored in a [`BTreeMap`] so iteration yields keys in
    /// the canonical lexicographic order required by BEP-3. Parsing rejects
    /// out-of-order input rather than silently fixing it — that way this type
    /// can never misrepresent what was on the wire.
    Dict(BTreeMap<Vec<u8>, Value>),
}

impl Value {
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Integer(i) => Some(*i),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::ByteString(b) => Some(b),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<Cow<'_, str>> {
        self.as_bytes().map(String::from_utf8_lossy)
    }

    #[must_use]
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(items) => Some(items),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, Value>> {
        match self {
            Value::Dict(map) => Some(map),
            _ => None,
        }
    }

    /// Case-sensitive dictionary lookup using an ASCII key.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_dict().and_then(|d| d.get(key.as_bytes()))
    }
}

/// Decode a complete bencode payload. Rejects trailing garbage.
/// Enforces strictly ascending dictionary key order per BEP-3.
pub fn parse(input: &[u8]) -> Result<Value, BencodeError> {
    let mut parser = Parser::new(input, true);
    let value = parser.parse_value()?;
    if parser.pos != input.len() {
        return Err(BencodeError::TrailingBytes { offset: parser.pos });
    }
    Ok(value)
}

/// Decode a complete bencode payload without enforcing dictionary key ordering.
/// Use for tracker responses which may not comply with BEP-3 key ordering.
pub fn parse_lenient(input: &[u8]) -> Result<Value, BencodeError> {
    let mut parser = Parser::new(input, false);
    let value = parser.parse_value()?;
    if parser.pos != input.len() {
        return Err(BencodeError::TrailingBytes { offset: parser.pos });
    }
    Ok(value)
}

/// Scan a `.torrent` payload and return the raw byte range of its top-level
/// `info` dictionary, including the leading `d` and trailing `e`.
///
/// The SHA-1 of this slice is the `info_hash` required by BEP-3 and by every
/// tracker announce request.
pub fn extract_info_dict_bytes(torrent: &[u8]) -> Result<&[u8], BencodeError> {
    let mut parser = Parser::new(torrent, true);

    // Top level must be a dict.
    if parser.peek()? != b'd' {
        return Err(BencodeError::TopLevelNotDict { offset: 0 });
    }
    parser.pos += 1;

    while parser.peek()? != b'e' {
        let key = parser.parse_byte_string()?;
        let value_start = parser.pos;
        parser.skip_value()?;
        let value_end = parser.pos;
        if key == b"info" {
            return Ok(&torrent[value_start..value_end]);
        }
    }

    Err(BencodeError::MissingKey { key: "info" })
}

/// Typed accessor: extract `info` dict as raw bytes, returning a decoded
/// [`Value::Dict`] alongside. Handy when a caller needs both the hash source
/// and the parsed fields.
pub fn extract_info(torrent: &[u8]) -> Result<(&[u8], Value), BencodeError> {
    let raw = extract_info_dict_bytes(torrent)?;
    let value = parse(raw)?;
    Ok((raw, value))
}

// ---------------------------------------------------------------------------
//  Internal parser
// ---------------------------------------------------------------------------

struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
    strict_order: bool,
}

impl<'a> Parser<'a> {
    fn new(input: &'a [u8], strict_order: bool) -> Self {
        Self {
            input,
            pos: 0,
            strict_order,
        }
    }

    fn peek(&self) -> Result<u8, BencodeError> {
        self.input
            .get(self.pos)
            .copied()
            .ok_or(BencodeError::UnexpectedEof(self.pos))
    }

    fn advance(&mut self) -> Result<u8, BencodeError> {
        let b = self.peek()?;
        self.pos += 1;
        Ok(b)
    }

    fn expect(&mut self, tag: u8, expected: &'static str) -> Result<(), BencodeError> {
        let offset = self.pos;
        let b = self.advance()?;
        if b != tag {
            return Err(BencodeError::Unexpected {
                offset,
                byte: b,
                expected,
            });
        }
        Ok(())
    }

    fn parse_value(&mut self) -> Result<Value, BencodeError> {
        match self.peek()? {
            b'i' => self.parse_integer().map(Value::Integer),
            b'l' => self.parse_list().map(Value::List),
            b'd' => self.parse_dict().map(Value::Dict),
            b'0'..=b'9' => self.parse_byte_string().map(Value::ByteString),
            other => Err(BencodeError::Unexpected {
                offset: self.pos,
                byte: other,
                expected: "bencode value (`i`, `l`, `d`, or ASCII digit)",
            }),
        }
    }

    /// Advance past one bencode value without building an owned [`Value`].
    fn skip_value(&mut self) -> Result<(), BencodeError> {
        match self.peek()? {
            b'i' => {
                self.parse_integer()?;
            }
            b'0'..=b'9' => {
                drop(self.parse_byte_string()?);
            }
            b'l' => {
                self.pos += 1;
                while self.peek()? != b'e' {
                    self.skip_value()?;
                }
                self.pos += 1;
            }
            b'd' => {
                self.pos += 1;
                while self.peek()? != b'e' {
                    drop(self.parse_byte_string()?);
                    self.skip_value()?;
                }
                self.pos += 1;
            }
            other => {
                return Err(BencodeError::Unexpected {
                    offset: self.pos,
                    byte: other,
                    expected: "bencode value",
                });
            }
        }
        Ok(())
    }

    fn parse_integer(&mut self) -> Result<i64, BencodeError> {
        let start = self.pos;
        self.expect(b'i', "`i` (integer start)")?;
        let number_start = self.pos;
        while self.peek()? != b'e' {
            self.pos += 1;
        }
        let digits = &self.input[number_start..self.pos];
        self.pos += 1; // consume 'e'

        let s = std::str::from_utf8(digits).map_err(|_| BencodeError::InvalidInteger {
            offset: number_start,
        })?;
        // BEP-3: reject `i-0e` and leading zeroes like `i03e` but allow `i0e`.
        let illegal = s == "-0"
            || (s.len() > 1 && s.starts_with('0'))
            || (s.len() > 2 && s.starts_with("-0"));
        if illegal {
            return Err(BencodeError::IllegalIntegerEncoding { offset: start });
        }
        s.parse::<i64>().map_err(|_| BencodeError::InvalidInteger {
            offset: number_start,
        })
    }

    fn parse_byte_string(&mut self) -> Result<Vec<u8>, BencodeError> {
        let len_start = self.pos;
        // Accumulate ASCII digits until ':'.
        let mut len_digits = 0usize;
        while self.peek()? != b':' {
            if !self.input[self.pos].is_ascii_digit() {
                return Err(BencodeError::InvalidLength { offset: len_start });
            }
            self.pos += 1;
            len_digits += 1;
        }
        if len_digits == 0 {
            return Err(BencodeError::InvalidLength { offset: len_start });
        }
        let len_str = std::str::from_utf8(&self.input[len_start..self.pos])
            .map_err(|_| BencodeError::InvalidLength { offset: len_start })?;
        let len: usize = len_str
            .parse()
            .map_err(|_| BencodeError::InvalidLength { offset: len_start })?;
        self.pos += 1; // consume ':'

        let end = self
            .pos
            .checked_add(len)
            .ok_or(BencodeError::TruncatedString { offset: len_start })?;
        if end > self.input.len() {
            return Err(BencodeError::TruncatedString { offset: len_start });
        }
        let bytes = self.input[self.pos..end].to_vec();
        self.pos = end;
        Ok(bytes)
    }

    fn parse_list(&mut self) -> Result<Vec<Value>, BencodeError> {
        self.expect(b'l', "`l` (list start)")?;
        let mut items = Vec::new();
        while self.peek()? != b'e' {
            items.push(self.parse_value()?);
        }
        self.pos += 1; // consume 'e'
        Ok(items)
    }

    fn parse_dict(&mut self) -> Result<BTreeMap<Vec<u8>, Value>, BencodeError> {
        self.expect(b'd', "`d` (dictionary start)")?;
        let mut map = BTreeMap::new();
        let mut last_key: Option<Vec<u8>> = None;
        while self.peek()? != b'e' {
            let key_offset = self.pos;
            if !self.input[self.pos].is_ascii_digit() {
                return Err(BencodeError::DictKeyNotString { offset: key_offset });
            }
            let key = self.parse_byte_string()?;
            if self.strict_order
                && let Some(prev) = &last_key
                && key.as_slice() <= prev.as_slice()
            {
                return Err(BencodeError::DictUnordered { offset: key_offset });
            }
            let value = self.parse_value()?;
            last_key = Some(key.clone());
            map.insert(key, value);
        }
        self.pos += 1; // consume 'e'
        Ok(map)
    }
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------
