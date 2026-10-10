//! MSDP (telnet option 69, <https://tintin.mudhalla.net/protocols/msdp/>): variables and their
//! values in a subnegotiation, as `VAR name VAL value`, where a value is text, a table
//! (`TABLE_OPEN VAR name VAL value ... TABLE_CLOSE`) or an array (`ARRAY_OPEN VAL v VAL v
//! ARRAY_CLOSE`). Decoding is conservative and bounded, as in the C# SDK's reader: a payload
//! that does not fit the shape is rejected whole, never half used.

use serde_json::Value;

pub const VAR: u8 = 1;
pub const VAL: u8 = 2;
pub const TABLE_OPEN: u8 = 3;
pub const TABLE_CLOSE: u8 = 4;
pub const ARRAY_OPEN: u8 = 5;
pub const ARRAY_CLOSE: u8 = 6;

/// Largest payload decoded (the C# limit; the telnet layer keeps up to 64 KiB).
pub const MAX_PAYLOAD: usize = 16_384;
/// Deepest nesting of tables and arrays.
pub const MAX_DEPTH: usize = 8;
/// Most entries in one table or array.
pub const MAX_ENTRIES: usize = 256;
/// Longest variable name.
pub const MAX_NAME: usize = 128;

/// One MSDP value. Text stays text: MSDP has no numbers, so `"42"` is what the world sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MsdpValue {
    Text(String),
    Array(Vec<MsdpValue>),
    /// Variables in the order sent; names are unique within a table.
    Table(Vec<(String, MsdpValue)>),
}

impl MsdpValue {
    /// The value as JSON: text as a string, arrays as arrays, tables as objects in wire order.
    pub fn to_json(&self) -> Value {
        match self {
            MsdpValue::Text(text) => Value::String(text.clone()),
            MsdpValue::Array(items) => Value::Array(items.iter().map(MsdpValue::to_json).collect()),
            MsdpValue::Table(entries) => Value::Object(
                entries
                    .iter()
                    .map(|(name, value)| (name.clone(), value.to_json()))
                    .collect(),
            ),
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            MsdpValue::Text(text) => Some(text),
            _ => None,
        }
    }

    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            MsdpValue::Text(text) => out.extend_from_slice(text.as_bytes()),
            MsdpValue::Array(items) => {
                out.push(ARRAY_OPEN);
                for item in items {
                    out.push(VAL);
                    item.encode(out);
                }
                out.push(ARRAY_CLOSE);
            }
            MsdpValue::Table(entries) => {
                out.push(TABLE_OPEN);
                encode_entries(entries, out);
                out.push(TABLE_CLOSE);
            }
        }
    }
}

/// The variables of one payload, in wire order.
pub type MsdpTable = Vec<(String, MsdpValue)>;

/// The whole payload as one JSON object (what diagnostics, bindings and the state cache read).
pub fn to_json(table: &MsdpTable) -> Value {
    Value::Object(table.iter().map(|(n, v)| (n.clone(), v.to_json())).collect())
}

/// Decode a payload (without the option byte, IAC escapes removed). `None` when it is empty,
/// too large, too deep, or not MSDP. Repeated `VAL`s after a variable (a command list without
/// `ARRAY_OPEN`) become an array, as servers send them.
pub fn parse(payload: &[u8]) -> Option<MsdpTable> {
    if payload.is_empty() || payload.len() > MAX_PAYLOAD {
        return None;
    }
    let mut reader = Reader { data: payload, at: 0 };
    reader.table(0, false).ok()
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

type Bad = ();

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.data.get(self.at).copied()
    }

    fn expect(&mut self, marker: u8) -> Result<(), Bad> {
        if self.peek() == Some(marker) {
            self.at += 1;
            Ok(())
        } else {
            Err(())
        }
    }

    fn table(&mut self, depth: usize, nested: bool) -> Result<MsdpTable, Bad> {
        if depth > MAX_DEPTH {
            return Err(());
        }
        let mut result: MsdpTable = Vec::new();
        while self.peek().is_some_and(|b| b != TABLE_CLOSE) {
            self.expect(VAR)?;
            let name = self.text()?;
            if name.is_empty() || name.chars().count() > MAX_NAME || result.len() >= MAX_ENTRIES {
                return Err(());
            }
            self.expect(VAL)?;
            let mut value = self.value(depth + 1)?;
            if self.peek() == Some(VAL) {
                let mut values = vec![value];
                while self.peek() == Some(VAL) {
                    self.at += 1;
                    values.push(self.value(depth + 1)?);
                }
                value = MsdpValue::Array(values);
            }
            if result.iter().any(|(n, _)| *n == name) {
                return Err(());
            }
            result.push((name, value));
        }
        if nested {
            self.expect(TABLE_CLOSE)?;
        } else if self.at != self.data.len() {
            return Err(());
        }
        Ok(result)
    }

    fn value(&mut self, depth: usize) -> Result<MsdpValue, Bad> {
        if depth > MAX_DEPTH {
            return Err(());
        }
        match self.peek() {
            Some(TABLE_OPEN) => {
                self.at += 1;
                Ok(MsdpValue::Table(self.table(depth, true)?))
            }
            Some(ARRAY_OPEN) => {
                self.at += 1;
                let mut items = Vec::new();
                while self.peek().is_some_and(|b| b != ARRAY_CLOSE) {
                    if items.len() >= MAX_ENTRIES {
                        return Err(());
                    }
                    self.expect(VAL)?;
                    items.push(self.value(depth + 1)?);
                }
                self.expect(ARRAY_CLOSE)?;
                Ok(MsdpValue::Array(items))
            }
            _ => Ok(MsdpValue::Text(self.text()?)),
        }
    }

    fn text(&mut self) -> Result<String, Bad> {
        let start = self.at;
        while self.peek().is_some_and(|b| b > ARRAY_CLOSE && b != 255) {
            self.at += 1;
        }
        if matches!(self.peek(), Some(0 | 255)) {
            return Err(());
        }
        Ok(String::from_utf8_lossy(&self.data[start..self.at]).into_owned())
    }
}

fn encode_entries(entries: &[(String, MsdpValue)], out: &mut Vec<u8>) {
    for (name, value) in entries {
        out.push(VAR);
        out.extend_from_slice(name.as_bytes());
        out.push(VAL);
        value.encode(out);
    }
}

/// Encode variables as a payload (without the option byte; the telnet layer escapes IAC).
pub fn encode(entries: &[(String, MsdpValue)]) -> Vec<u8> {
    let mut out = Vec::new();
    encode_entries(entries, &mut out);
    out
}

/// A command with its arguments as repeated values: `VAR command VAL a VAL b`.
pub fn command<S: AsRef<str>>(command: &str, arguments: &[S]) -> Vec<u8> {
    let mut out = vec![VAR];
    out.extend_from_slice(command.as_bytes());
    for argument in arguments {
        out.push(VAL);
        out.extend_from_slice(argument.as_ref().as_bytes());
    }
    out
}

/// `LIST REPORTABLE_VARIABLES`, sent when the server enables MSDP.
pub fn list_reportable() -> Vec<u8> {
    command("LIST", &["REPORTABLE_VARIABLES"])
}

/// A name the REPORT and SEND commands accept: ASCII letters, digits and underscore, not starting
/// with a digit, at most 128 characters.
pub fn valid_name(name: &str) -> bool {
    (1..=MAX_NAME).contains(&name.len())
        && !name.as_bytes()[0].is_ascii_digit()
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> MsdpValue {
        MsdpValue::Text(s.into())
    }

    #[test]
    fn variables_tables_and_arrays_decode_in_wire_order() {
        // HEALTH 42, then ROOM { NAME Hall, EXITS [north, south] }.
        let payload = b"\x01HEALTH\x0242\x01ROOM\x02\x03\x01NAME\x02Hall\x01EXITS\x02\x05\x02north\x02south\x06\x04";
        let table = parse(payload).unwrap();
        assert_eq!(
            table,
            vec![
                ("HEALTH".to_string(), text("42")),
                (
                    "ROOM".to_string(),
                    MsdpValue::Table(vec![
                        ("NAME".to_string(), text("Hall")),
                        (
                            "EXITS".to_string(),
                            MsdpValue::Array(vec![text("north"), text("south")])
                        ),
                    ])
                ),
            ]
        );
        assert_eq!(
            serde_json::to_string(&to_json(&table)).unwrap(),
            r#"{"HEALTH":"42","ROOM":{"NAME":"Hall","EXITS":["north","south"]}}"#
        );
        // The encoder writes the same bytes back.
        assert_eq!(encode(&table), payload.to_vec());
    }

    #[test]
    fn nested_tables_inside_arrays_and_repeated_values() {
        let payload = b"\x01GROUP\x02\x05\x02\x03\x01NAME\x02Wren\x01HP\x0290\x04\x02\x03\x01NAME\x02Odo\x04\x06\x01COMMANDS\x02LIST\x02REPORT\x02SEND";
        let table = parse(payload).unwrap();
        assert_eq!(
            serde_json::to_string(&to_json(&table)).unwrap(),
            r#"{"GROUP":[{"NAME":"Wren","HP":"90"},{"NAME":"Odo"}],"COMMANDS":["LIST","REPORT","SEND"]}"#
        );
        // Empty text, an empty table and an empty array are values too.
        let table = parse(b"\x01A\x02\x01B\x02\x03\x04\x01C\x02\x05\x06").unwrap();
        assert_eq!(
            serde_json::to_string(&to_json(&table)).unwrap(),
            r#"{"A":"","B":{},"C":[]}"#
        );
        assert_eq!(encode(&table), b"\x01A\x02\x01B\x02\x03\x04\x01C\x02\x05\x06".to_vec());
    }

    #[test]
    fn malformed_payloads_are_rejected_whole() {
        for bad in [
            &b""[..],
            b"HEALTH\x0242",            // no VAR
            b"\x01HEALTH",              // no VAL
            b"\x01\x0242",              // empty name
            b"\x01A\x021\x01A\x022",    // a name twice
            b"\x01A\x02\x03\x01B\x022", // table not closed
            b"\x01A\x02\x05\x02x",      // array not closed
            b"\x01A\x02x\x04",          // a stray TABLE_CLOSE at the top
            b"\x01A\x02x\x00y",         // NUL in text
            b"\x01A\x02x\xff",          // IAC in text
            b"\x01A\x02\x05x\x06",      // array item without VAL
        ] {
            assert!(parse(bad).is_none(), "{bad:?}");
        }
        assert!(parse(&vec![b'x'; MAX_PAYLOAD + 1]).is_none());
        // Nine levels of tables is too deep; eight is fine.
        let deep = |levels: usize| {
            let mut p = Vec::new();
            for _ in 0..levels {
                p.extend_from_slice(b"\x01T\x02\x03");
            }
            p.extend_from_slice(b"\x01X\x021");
            p.extend(std::iter::repeat_n(TABLE_CLOSE, levels));
            p
        };
        assert!(parse(&deep(7)).is_some());
        assert!(parse(&deep(9)).is_none());
        // 257 entries in one table is one too many.
        let mut many = Vec::new();
        for i in 0..257 {
            many.extend_from_slice(format!("\x01V{i}\x021").as_bytes());
        }
        assert!(parse(&many).is_none());
    }

    #[test]
    fn commands_and_names() {
        assert_eq!(list_reportable(), b"\x01LIST\x02REPORTABLE_VARIABLES");
        assert_eq!(command("REPORT", &["HEALTH", "MANA"]), b"\x01REPORT\x02HEALTH\x02MANA");
        assert!(valid_name("FORCE_POOL") && valid_name("_x") && valid_name("A1"));
        assert!(!valid_name("") && !valid_name("9BAD") && !valid_name("bad name") && !valid_name(&"A".repeat(129)));
    }

    #[test]
    fn text_is_utf8_with_replacement() {
        let table = parse("\x01NAME\x02Caf\u{e9}".as_bytes()).unwrap();
        assert_eq!(table[0].1, text("Café"));
        let table = parse(b"\x01NAME\x02Caf\xe9").unwrap();
        assert_eq!(table[0].1, text("Caf\u{fffd}"));
    }
}
