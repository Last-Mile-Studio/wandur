//! Recognises Mudlet Lua that only sends commands (the C# `MudletPlainSend`): a sequence of
//! `send`, `sendAll` and `expandAlias` calls whose arguments are string literals, numbers,
//! captures (`matches[n]`) and the trigger line (`line`) joined with `..`, with comments and
//! semicolons allowed between them. Anything else is not plain sending, and the item keeps its
//! Lua for conversion by hand. Written from the Lua 5.1 lexical rules.

/// One piece of a command: literal text, a numbered capture from the match (1 is the whole
/// match, as Mudlet numbers them), or the whole line that fired the trigger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendPart {
    Literal(String),
    Capture(u8),
    Line,
}

/// One command a script sends. `through_aliases` is true for `expandAlias`, which runs the
/// text through the aliases first, as typed input would.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendStep {
    pub parts: Vec<SendPart>,
    pub through_aliases: bool,
}

impl SendStep {
    pub fn literal(text: &str, through_aliases: bool) -> Self {
        Self {
            parts: vec![SendPart::Literal(text.to_string())],
            through_aliases,
        }
    }

    pub fn is_literal(&self) -> bool {
        self.parts.iter().all(|p| matches!(p, SendPart::Literal(_)))
    }

    /// The literal parts joined.
    pub fn literal_text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                SendPart::Literal(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Name(String),
    Str(String),
    Number(String),
    Symbol(String),
}

impl Token {
    fn is(&self, symbol: &str) -> bool {
        matches!(self, Token::Symbol(s) if s == symbol)
    }
}

/// The steps, when the Lua only sends commands.
pub fn parse(lua: &str) -> Option<Vec<SendStep>> {
    let tokens = tokenize(lua)?;
    let mut steps = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        if tokens[index].is(";") {
            index += 1;
            continue;
        }
        statement(&tokens, &mut index, &mut steps)?;
    }
    Some(steps)
}

fn statement(tokens: &[Token], index: &mut usize, steps: &mut Vec<SendStep>) -> Option<()> {
    let Token::Name(name) = &tokens[*index] else {
        return None;
    };
    if !matches!(name.as_str(), "send" | "sendAll" | "expandAlias") {
        return None;
    }
    let expand = name == "expandAlias";
    *index += 1;
    if let Some(Token::Str(text)) = tokens.get(*index) {
        // Lua allows f"text" as a call with one string argument.
        if name == "sendAll" {
            return None;
        }
        steps.push(SendStep::literal(text, expand));
        *index += 1;
        return Some(());
    }
    take(tokens, index, "(")?;
    let mut arguments = Vec::new();
    let mut flags = 0;
    if !tokens.get(*index).is_some_and(|t| t.is(")")) {
        loop {
            if let Some(Token::Name(word)) = tokens.get(*index)
                && matches!(word.as_str(), "true" | "false" | "nil")
            {
                // The echo flag; it must come last.
                *index += 1;
                flags += 1;
            } else {
                if flags > 0 {
                    return None;
                }
                arguments.push(expression(tokens, index)?);
            }
            if tokens.get(*index).is_some_and(|t| t.is(",")) {
                *index += 1;
                continue;
            }
            break;
        }
    }
    take(tokens, index, ")")?;
    if flags > 1 || arguments.is_empty() || (name != "sendAll" && arguments.len() != 1) {
        return None;
    }
    steps.extend(arguments.into_iter().map(|parts| SendStep {
        parts,
        through_aliases: expand,
    }));
    Some(())
}

fn expression(tokens: &[Token], index: &mut usize) -> Option<Vec<SendPart>> {
    let mut parts = Vec::new();
    loop {
        term(tokens, index, &mut parts)?;
        if tokens.get(*index).is_some_and(|t| t.is("..")) {
            *index += 1;
            continue;
        }
        return Some(parts);
    }
}

fn term(tokens: &[Token], index: &mut usize, parts: &mut Vec<SendPart>) -> Option<()> {
    match tokens.get(*index)? {
        Token::Str(text) | Token::Number(text) => {
            parts.push(SendPart::Literal(text.clone()));
            *index += 1;
        }
        Token::Symbol(s) if s == "(" => {
            *index += 1;
            let inner = expression(tokens, index)?;
            take(tokens, index, ")")?;
            parts.extend(inner);
        }
        Token::Name(name) if name == "line" => {
            parts.push(SendPart::Line);
            *index += 1;
        }
        Token::Name(name) if name == "matches" => {
            *index += 1;
            take(tokens, index, "[")?;
            let Some(Token::Number(digits)) = tokens.get(*index) else {
                return None;
            };
            let capture: u8 = digits.parse().ok().filter(|n| (1..=99).contains(n))?;
            *index += 1;
            take(tokens, index, "]")?;
            parts.push(SendPart::Capture(capture));
        }
        _ => return None,
    }
    Some(())
}

fn take(tokens: &[Token], index: &mut usize, symbol: &str) -> Option<()> {
    if tokens.get(*index)?.is(symbol) {
        *index += 1;
        Some(())
    } else {
        None
    }
}

/// Names, strings, numbers and the few symbols plain sending uses; comments and whitespace are
/// dropped. Any other symbol is still returned so the parser refuses it. `None` for text Lua
/// would not read (an unfinished string, say).
fn tokenize(source: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = source.chars().collect();
    let at = |i: usize| chars.get(i).copied().unwrap_or('\0');
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '-' && at(i + 1) == '-' {
            i += 2;
            if at(i) == '['
                && let Some(level) = long_bracket_level(&chars, i)
            {
                long_bracket(&chars, &mut i, level)?;
                continue;
            }
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            tokens.push(Token::Name(chars[start..i].iter().collect()));
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            // Only whole decimal numbers: a fraction or exponent would need Lua's number formatting.
            if i < chars.len() && (chars[i].is_ascii_alphabetic() || (chars[i] == '.' && at(i + 1) != '.')) {
                return None;
            }
            let digits: String = chars[start..i].iter().collect();
            let trimmed = digits.trim_start_matches('0');
            tokens.push(Token::Number(if trimmed.is_empty() {
                "0".into()
            } else {
                trimmed.into()
            }));
            continue;
        }
        if c == '"' || c == '\'' {
            tokens.push(Token::Str(short_string(&chars, &mut i)?));
            continue;
        }
        if c == '['
            && let Some(level) = long_bracket_level(&chars, i)
        {
            tokens.push(Token::Str(long_bracket(&chars, &mut i, level)?));
            continue;
        }
        if c == '.' && at(i + 1) == '.' {
            tokens.push(Token::Symbol("..".into()));
            i += 2;
            continue;
        }
        tokens.push(Token::Symbol(c.to_string()));
        i += 1;
    }
    Some(tokens)
}

/// The level of a long bracket opening at `index` (`[[` is 0, `[=[` is 1), or `None`.
fn long_bracket_level(chars: &[char], index: usize) -> Option<usize> {
    let mut i = index + 1;
    let mut level = 0;
    while chars.get(i) == Some(&'=') {
        level += 1;
        i += 1;
    }
    (chars.get(i) == Some(&'[')).then_some(level)
}

fn long_bracket(chars: &[char], i: &mut usize, level: usize) -> Option<String> {
    *i += level + 2;
    // A newline right after the opening bracket is not part of the string.
    match chars.get(*i) {
        Some('\r') => {
            *i += 1;
            if chars.get(*i) == Some(&'\n') {
                *i += 1;
            }
        }
        Some('\n') => {
            *i += 1;
            if chars.get(*i) == Some(&'\r') {
                *i += 1;
            }
        }
        _ => {}
    }
    let close: Vec<char> = std::iter::once(']')
        .chain(std::iter::repeat_n('=', level))
        .chain(std::iter::once(']'))
        .collect();
    let end = (*i..chars.len()).find(|&j| chars[j..].starts_with(&close))?;
    let text = chars[*i..end].iter().collect();
    *i = end + close.len();
    Some(text)
}

fn short_string(chars: &[char], i: &mut usize) -> Option<String> {
    let quote = chars[*i];
    *i += 1;
    let mut text = String::new();
    loop {
        let c = *chars.get(*i)?;
        if c == '\n' || c == '\r' {
            return None;
        }
        *i += 1;
        if c == quote {
            return Some(text);
        }
        if c != '\\' {
            text.push(c);
            continue;
        }
        let e = *chars.get(*i)?;
        *i += 1;
        match e {
            'n' => text.push('\n'),
            't' => text.push('\t'),
            'r' => text.push('\r'),
            'a' => text.push('\u{7}'),
            'b' => text.push('\u{8}'),
            'f' => text.push('\u{c}'),
            'v' => text.push('\u{b}'),
            '\\' => text.push('\\'),
            '"' => text.push('"'),
            '\'' => text.push('\''),
            '\n' => {
                text.push('\n');
                if chars.get(*i) == Some(&'\r') {
                    *i += 1;
                }
            }
            '\r' => {
                text.push('\n');
                if chars.get(*i) == Some(&'\n') {
                    *i += 1;
                }
            }
            d if d.is_ascii_digit() => {
                let mut digits = d.to_string();
                while digits.len() < 3 && chars.get(*i).is_some_and(|c| c.is_ascii_digit()) {
                    digits.push(chars[*i]);
                    *i += 1;
                }
                let value: u32 = digits.parse().ok()?;
                // Only ASCII: a higher byte would depend on how the profile's text was encoded.
                if value > 127 {
                    return None;
                }
                text.push(char::from_u32(value)?);
            }
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C# `PlainSendRecognisesOnlySending`.
    #[test]
    fn plain_send_recognises_only_sending() {
        let steps =
            parse("send(\"a\") ; send('b' .. matches[3], true) -- note\n--[[ block\n]] sendAll(\"c\", [==[d]==])")
                .expect("plain sending");
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[1].parts[1], SendPart::Capture(3));
        let call = parse("expandAlias \"go north\"").unwrap();
        assert_eq!(call.len(), 1);
        assert!(call[0].through_aliases);
        let escaped = parse("send(\"say \\\"hi\\\"\\065\")").unwrap();
        assert_eq!(escaped[0].literal_text(), "say \"hi\"A");
        for lua in [
            "if x then send('a') end",
            "send(x)",
            "send('a' .. b)",
            "local a = 1",
            "send('a'",
            "send('\\200')",
            "send(\"a\", 'b')",
            "echo('hi')",
            "send(1.5)",
        ] {
            assert_eq!(parse(lua), None, "{lua}");
        }
    }
}
