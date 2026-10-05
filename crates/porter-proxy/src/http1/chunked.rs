//! Where a chunked body ends (RFC 9112 7.1). The relay passes the bytes through unchanged and
//! only needs to know which byte is the last, so the next request head starts in the right
//! place. Strict about line ends: a bare `LF` is refused, because a server that read it
//! differently would find a different end.

/// Where a chunked body stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Chunked {
    /// Reading a chunk-size line.
    #[default]
    Size,
    /// Inside a chunk's data, this many more bytes.
    Data(u64),
    /// Expecting the `CRLF` after a chunk's data (this many of its two bytes still to come).
    DataEnd(u8),
    /// Reading the trailer lines up to the blank one.
    Trailer,
    /// The last chunk and its trailer are past.
    Done,
}

/// The parser: its state and the line it is in the middle of.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChunkParser {
    /// Where the body stands.
    pub state: Chunked,
    line: Vec<u8>,
}

/// The longest size or trailer line accepted.
const MAX_LINE: usize = 8 * 1024;

/// A chunked body that is not well formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed;

fn size_of(line: &[u8]) -> Result<u64, Malformed> {
    let digits: Vec<u8> = line
        .iter()
        .copied()
        .take_while(|b| b.is_ascii_hexdigit())
        .collect();
    let rest = &line[digits.len()..];
    let extension_ok = rest.is_empty() || rest[0] == b';' || rest[0] == b' ' || rest[0] == b'\t';
    let text = std::str::from_utf8(&digits).map_err(|_| Malformed)?;
    match (digits.is_empty() || digits.len() > 15, extension_ok) {
        (false, true) => u64::from_str_radix(text, 16).map_err(|_| Malformed),
        _ => Err(Malformed),
    }
}

impl ChunkParser {
    /// Consumes bytes of `data` up to the end of the body or the end of `data`; returns how many
    /// it took. `state == Done` after it means the body ended at that byte.
    pub fn feed(&mut self, data: &[u8]) -> Result<usize, Malformed> {
        let mut at = 0;
        while at < data.len() && self.state != Chunked::Done {
            at += match self.state.clone() {
                Chunked::Size | Chunked::Trailer => self.line_byte(data[at])?,
                Chunked::Data(left) => {
                    let take =
                        usize::try_from(left).map_or(data.len() - at, |l| l.min(data.len() - at));
                    self.state = match left - take as u64 {
                        0 => Chunked::DataEnd(2),
                        more => Chunked::Data(more),
                    };
                    take
                }
                Chunked::DataEnd(left) => {
                    let expected = if left == 2 { b'\r' } else { b'\n' };
                    if data[at] != expected {
                        return Err(Malformed);
                    }
                    self.state = match left {
                        1 => Chunked::Size,
                        _ => Chunked::DataEnd(1),
                    };
                    1
                }
                Chunked::Done => 0,
            };
        }
        Ok(at)
    }

    fn line_byte(&mut self, byte: u8) -> Result<usize, Malformed> {
        if byte != b'\n' {
            self.line.push(byte);
            return match self.line.len() > MAX_LINE {
                true => Err(Malformed),
                false => Ok(1),
            };
        }
        let line = std::mem::take(&mut self.line);
        let Some(line) = line.strip_suffix(b"\r") else {
            return Err(Malformed);
        };
        self.state = match (&self.state, line.is_empty()) {
            (Chunked::Size, _) => match size_of(line)? {
                0 => Chunked::Trailer,
                size => Chunked::Data(size),
            },
            (_, true) => Chunked::Done,
            (_, false) => Chunked::Trailer,
        };
        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(parts: &[&[u8]]) -> (Result<usize, Malformed>, ChunkParser) {
        let mut parser = ChunkParser::default();
        let mut total = 0;
        for part in parts {
            match parser.feed(part) {
                Ok(n) => total += n,
                Err(e) => return (Err(e), parser),
            }
        }
        (Ok(total), parser)
    }

    #[test]
    fn a_chunked_body_ends_at_its_blank_line_wherever_the_reads_split() {
        const BODY: &[u8] = b"4\r\nWiki\r\n5;ext=1\r\npedia\r\n0\r\nX-Sum: 1\r\n\r\nNEXT";
        for cut in 0..BODY.len() {
            let (left, right) = BODY.split_at(cut);
            let mut parser = ChunkParser::default();
            let mut used = parser.feed(left).expect("well formed");
            if parser.state != Chunked::Done {
                used += parser.feed(right).expect("well formed");
            }
            assert_eq!(parser.state, Chunked::Done, "cut {cut}");
            assert_eq!(&BODY[used..], b"NEXT", "cut {cut}");
        }
    }

    #[test]
    fn a_malformed_body_is_refused() {
        const CASES: &[(&str, &[u8])] = &[
            ("not hex", b"zz\r\n"),
            ("empty size", b"\r\n"),
            ("bare lf", b"4\nWiki"),
            ("no crlf after data", b"4\r\nWikiXX"),
            ("size too large", b"fffffffffffffffff\r\n"),
            ("junk after size", b"4x\r\n"),
        ];
        for (name, bytes) in CASES {
            assert!(feed_all(&[bytes]).0.is_err(), "{name}");
        }
    }

    #[test]
    fn an_empty_body_is_the_last_chunk_alone() {
        let (used, parser) = feed_all(&[b"0\r\n\r\n"]);
        assert_eq!(used, Ok(5));
        assert_eq!(parser.state, Chunked::Done);
    }
}
