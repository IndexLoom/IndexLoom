//! Bounded bencode parser and BEP 3/BEP 52 metadata shape validation.
use crate::EngineError;
use std::collections::BTreeMap;

#[derive(Debug)]
enum Value<'a> {
    Int(i64),
    Bytes(&'a [u8]),
    List(Vec<Self>),
    Dict(BTreeMap<&'a [u8], Self>),
}
impl<'a> Value<'a> {
    fn dict(&self) -> Option<&BTreeMap<&'a [u8], Self>> {
        if let Self::Dict(v) = self {
            Some(v)
        } else {
            None
        }
    }
    fn bytes(&self) -> Option<&'a [u8]> {
        if let Self::Bytes(v) = self {
            Some(v)
        } else {
            None
        }
    }
    fn uint(&self) -> Option<u64> {
        if let Self::Int(v) = self {
            (*v).try_into().ok()
        } else {
            None
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    offset: usize,
    remaining: usize,
}
impl<'a> Parser<'a> {
    fn string(&mut self) -> Option<&'a [u8]> {
        let start = self.offset;
        while self.bytes.get(self.offset)?.is_ascii_digit() {
            self.offset += 1;
        }
        if self.bytes.get(self.offset) != Some(&b':') || self.offset == start {
            return None;
        }
        if self.offset - start > 1 && self.bytes[start] == b'0' {
            return None;
        }
        let size: usize = std::str::from_utf8(&self.bytes[start..self.offset])
            .ok()?
            .parse()
            .ok()?;
        self.offset += 1;
        let end = self.offset.checked_add(size)?;
        let value = self.bytes.get(self.offset..end)?;
        self.offset = end;
        Some(value)
    }
    fn value(&mut self, depth: usize) -> Option<Value<'a>> {
        if depth > 32 || self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        match *self.bytes.get(self.offset)? {
            b'0'..=b'9' => self.string().map(Value::Bytes),
            b'i' => {
                self.offset += 1;
                let start = self.offset;
                while *self.bytes.get(self.offset)? != b'e' {
                    self.offset += 1;
                }
                let raw = std::str::from_utf8(&self.bytes[start..self.offset]).ok()?;
                if raw.is_empty()
                    || raw == "-0"
                    || raw.starts_with('+')
                    || (raw.len() > 1 && raw.starts_with('0'))
                    || raw.starts_with("-0")
                {
                    return None;
                }
                let value = raw.parse().ok()?;
                self.offset += 1;
                Some(Value::Int(value))
            }
            b'l' => {
                self.offset += 1;
                let mut list = Vec::new();
                while *self.bytes.get(self.offset)? != b'e' {
                    list.push(self.value(depth + 1)?);
                }
                self.offset += 1;
                Some(Value::List(list))
            }
            b'd' => {
                self.offset += 1;
                let mut dict = BTreeMap::new();
                let mut previous: Option<&[u8]> = None;
                while *self.bytes.get(self.offset)? != b'e' {
                    if self.remaining == 0 {
                        return None;
                    }
                    self.remaining -= 1;
                    let key = self.string()?;
                    if previous.is_some_and(|p| p >= key) {
                        return None;
                    }
                    previous = Some(key);
                    dict.insert(key, self.value(depth + 1)?);
                }
                self.offset += 1;
                Some(Value::Dict(dict))
            }
            _ => None,
        }
    }
}

/// Validates complete bencoding, bounded nesting/node count, an `info`
/// dictionary and coherent single/multi-file v1 or v2 metadata. Does not trust
/// MIME types, file extensions or just the initial `d` byte.
///
/// # Errors
/// Returns [`EngineError::InvalidTorrent`] for malformed or incomplete metadata.
pub fn validate_torrent(bytes: &[u8]) -> Result<(), EngineError> {
    if bytes.is_empty() || bytes.len() > super::super::MAX_DIRECT_RESPONSE_BYTES {
        return Err(EngineError::InvalidTorrent);
    }
    validate(bytes).ok_or(EngineError::InvalidTorrent)
}

fn validate(bytes: &[u8]) -> Option<()> {
    let mut parser = Parser {
        bytes,
        offset: 0,
        remaining: 200_000,
    };
    let root = parser.value(0)?;
    if parser.offset != bytes.len() {
        return None;
    }
    let root = root.dict()?;
    let info = root.get(b"info".as_slice())?.dict()?;
    path_component(info.get(b"name".as_slice())?.bytes()?)?;
    let piece_length = info.get(b"piece length".as_slice())?.uint()?;
    if piece_length == 0 {
        return None;
    }
    let is_v2 = info.get(b"meta version".as_slice()).and_then(Value::uint) == Some(2);
    let has_v1 = info.contains_key(b"pieces".as_slice());
    if !is_v2 && !has_v1 {
        return None;
    }
    if has_v1 {
        let pieces = info.get(b"pieces".as_slice())?.bytes()?;
        if pieces.len() % 20 != 0 {
            return None;
        }
        let length = match (
            info.get(b"length".as_slice()),
            info.get(b"files".as_slice()),
        ) {
            (Some(length), None) => length.uint()?,
            (None, Some(Value::List(files))) if !files.is_empty() => {
                let mut total = 0_u64;
                let mut seen = std::collections::BTreeSet::new();
                for file in files {
                    let file = file.dict()?;
                    total = total.checked_add(file.get(b"length".as_slice())?.uint()?)?;
                    let Value::List(path) = file.get(b"path".as_slice())? else {
                        return None;
                    };
                    if path.is_empty() {
                        return None;
                    }
                    let mut components = Vec::new();
                    for component in path {
                        let component = component.bytes()?;
                        path_component(component)?;
                        components.push(component);
                    }
                    if !seen.insert(components) {
                        return None;
                    }
                }
                total
            }
            _ => return None,
        };
        let expected = length / piece_length + u64::from(length % piece_length != 0);
        if u64::try_from(pieces.len() / 20).ok()? != expected {
            return None;
        }
    }
    if is_v2 {
        if piece_length < 16384 || !piece_length.is_power_of_two() {
            return None;
        }
        let tree = info.get(b"file tree".as_slice())?.dict()?;
        if tree.is_empty() {
            return None;
        }
        let mut required_layers = BTreeMap::new();
        validate_tree(tree, false, piece_length, &mut required_layers)?;
        let layers = match root.get(b"piece layers".as_slice()) {
            Some(value) => Some(value.dict()?),
            None => None,
        };
        for (hash, count) in required_layers {
            let layer = layers?.get(hash)?.bytes()?;
            if u64::try_from(layer.len()).ok()? != count.checked_mul(32)? {
                return None;
            }
        }
        if let Some(layers) = root.get(b"piece layers".as_slice()) {
            for (hash, layer) in layers.dict()? {
                if hash.len() != 32 || layer.bytes()?.len() % 32 != 0 {
                    return None;
                }
            }
        }
    } else if info.contains_key(b"meta version".as_slice()) {
        return None;
    }
    Some(())
}

fn path_component(value: &[u8]) -> Option<()> {
    if value.is_empty()
        || value == b"."
        || value == b".."
        || value.iter().any(|c| matches!(c, b'/' | b'\\' | 0))
    {
        None
    } else {
        Some(())
    }
}
fn validate_tree<'a>(
    tree: &BTreeMap<&'a [u8], Value<'a>>,
    leaf_allowed: bool,
    piece_length: u64,
    required: &mut BTreeMap<&'a [u8], u64>,
) -> Option<()> {
    if let Some(leaf) = tree.get(b"".as_slice()) {
        if !leaf_allowed || tree.len() != 1 {
            return None;
        }
        let leaf = leaf.dict()?;
        let length = leaf.get(b"length".as_slice())?.uint()?;
        if length > 0 {
            let hash = leaf.get(b"pieces root".as_slice())?.bytes()?;
            if hash.len() != 32 {
                return None;
            }
            if length > piece_length {
                let count = length.div_ceil(piece_length);
                if required
                    .insert(hash, count)
                    .is_some_and(|previous| previous != count)
                {
                    return None;
                }
            }
        } else if leaf.contains_key(b"pieces root".as_slice()) {
            return None;
        }
        return Some(());
    }
    if tree.is_empty() {
        return None;
    }
    for (name, child) in tree {
        path_component(name)?;
        validate_tree(child.dict()?, true, piece_length, required)?;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_binary_metadata_and_rejects_fake_torrents() {
        let mut good = b"d4:infod6:lengthi1e4:name1:x12:piece lengthi16384e6:pieces20:".to_vec();
        good.extend([0xff; 20]);
        good.extend(b"ee");
        assert!(validate_torrent(&good).is_ok());
        for bad in [
            b"d4:infodee".as_slice(),
            b"<html>login</html>",
            b"d4:infoi1ee",
            b"d4:infod4:name1:xee",
            b"d1:ai01ee",
            b"d1:ai-0ee",
            b"d1:ai1e1:ai2ee",
        ] {
            assert!(validate_torrent(bad).is_err());
        }
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(validate_torrent(&trailing).is_err());
        good.pop();
        assert!(validate_torrent(&good).is_err());
    }
    fn encode(value: &Value<'_>, out: &mut Vec<u8>) {
        match value {
            Value::Int(number) => out.extend(format!("i{number}e").as_bytes()),
            Value::Bytes(bytes) => {
                out.extend(format!("{}:", bytes.len()).as_bytes());
                out.extend(*bytes);
            }
            Value::List(list) => {
                out.push(b'l');
                for child in list {
                    encode(child, out);
                }
                out.push(b'e');
            }
            Value::Dict(dict) => {
                out.push(b'd');
                for (key, value) in dict {
                    encode(&Value::Bytes(key), out);
                    encode(value, out);
                }
                out.push(b'e');
            }
        }
    }
    fn dict<'a, const N: usize>(pairs: [(&'a [u8], Value<'a>); N]) -> Value<'a> {
        Value::Dict(pairs.into_iter().collect())
    }
    fn v2(length: i64, layers: Option<usize>) -> Vec<u8> {
        let hash = [1_u8; 32];
        let info = dict([
            (
                b"file tree".as_slice(),
                dict([(
                    b"x".as_slice(),
                    dict([(
                        b"".as_slice(),
                        dict([
                            (b"length".as_slice(), Value::Int(length)),
                            (b"pieces root".as_slice(), Value::Bytes(&hash)),
                        ]),
                    )]),
                )]),
            ),
            (b"meta version".as_slice(), Value::Int(2)),
            (b"name".as_slice(), Value::Bytes(b"fixture")),
            (b"piece length".as_slice(), Value::Int(16384)),
        ]);
        let layer_bytes = vec![0_u8; layers.unwrap_or(0)];
        let mut root = BTreeMap::from([(b"info".as_slice(), info)]);
        if layers.is_some() {
            root.insert(
                b"piece layers".as_slice(),
                dict([(hash.as_slice(), Value::Bytes(&layer_bytes))]),
            );
        }
        let mut bytes = Vec::new();
        encode(&Value::Dict(root), &mut bytes);
        bytes
    }
    #[test]
    fn validates_v2_file_tree_and_required_piece_layer_sizes() {
        assert!(validate_torrent(&v2(1, None)).is_ok());
        assert!(validate_torrent(&v2(16385, None)).is_err());
        assert!(validate_torrent(&v2(16385, Some(32))).is_err());
        assert!(validate_torrent(&v2(16385, Some(64))).is_ok());
    }
    #[test]
    fn rejects_excessive_nesting_and_path_traversal() {
        let mut bytes = vec![b'l'; 100];
        bytes.extend(vec![b'e'; 100]);
        assert!(validate_torrent(&bytes).is_err());
        assert!(path_component(b"..").is_none());
        assert!(path_component(b"/tmp/x").is_none());
    }
}
