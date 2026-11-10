//! Minecraft's NBT format: reading and writing it exactly, for the few world files the app edits
//! (`level.dat`, `data/minecraft/game_rules.dat`).
//!
//! A compound keeps its entries in file order, and strings are converted from and to Java's modified
//! UTF-8, so a file read and written back without changes comes out byte for byte the same (before
//! compression). Files are gzipped; one that isn't is read as it is.

use std::io::{Read, Write};

use flate2::{Compression, read::GzDecoder, write::GzEncoder};

/// Nesting deeper than this is refused. World settings files are a handful of levels deep, and a
/// malicious one mustn't exhaust the stack: the game's own limit, 512, overflowed a 2 MB thread stack in a
/// Windows debug build.
const MAX_DEPTH: usize = 64;
/// Larger files aren't world settings.
pub const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    /// The element type is kept for empty lists, which can't show it otherwise.
    List(u8, Vec<Tag>),
    Compound(Compound),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

/// Named entries, in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Compound(pub Vec<(String, Tag)>);

impl Compound {
    pub fn get(&self, name: &str) -> Option<&Tag> {
        self.0.iter().find(|(key, _)| key == name).map(|(_, tag)| tag)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Tag> {
        self.0.iter_mut().find(|(key, _)| key == name).map(|(_, tag)| tag)
    }

    /// Replaces an entry in place, or adds it at the end.
    pub fn set(&mut self, name: &str, tag: Tag) {
        match self.get_mut(name) {
            Some(old) => *old = tag,
            None => self.0.push((name.to_owned(), tag)),
        }
    }

    /// The compound at a path of names.
    pub fn compound_at(&self, path: &[&str]) -> Option<&Compound> {
        path.iter().try_fold(self, |at, name| match at.get(name)? {
            Tag::Compound(inner) => Some(inner),
            _ => None,
        })
    }

    pub fn compound_at_mut(&mut self, path: &[&str]) -> Option<&mut Compound> {
        path.iter().try_fold(self, |at, name| match at.get_mut(name)? {
            Tag::Compound(inner) => Some(inner),
            _ => None,
        })
    }
}

impl Tag {
    fn id(&self) -> u8 {
        match self {
            Self::Byte(_) => 1,
            Self::Short(_) => 2,
            Self::Int(_) => 3,
            Self::Long(_) => 4,
            Self::Float(_) => 5,
            Self::Double(_) => 6,
            Self::ByteArray(_) => 7,
            Self::String(_) => 8,
            Self::List(..) => 9,
            Self::Compound(_) => 10,
            Self::IntArray(_) => 11,
            Self::LongArray(_) => 12,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NbtError {
    #[error("the file ends too early")]
    Truncated,
    #[error("unknown tag type {0}")]
    UnknownTag(u8),
    #[error("the root isn't a compound")]
    NotACompound,
    #[error("it is nested too deeply")]
    TooDeep,
    #[error("a size in it is impossible")]
    BadSize,
    #[error("it is larger than {MAX_BYTES} bytes once unpacked")]
    TooLarge,
    #[error("it can't be unpacked: {0}")]
    Gzip(String),
    #[error("a string in it isn't valid modified UTF-8")]
    BadString,
    #[error("there is data after its end")]
    TrailingData,
}

/// A whole file: its root's name and the root compound. Gzipped or not.
pub fn read_file(bytes: &[u8]) -> Result<(String, Compound), NbtError> {
    let raw = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        GzDecoder::new(bytes)
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut out)
            .map_err(|error| NbtError::Gzip(error.to_string()))?;
        if out.len() > MAX_BYTES {
            return Err(NbtError::TooLarge);
        }
        out
    } else {
        bytes.to_vec()
    };
    let mut reader = Reader { bytes: &raw, at: 0 };
    if reader.u8()? != 10 {
        return Err(NbtError::NotACompound);
    }
    let name = reader.string()?;
    let root = reader.compound(0)?;
    if reader.at != raw.len() {
        return Err(NbtError::TrailingData);
    }
    Ok((name, root))
}

/// The file's bytes, uncompressed.
pub fn to_bytes(name: &str, root: &Compound) -> Vec<u8> {
    let mut out = vec![10];
    write_string(&mut out, name);
    write_compound(&mut out, root);
    out
}

/// The file's bytes, gzipped as the game writes them.
pub fn write_file(name: &str, root: &Compound) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    // Writing to memory can't fail.
    let _ = encoder.write_all(&to_bytes(name, root));
    encoder.finish().unwrap_or_default()
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], NbtError> {
        let end = self.at.checked_add(count).ok_or(NbtError::BadSize)?;
        let slice = self.bytes.get(self.at..end).ok_or(NbtError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], NbtError> {
        self.take(N)?.try_into().map_err(|_| NbtError::Truncated)
    }

    fn u8(&mut self) -> Result<u8, NbtError> {
        Ok(self.array::<1>()?[0])
    }

    /// A length, which can't be negative or longer than what is left.
    fn length(&mut self, element_size: usize) -> Result<usize, NbtError> {
        let length = usize::try_from(i32::from_be_bytes(self.array()?)).map_err(|_| NbtError::BadSize)?;
        if length.saturating_mul(element_size) > self.bytes.len() - self.at {
            return Err(NbtError::Truncated);
        }
        Ok(length)
    }

    fn string(&mut self) -> Result<String, NbtError> {
        let length = usize::from(u16::from_be_bytes(self.array()?));
        decode_mutf8(self.take(length)?)
    }

    fn compound(&mut self, depth: usize) -> Result<Compound, NbtError> {
        if depth > MAX_DEPTH {
            return Err(NbtError::TooDeep);
        }
        let mut entries = Vec::new();
        loop {
            let id = self.u8()?;
            if id == 0 {
                return Ok(Compound(entries));
            }
            let name = self.string()?;
            entries.push((name, self.payload(id, depth + 1)?));
        }
    }

    fn payload(&mut self, id: u8, depth: usize) -> Result<Tag, NbtError> {
        Ok(match id {
            1 => Tag::Byte(i8::from_be_bytes(self.array()?)),
            2 => Tag::Short(i16::from_be_bytes(self.array()?)),
            3 => Tag::Int(i32::from_be_bytes(self.array()?)),
            4 => Tag::Long(i64::from_be_bytes(self.array()?)),
            5 => Tag::Float(f32::from_be_bytes(self.array()?)),
            6 => Tag::Double(f64::from_be_bytes(self.array()?)),
            7 => {
                let length = self.length(1)?;
                Tag::ByteArray(
                    self.take(length)?
                        .iter()
                        .map(|&b| i8::from_be_bytes([b]))
                        .collect(),
                )
            }
            8 => Tag::String(self.string()?),
            9 => {
                if depth > MAX_DEPTH {
                    return Err(NbtError::TooDeep);
                }
                let element = self.u8()?;
                let length = self.length(1)?;
                let items = (0..length)
                    .map(|_| self.payload(element, depth + 1))
                    .collect::<Result<_, _>>()?;
                Tag::List(element, items)
            }
            10 => Tag::Compound(self.compound(depth)?),
            11 => {
                let length = self.length(4)?;
                Tag::IntArray(
                    (0..length)
                        .map(|_| Ok(i32::from_be_bytes(self.array()?)))
                        .collect::<Result<_, NbtError>>()?,
                )
            }
            12 => {
                let length = self.length(8)?;
                Tag::LongArray(
                    (0..length)
                        .map(|_| Ok(i64::from_be_bytes(self.array()?)))
                        .collect::<Result<_, NbtError>>()?,
                )
            }
            other => return Err(NbtError::UnknownTag(other)),
        })
    }
}

fn write_compound(out: &mut Vec<u8>, compound: &Compound) {
    for (name, tag) in &compound.0 {
        out.push(tag.id());
        write_string(out, name);
        write_payload(out, tag);
    }
    out.push(0);
}

fn write_length(out: &mut Vec<u8>, length: usize) {
    out.extend_from_slice(&i32::try_from(length).unwrap_or(i32::MAX).to_be_bytes());
}

fn write_payload(out: &mut Vec<u8>, tag: &Tag) {
    match tag {
        Tag::Byte(value) => out.extend_from_slice(&value.to_be_bytes()),
        Tag::Short(value) => out.extend_from_slice(&value.to_be_bytes()),
        Tag::Int(value) => out.extend_from_slice(&value.to_be_bytes()),
        Tag::Long(value) => out.extend_from_slice(&value.to_be_bytes()),
        Tag::Float(value) => out.extend_from_slice(&value.to_be_bytes()),
        Tag::Double(value) => out.extend_from_slice(&value.to_be_bytes()),
        Tag::ByteArray(values) => {
            write_length(out, values.len());
            out.extend(values.iter().map(|value| value.to_be_bytes()[0]));
        }
        Tag::String(value) => write_string(out, value),
        Tag::List(element, items) => {
            out.push(items.first().map_or(*element, Tag::id));
            write_length(out, items.len());
            for item in items {
                write_payload(out, item);
            }
        }
        Tag::Compound(compound) => write_compound(out, compound),
        Tag::IntArray(values) => {
            write_length(out, values.len());
            for value in values {
                out.extend_from_slice(&value.to_be_bytes());
            }
        }
        Tag::LongArray(values) => {
            write_length(out, values.len());
            for value in values {
                out.extend_from_slice(&value.to_be_bytes());
            }
        }
    }
}

fn write_string(out: &mut Vec<u8>, text: &str) {
    let encoded = encode_mutf8(text);
    // Longer strings can't be stored; the game itself refuses to write them.
    let length = u16::try_from(encoded.len()).unwrap_or(u16::MAX);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&encoded[..usize::from(length)]);
}

/// Java's modified UTF-8: NUL is two bytes, and characters beyond the first 65,536 are two
/// three-byte surrogates instead of four bytes.
fn encode_mutf8(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for unit in text.encode_utf16() {
        match unit {
            0x0001..=0x007f => out.push(u8::try_from(unit).unwrap_or(0)),
            0x0000 | 0x0080..=0x07ff => {
                out.push(0xc0 | u8::try_from(unit >> 6).unwrap_or(0));
                out.push(0x80 | u8::try_from(unit & 0x3f).unwrap_or(0));
            }
            _ => {
                out.push(0xe0 | u8::try_from(unit >> 12).unwrap_or(0));
                out.push(0x80 | u8::try_from((unit >> 6) & 0x3f).unwrap_or(0));
                out.push(0x80 | u8::try_from(unit & 0x3f).unwrap_or(0));
            }
        }
    }
    out
}

fn decode_mutf8(bytes: &[u8]) -> Result<String, NbtError> {
    let mut units = Vec::with_capacity(bytes.len());
    let mut at = 0;
    let continuation = |byte: Option<&u8>| match byte {
        Some(byte) if byte & 0xc0 == 0x80 => Ok(u16::from(byte & 0x3f)),
        _ => Err(NbtError::BadString),
    };
    while let Some(&first) = bytes.get(at) {
        let (unit, used) = match first {
            0x01..=0x7f => (u16::from(first), 1),
            0xc0..=0xdf => (
                (u16::from(first & 0x1f) << 6) | continuation(bytes.get(at + 1))?,
                2,
            ),
            0xe0..=0xef => (
                (u16::from(first & 0x0f) << 12)
                    | (continuation(bytes.get(at + 1))? << 6)
                    | continuation(bytes.get(at + 2))?,
                3,
            ),
            _ => return Err(NbtError::BadString),
        };
        units.push(unit);
        at += used;
    }
    String::from_utf16(&units).map_err(|_| NbtError::BadString)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    fn unpacked(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        GzDecoder::new(bytes).read_to_end(&mut out).unwrap();
        out
    }

    /// Real files from real servers come back byte for byte.
    #[test]
    fn real_world_files_round_trip_exactly() {
        for name in ["level_1.20.1.dat", "level_26.3.dat", "game_rules_26.3.dat"] {
            let bytes = fixture(name);
            let (root_name, root) = read_file(&bytes).unwrap();
            assert_eq!(to_bytes(&root_name, &root), unpacked(&bytes), "{name}");
            let again = read_file(&write_file(&root_name, &root)).unwrap();
            assert_eq!(again, (root_name, root), "{name} after gzip");
        }
    }

    #[test]
    fn values_are_found_and_changed_in_place() {
        let (_, mut root) = read_file(&fixture("level_1.20.1.dat")).unwrap();
        let rules = root.compound_at(&["Data", "GameRules"]).unwrap();
        assert_eq!(rules.get("keepInventory"), Some(&Tag::String("false".into())));
        let position = rules.0.iter().position(|(key, _)| key == "keepInventory");
        root.compound_at_mut(&["Data", "GameRules"])
            .unwrap()
            .set("keepInventory", Tag::String("true".into()));
        let rules = root.compound_at(&["Data", "GameRules"]).unwrap();
        assert_eq!(rules.get("keepInventory"), Some(&Tag::String("true".into())));
        assert_eq!(
            rules.0.iter().position(|(key, _)| key == "keepInventory"),
            position,
            "where it was"
        );
    }

    #[test]
    fn modified_utf8_strings() {
        for text in ["plain", "é ü", "a\0b", "🙂 emoji", ""] {
            let encoded = encode_mutf8(text);
            assert!(!encoded.contains(&0), "{text:?} has no zero byte");
            assert_eq!(decode_mutf8(&encoded).unwrap(), text);
        }
        assert_eq!(encode_mutf8("\0"), [0xc0, 0x80]);
        assert_eq!(encode_mutf8("🙂").len(), 6, "two surrogates");
        assert_eq!(decode_mutf8(&[0xff]), Err(NbtError::BadString));
    }

    #[test]
    fn broken_and_hostile_files_are_refused() {
        let good = to_bytes("", &Compound(vec![("a".into(), Tag::Int(1))]));
        for cut in 1..good.len() {
            assert!(read_file(&good[..cut]).is_err(), "cut at {cut}");
        }
        assert_eq!(read_file(&[8, 0, 0]), Err(NbtError::NotACompound));
        // A list claiming two billion entries.
        let mut huge = vec![10, 0, 0, 9, 0, 1, b'x', 1];
        huge.extend_from_slice(&i32::MAX.to_be_bytes());
        assert_eq!(read_file(&huge), Err(NbtError::Truncated));
        // Lists nested past the limit.
        let mut deep = vec![10, 0, 0, 9, 0, 1, b'x'];
        for _ in 0..=MAX_DEPTH + 1 {
            deep.extend_from_slice(&[9, 0, 0, 0, 1]);
        }
        assert_eq!(read_file(&deep), Err(NbtError::TooDeep));
        // As deep as is allowed, on a test thread's 2 MB stack: read, not a crash.
        let mut nested = vec![10, 0, 0, 9, 0, 1, b'x'];
        for _ in 0..MAX_DEPTH - 2 {
            nested.extend_from_slice(&[9, 0, 0, 0, 1]);
        }
        nested.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        assert!(read_file(&nested).is_ok(), "{:?}", read_file(&nested));
        let mut trailing = good.clone();
        trailing.push(0);
        assert_eq!(read_file(&trailing), Err(NbtError::TrailingData));
    }
}
