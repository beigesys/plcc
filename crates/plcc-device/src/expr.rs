// SPDX-License-Identifier: MPL-2.0

//! `{expr}` templates in repeated `[[io]]` entries: `"I{n}"`,
//! `"%IX{(n-1)/8}.{(n-1)%8}"`. An expression is integer arithmetic on the
//! variable `n`: literals, `+ - * / %`, unary minus and parentheses. `/` and
//! `%` truncate toward zero, as in C and Rust.

/// Expand every `{expr}` in `template` with `n`. `{{` and `}}` are literal braces.
pub fn expand(template: &str, n: i64) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '{' if chars.peek().map(|&(_, c)| c) == Some('{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek().map(|&(_, c)| c) == Some('}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let start = i + 1;
                let mut end = None;
                for (j, c) in chars.by_ref() {
                    if c == '}' {
                        end = Some(j);
                        break;
                    }
                }
                let end = end.ok_or_else(|| format!("`{template}`: `{{` without a closing `}}`"))?;
                let v = eval(&template[start..end], n)
                    .map_err(|e| format!("`{template}`: in `{{{}}}`: {e}", &template[start..end]))?;
                out.push_str(&v.to_string());
            }
            '}' => return Err(format!("`{template}`: `}}` without an opening `{{` (write `}}}}` for a brace)")),
            c => out.push(c),
        }
    }
    Ok(out)
}

/// Whether `s` contains a `{expr}` (as opposed to only literal text).
pub fn has_placeholder(s: &str) -> bool {
    s.replace("{{", "").contains('{')
}

/// Evaluate one expression.
pub fn eval(expr: &str, n: i64) -> Result<i64, String> {
    let tokens = lex(expr)?;
    let mut p = Parser { tokens, pos: 0, n };
    let v = p.sum()?;
    if p.pos != p.tokens.len() {
        return Err(format!("unexpected `{}`", p.tokens[p.pos]));
    }
    Ok(v)
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(i64),
    N,
    Op(char),
}

impl std::fmt::Display for Tok {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tok::Num(v) => write!(f, "{v}"),
            Tok::N => f.write_str("n"),
            Tok::Op(c) => write!(f, "{c}"),
        }
    }
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let v = s[start..i]
                .parse()
                .map_err(|_| format!("`{}` is too large", &s[start..i]))?;
            out.push(Tok::Num(v));
        } else if c == 'n' {
            if i + 1 < b.len() && (b[i + 1].is_ascii_alphanumeric() || b[i + 1] == b'_') {
                return Err("the only variable is `n`".into());
            }
            out.push(Tok::N);
            i += 1;
        } else if "+-*/%()".contains(c) {
            out.push(Tok::Op(c));
            i += 1;
        } else if c.is_ascii_alphabetic() || c == '_' {
            return Err("the only variable is `n`".into());
        } else {
            return Err(format!("unexpected `{c}`"));
        }
    }
    if out.is_empty() {
        return Err("empty expression".into());
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
    n: i64,
}

impl Parser {
    fn peek_op(&self) -> Option<char> {
        match self.tokens.get(self.pos) {
            Some(Tok::Op(c)) => Some(*c),
            _ => None,
        }
    }

    fn sum(&mut self) -> Result<i64, String> {
        let mut v = self.product()?;
        while let Some(op @ ('+' | '-')) = self.peek_op() {
            self.pos += 1;
            let r = self.product()?;
            v = if op == '+' { v.checked_add(r) } else { v.checked_sub(r) }
                .ok_or("overflow")?;
        }
        Ok(v)
    }

    fn product(&mut self) -> Result<i64, String> {
        let mut v = self.unary()?;
        while let Some(op @ ('*' | '/' | '%')) = self.peek_op() {
            self.pos += 1;
            let r = self.unary()?;
            v = match op {
                '*' => v.checked_mul(r).ok_or("overflow")?,
                _ if r == 0 => return Err("division by zero".into()),
                '/' => v / r,
                _ => v % r,
            };
        }
        Ok(v)
    }

    fn unary(&mut self) -> Result<i64, String> {
        if self.peek_op() == Some('-') {
            self.pos += 1;
            return self.unary()?.checked_neg().ok_or_else(|| "overflow".into());
        }
        self.atom()
    }

    fn atom(&mut self) -> Result<i64, String> {
        let tok = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        match tok {
            Some(Tok::Num(v)) => Ok(v),
            Some(Tok::N) => Ok(self.n),
            Some(Tok::Op('(')) => {
                let v = self.sum()?;
                if self.tokens.get(self.pos) != Some(&Tok::Op(')')) {
                    return Err("missing `)`".into());
                }
                self.pos += 1;
                Ok(v)
            }
            Some(t) => Err(format!("unexpected `{t}`")),
            None => Err("expression ends early".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates() {
        assert_eq!(expand("I{n}", 3).unwrap(), "I3");
        assert_eq!(expand("%IX0.{n-1}", 1).unwrap(), "%IX0.0");
        assert_eq!(expand("%IX{(n-1)/8}.{(n-1)%8}", 10).unwrap(), "%IX1.1");
        assert_eq!(expand("{n*2 + 1}", 4).unwrap(), "9");
        assert_eq!(expand("{-n}", 4).unwrap(), "-4");
        assert_eq!(expand("a {{b}} c", 1).unwrap(), "a {b} c");
        assert_eq!(expand("plain", 1).unwrap(), "plain");
    }

    #[test]
    fn errors() {
        assert!(expand("{n", 1).unwrap_err().contains("closing"));
        assert!(expand("n}", 1).unwrap_err().contains("opening"));
        assert!(expand("{m}", 1).unwrap_err().contains("only variable"));
        assert!(expand("{n/0}", 1).unwrap_err().contains("division by zero"));
        assert!(expand("{(n}", 1).unwrap_err().contains("missing `)`"));
        assert!(expand("{}", 1).unwrap_err().contains("empty"));
        assert!(expand("{n n}", 1).unwrap_err().contains("unexpected"));
        assert!(expand("{n$}", 1).unwrap_err().contains("unexpected `$`"));
        assert!(has_placeholder("I{n}"));
        assert!(!has_placeholder("a {{b}}"));
    }
}
