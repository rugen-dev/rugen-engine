//! Tokenizer matching MUGEN's rules: identifiers run until one of
//! `" !=<>()|&+-*/%,[]^:;{}#\"` (so `velocity.walk.fwd.x` is one token),
//! numbers are digits with at most one `.`, strings are double-quoted.

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Num(String),
    /// Lowercased identifier.
    Ident(String),
    /// Double-quoted string, original case, quotes removed.
    Str(String),
    Op(&'static str),
    Comma,
    LParen,
    RParen,
    LBracket,
    RBracket,
    End,
}

impl Tok {
    pub fn is_op(&self, s: &str) -> bool {
        matches!(self, Tok::Op(o) if *o == s)
    }
    pub fn ident(&self) -> Option<&str> {
        match self {
            Tok::Ident(s) => Some(s),
            _ => None,
        }
    }
}

const SPECIAL: &[u8] = b" !=<>()|&+-*/%,[]^:;{}#\"\t\r\n";

pub fn lex(src: &str) -> Result<Vec<Tok>, crate::Error> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let two = |s: &'static str| -> Option<&'static str> {
            if b[i..].starts_with(s.as_bytes()) {
                Some(s)
            } else {
                None
            }
        };
        let op = match c {
            b'=' => Some("="),
            b':' => two(":=").or(Some(":")),
            b'!' => two("!=").or(Some("!")),
            b'>' => two(">=").or(Some(">")),
            b'<' => two("<=").or(Some("<")),
            b'~' => Some("~"),
            b'&' => two("&&").or(Some("&")),
            b'^' => two("^^").or(Some("^")),
            b'|' => two("||").or(Some("|")),
            b'+' => Some("+"),
            b'-' => Some("-"),
            b'*' => two("**").or(Some("*")),
            b'/' => Some("/"),
            b'%' => Some("%"),
            b';' => Some(";"),
            _ => None,
        };
        if let Some(op) = op {
            i += op.len();
            out.push(Tok::Op(op));
            continue;
        }
        match c {
            b',' => {
                out.push(Tok::Comma);
                i += 1;
                continue;
            }
            b'(' => {
                out.push(Tok::LParen);
                i += 1;
                continue;
            }
            b')' => {
                out.push(Tok::RParen);
                i += 1;
                continue;
            }
            b'[' => {
                out.push(Tok::LBracket);
                i += 1;
                continue;
            }
            b']' => {
                out.push(Tok::RBracket);
                i += 1;
                continue;
            }
            b'"' => {
                let start = i + 1;
                let end = src[start..]
                    .find('"')
                    .map(|e| start + e)
                    .ok_or_else(|| crate::Error::new("unterminated string"))?;
                out.push(Tok::Str(src[start..end].to_string()));
                i = end + 1;
                continue;
            }
            _ => {}
        }
        // number: digits with at most one '.', optional exponent
        let mut j = i;
        let mut dot = false;
        while j < b.len() {
            if b[j] == b'.' {
                if dot {
                    break;
                }
                dot = true;
            } else if !b[j].is_ascii_digit() {
                break;
            }
            j += 1;
        }
        if j > i && j < b.len() && (b[j] == b'e' || b[j] == b'E') {
            let mut k = j + 1;
            if k < b.len() && (b[k] == b'-' || b[k] == b'+') {
                k += 1;
            }
            let ds = k;
            while k < b.len() && b[k].is_ascii_digit() {
                k += 1;
            }
            if k > ds {
                j = k;
            }
        }
        if j > i && &src[i..j] != "." {
            out.push(Tok::Num(src[i..j].to_string()));
            i = j;
            continue;
        }
        // identifier up to a special symbol
        let mut j = i;
        while j < b.len() && !SPECIAL.contains(&b[j]) {
            j += 1;
        }
        if j == i {
            return Err(crate::Error::new(format!(
                "unexpected character {:?}",
                c as char
            )));
        }
        out.push(Tok::Ident(src[i..j].to_ascii_lowercase()));
        i = j;
    }
    out.push(Tok::End);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tokens() {
        let t = lex("Pos Y >= -2.5 && var(3) := 1 || command != \"QCF_x\"").unwrap();
        assert_eq!(
            t,
            vec![
                Tok::Ident("pos".into()),
                Tok::Ident("y".into()),
                Tok::Op(">="),
                Tok::Op("-"),
                Tok::Num("2.5".into()),
                Tok::Op("&&"),
                Tok::Ident("var".into()),
                Tok::LParen,
                Tok::Num("3".into()),
                Tok::RParen,
                Tok::Op(":="),
                Tok::Num("1".into()),
                Tok::Op("||"),
                Tok::Ident("command".into()),
                Tok::Op("!="),
                Tok::Str("QCF_x".into()),
                Tok::End
            ]
        );
        assert_eq!(
            lex("const(velocity.walk.fwd.x)").unwrap()[2],
            Tok::Ident("velocity.walk.fwd.x".into())
        );
        assert_eq!(
            lex(".5").unwrap().first().cloned(),
            Some(Tok::Num(".5".into()))
        );
    }
}
