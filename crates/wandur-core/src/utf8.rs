//! Incremental UTF-8 decoding: a character split across network reads is kept until it completes;
//! invalid bytes become U+FFFD.

#[derive(Debug, Default)]
pub struct Utf8Decoder {
    pending: [u8; 4],
    pending_len: usize,
}

impl Utf8Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode `bytes`, appending to `out`.
    pub fn decode(&mut self, mut bytes: &[u8], out: &mut String) {
        // Finish a character left over from the previous call, one byte at a time.
        while self.pending_len > 0 && !bytes.is_empty() {
            self.pending[self.pending_len] = bytes[0];
            self.pending_len += 1;
            bytes = &bytes[1..];
            match std::str::from_utf8(&self.pending[..self.pending_len]) {
                Ok(s) => {
                    out.push_str(s);
                    self.pending_len = 0;
                }
                Err(e) if e.error_len().is_some() => {
                    // Not a valid continuation: emit a replacement for the broken start and
                    // decode the remaining pending bytes afresh.
                    out.push(char::REPLACEMENT_CHARACTER);
                    let skip = e.error_len().unwrap_or(1);
                    let rest: Vec<u8> = self.pending[skip..self.pending_len].to_vec();
                    self.pending_len = 0;
                    self.decode(&rest, out);
                }
                Err(_) => {
                    if self.pending_len == 4 {
                        out.push(char::REPLACEMENT_CHARACTER);
                        self.pending_len = 0;
                    }
                }
            }
        }
        loop {
            match std::str::from_utf8(bytes) {
                Ok(s) => {
                    out.push_str(s);
                    return;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    // Safety of the unchecked path is not needed: from_utf8 on the prefix cannot fail.
                    out.push_str(std::str::from_utf8(&bytes[..valid]).unwrap_or_default());
                    match e.error_len() {
                        Some(n) => {
                            out.push(char::REPLACEMENT_CHARACTER);
                            bytes = &bytes[valid + n..];
                        }
                        None => {
                            let tail = &bytes[valid..];
                            self.pending[..tail.len()].copy_from_slice(tail);
                            self.pending_len = tail.len();
                            return;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(chunks: &[&[u8]]) -> String {
        let mut d = Utf8Decoder::new();
        let mut out = String::new();
        for c in chunks {
            d.decode(c, &mut out);
        }
        out
    }

    #[test]
    fn ascii_and_multibyte() {
        assert_eq!(decode(&["héllo €𝄞".as_bytes()]), "héllo €𝄞");
    }

    #[test]
    fn split_characters_are_joined() {
        let bytes = "a€b𝄞c".as_bytes();
        for split in 0..bytes.len() {
            assert_eq!(decode(&[&bytes[..split], &bytes[split..]]), "a€b𝄞c", "split {split}");
        }
        let one_by_one: Vec<&[u8]> = bytes.chunks(1).collect();
        assert_eq!(decode(&one_by_one), "a€b𝄞c");
    }

    #[test]
    fn invalid_bytes_become_replacement() {
        assert_eq!(decode(&[b"a\xffb"]), "a\u{fffd}b");
        assert_eq!(decode(&[b"a\xe2", b"\x28b"]), "a\u{fffd}(b");
        // Latin-1 text in a UTF-8 session degrades without losing the rest.
        assert_eq!(decode(&[b"caf\xe9 ok"]), "caf\u{fffd} ok");
    }
}
