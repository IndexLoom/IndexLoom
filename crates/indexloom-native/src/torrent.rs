use crate::Error;

/// Structural metainfo check, not a content/hash verification service. Bounded
/// recursion and token count protect against pathological bencoded downloads.
pub(crate) fn validate(bytes: &[u8]) -> Result<(), Error> {
    let mut parser = Parser {
        bytes,
        at: 0,
        tokens: 0,
    };
    if parser.take()? != b'd' {
        return Err(invalid());
    }
    let mut info = false;
    while parser.peek()? != b'e' {
        let key = parser.string()?;
        if key == b"info" {
            if info || parser.peek()? != b'd' {
                return Err(invalid());
            }
            info = true;
        }
        parser.value(1)?;
    }
    parser.take()?;
    if !info || parser.at != bytes.len() {
        return Err(invalid());
    }
    Ok(())
}

fn invalid() -> Error {
    Error::Parse("invalid torrent metainfo")
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
    tokens: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Result<u8, Error> {
        self.bytes.get(self.at).copied().ok_or_else(invalid)
    }
    fn take(&mut self) -> Result<u8, Error> {
        let value = self.peek()?;
        self.at += 1;
        Ok(value)
    }
    fn string(&mut self) -> Result<&'a [u8], Error> {
        let start = self.at;
        let mut size = 0usize;
        while self.peek()?.is_ascii_digit() {
            size = size
                .checked_mul(10)
                .and_then(|n| n.checked_add(usize::from(self.take().ok()? - b'0')))
                .ok_or_else(invalid)?;
        }
        if self.at == start
            || (self.at > start + 1 && self.bytes[start] == b'0')
            || self.take()? != b':'
        {
            return Err(invalid());
        }
        let end = self.at.checked_add(size).ok_or_else(invalid)?;
        let result = self.bytes.get(self.at..end).ok_or_else(invalid)?;
        self.at = end;
        Ok(result)
    }
    fn value(&mut self, depth: usize) -> Result<(), Error> {
        self.tokens += 1;
        if depth > 64 || self.tokens > 100_000 {
            return Err(invalid());
        }
        match self.peek()? {
            b'd' | b'l' => {
                let dictionary = self.take()? == b'd';
                while self.peek()? != b'e' {
                    if dictionary {
                        self.string()?;
                    }
                    self.value(depth + 1)?;
                }
                self.take()?;
            }
            b'i' => {
                self.take()?;
                let start = self.at;
                if self.peek()? == b'-' {
                    self.take()?;
                }
                let digits = self.at;
                while self.peek()?.is_ascii_digit() {
                    self.take()?;
                }
                if self.at == digits
                    || (self.at > digits + 1 && self.bytes[digits] == b'0')
                    || (digits > start && self.bytes[digits] == b'0')
                    || self.take()? != b'e'
                {
                    return Err(invalid());
                }
            }
            b'0'..=b'9' => {
                self.string()?;
            }
            _ => return Err(invalid()),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::validate;

    #[test]
    fn rejects_truncation_html_missing_info_and_pathological_input() {
        let valid = b"d4:infod4:name4:demo6:lengthi1eee";
        assert!(validate(valid).is_ok());
        for n in 0..valid.len() {
            assert!(validate(&valid[..n]).is_err(), "prefix {n}");
        }
        for invalid in [
            b"<html>login</html>".as_slice(),
            b"de",
            b"d4:infolee",
            b"d4:infodeejunk",
            b"d4:infod1:ai01eee",
            b"d4:infod1:ai-0eee",
            b"d4:infod01:ai1eee",
            b"d4:infod9999999999999999999999:aee",
            b"d4:infode4:infodee",
        ] {
            assert!(validate(invalid).is_err());
        }
        let mut deep = b"d4:info".to_vec();
        deep.extend(std::iter::repeat_n(b'd', 100));
        assert!(validate(&deep).is_err());
    }
}
