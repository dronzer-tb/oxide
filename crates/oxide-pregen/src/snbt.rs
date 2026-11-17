//! The SNBT subset Oxide's decorator writes for block entities (`oxide_chunkgen` `block_entity`):
//! compounds, lists, quoted strings and numbers with an optional `b`/`s`/`L`/`f`/`d` suffix.
//! Typed arrays (`[I;..]`) and unquoted strings are not produced there and are refused here.

use std::collections::HashMap;

use anyhow::{anyhow, bail, Result};
use fastnbt::Value;

pub fn parse(text: &str) -> Result<Value> {
    let mut parser = Parser {
        s: text.as_bytes(),
        i: 0,
    };
    let value = parser.value()?;
    parser.skip_ws();
    if parser.i != parser.s.len() {
        bail!("trailing characters at {} in {text}", parser.i);
    }
    Ok(value)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.s.get(self.i).copied()
    }

    fn expect(&mut self, c: u8) -> Result<()> {
        if self.peek() != Some(c) {
            bail!("expected '{}' at {}", c as char, self.i);
        }
        self.i += 1;
        Ok(())
    }

    fn value(&mut self) -> Result<Value> {
        match self.peek() {
            Some(b'{') => self.compound(),
            Some(b'[') => self.list(),
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(_) => self.number(),
            None => bail!("unexpected end"),
        }
    }

    fn compound(&mut self) -> Result<Value> {
        self.expect(b'{')?;
        let mut map = HashMap::new();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Value::Compound(map));
        }
        loop {
            let key = if self.peek() == Some(b'"') {
                self.string()?
            } else {
                let start = self.i;
                while self.i < self.s.len()
                    && (self.s[self.i].is_ascii_alphanumeric() || b"_-.+".contains(&self.s[self.i]))
                {
                    self.i += 1;
                }
                String::from_utf8(self.s[start..self.i].to_vec())?
            };
            self.expect(b':')?;
            map.insert(key, self.value()?);
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Compound(map));
                }
                _ => bail!("expected ',' or '}}' at {}", self.i),
            }
        }
    }

    fn list(&mut self) -> Result<Value> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Value::List(items));
        }
        loop {
            items.push(self.value()?);
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::List(items));
                }
                _ => bail!("expected ',' or ']' at {}", self.i),
            }
        }
    }

    fn string(&mut self) -> Result<String> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        while let Some(&c) = self.s.get(self.i) {
            self.i += 1;
            match c {
                b'"' => return Ok(String::from_utf8(out)?),
                b'\\' => {
                    out.push(
                        *self
                            .s
                            .get(self.i)
                            .ok_or_else(|| anyhow!("dangling escape"))?,
                    );
                    self.i += 1;
                }
                _ => out.push(c),
            }
        }
        bail!("unterminated string")
    }

    fn number(&mut self) -> Result<Value> {
        let start = self.i;
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_digit() || b"-+.eE".contains(&self.s[self.i]))
        {
            self.i += 1;
        }
        let digits = std::str::from_utf8(&self.s[start..self.i])?;
        let suffix = self.s.get(self.i).copied().filter(u8::is_ascii_alphabetic);
        if suffix.is_some() {
            self.i += 1;
        }
        Ok(match suffix.map(|c| c.to_ascii_lowercase()) {
            Some(b'b') => Value::Byte(digits.parse()?),
            Some(b's') => Value::Short(digits.parse()?),
            Some(b'l') => Value::Long(digits.parse()?),
            Some(b'f') => Value::Float(digits.parse()?),
            Some(b'd') => Value::Double(digits.parse()?),
            None if digits.contains('.') => Value::Double(digits.parse()?),
            None => Value::Int(digits.parse()?),
            Some(c) => bail!("unknown number suffix '{}'", c as char),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_what_the_decorator_writes() {
        let v = parse(
            "{id:\"minecraft:mob_spawner\",x:1,y:-2,z:3,Delay:20s,Seed:-5L,\
             SpawnData:{entity:{id:\"minecraft:zombie\"}},SpawnPotentials:[]}",
        )
        .unwrap();
        let Value::Compound(m) = v else { panic!() };
        assert_eq!(m["y"], Value::Int(-2));
        assert_eq!(m["Delay"], Value::Short(20));
        assert_eq!(m["Seed"], Value::Long(-5));
        assert_eq!(m["SpawnPotentials"], Value::List(vec![]));
        let Value::Compound(data) = &m["SpawnData"] else {
            panic!()
        };
        let Value::Compound(entity) = &data["entity"] else {
            panic!()
        };
        assert_eq!(entity["id"], Value::String("minecraft:zombie".into()));
    }
}
