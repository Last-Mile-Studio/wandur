//! The text encoding of a connection: UTF-8 (default) or Latin-1, as the C# client offers per
//! profile. Decoding is incremental; encoding of commands doubles IAC and appends CR LF.

use serde::{Deserialize, Serialize};

use crate::telnet::IAC;
use crate::utf8::Utf8Decoder;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Charset {
    #[default]
    Utf8,
    Latin1,
}

impl Charset {
    pub fn label(self) -> &'static str {
        match self {
            Charset::Utf8 => "UTF-8",
            Charset::Latin1 => "Latin-1",
        }
    }
}

/// Incremental decoder for one connection.
#[derive(Debug, Default)]
pub struct TextDecoder {
    charset: Charset,
    utf8: Utf8Decoder,
}

impl TextDecoder {
    pub fn new(charset: Charset) -> Self {
        Self {
            charset,
            utf8: Utf8Decoder::new(),
        }
    }

    pub fn decode(&mut self, bytes: &[u8], out: &mut String) {
        match self.charset {
            Charset::Utf8 => self.utf8.decode(bytes, out),
            Charset::Latin1 => out.extend(bytes.iter().map(|&b| b as char)),
        }
    }
}

/// Encode a command line for sending: in `charset` (characters Latin-1 cannot hold become `?`),
/// IAC doubled, CR LF appended.
pub fn encode_line(line: &str, charset: Charset, out: &mut Vec<u8>) {
    out.reserve(line.len() + 2);
    let mut push = |b: u8| {
        if b == IAC {
            out.push(IAC);
        }
        out.push(b);
    };
    match charset {
        Charset::Utf8 => line.bytes().for_each(&mut push),
        Charset::Latin1 => line
            .chars()
            .for_each(|c| push(u8::try_from(u32::from(c)).unwrap_or(b'?'))),
    }
    out.extend_from_slice(b"\r\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_round_trip() {
        let mut d = TextDecoder::new(Charset::Latin1);
        let mut s = String::new();
        d.decode(b"caf\xe9 \xff", &mut s);
        assert_eq!(s, "café ÿ");
        let mut out = Vec::new();
        encode_line("café ÿ 漢", Charset::Latin1, &mut out);
        assert_eq!(out, b"caf\xe9 \xff\xff ?\r\n");
    }

    #[test]
    fn utf8_encoding_has_no_iac_to_double() {
        let mut out = Vec::new();
        encode_line("say ÿ", Charset::Utf8, &mut out);
        assert_eq!(out, b"say \xC3\xBF\r\n");
        let mut d = TextDecoder::new(Charset::Utf8);
        let mut s = String::new();
        d.decode("é".as_bytes(), &mut s);
        assert_eq!(s, "é");
    }
}
