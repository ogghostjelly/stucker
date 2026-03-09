use std::{
    io::{self, BufReader, Bytes, Read as _},
    num::{ParseFloatError, ParseIntError},
};

use crate::ast::Number;

pub struct Tokenizer<R: io::Read> {
    buf: Option<u8>,
    rdr: Bytes<BufReader<R>>,
}

impl<R: io::Read> Tokenizer<R> {
    pub fn new(rdr: R) -> Result<Self> {
        let mut rdr = BufReader::new(rdr).bytes();
        let buf = rdr.next().transpose()?;
        Ok(Self { buf, rdr })
    }

    pub fn next_token(&mut self) -> Result<Option<Token>> {
        self.skip_whitespace()?;

        match self.peek() {
            Some(ch) if Self::is_begin_number(ch) => {
                _ = self.pop()?;
                Ok(Some(Token::Number(self.parse_number(ch)?)))
            }
            Some(ch) if Self::is_valid_symbol(ch) => Ok(Some(self.parse_symbol()?)),
            Some(ch) if Self::is_begin_comment(ch) => {
                self.parse_comment()?;
                self.next_token()
            }
            Some(ch) => {
                _ = self.pop()?;
                let next_ch = self.peek();

                match (ch, next_ch) {
                    (b'>' | b'<' | b'!' | b'=', Some(b'=')) => {
                        _ = self.pop()?;
                        Ok(Some(Token::Operand2(ch as char, '=')))
                    }
                    (b'&' | b'|' | b'<' | b'>', Some(next_ch)) if ch == next_ch => {
                        _ = self.pop()?;
                        Ok(Some(Token::Operand2(ch as char, ch as char)))
                    }
                    _ => Ok(Some(Token::Operand(ch as char))),
                }
            }
            None => Ok(None),
        }
    }

    fn is_begin_number(ch: u8) -> bool {
        ch.is_ascii_digit()
    }
    fn is_valid_number(ch: u8) -> bool {
        ch.is_ascii_alphanumeric() || ch == b'.'
    }
    fn is_begin_comment(ch: u8) -> bool {
        ch == b'#'
    }
    fn is_valid_symbol(ch: u8) -> bool {
        ch.is_ascii_alphanumeric() || b"_".contains(&ch)
    }

    pub fn parse_comment(&mut self) -> Result<()> {
        self.skip_until(b'\n')?;
        _ = self.pop()?; // eat the newline character
        Ok(())
    }

    pub fn parse_number(&mut self, begin: u8) -> Result<Number> {
        match self.take_while(String::from(begin as char), Self::is_valid_number) {
            Ok(Some(string)) => {
                let opt = string
                    .len()
                    .checked_sub(3)
                    .and_then(|len| string.split_at_checked(len));

                let (left, kind) = match opt {
                    Some(x) => x,
                    None => {
                        if string.contains(".") {
                            return Ok(Number::F64(string.parse()?));
                        } else {
                            return Ok(Number::I32(string.parse()?));
                        }
                    }
                };

                Ok(match kind {
                    "i8" => Number::I8(left.parse()?),
                    "i16" => Number::I16(left.parse()?),
                    "i32" => Number::I32(left.parse()?),
                    "i64" => Number::I64(left.parse()?),
                    "u8" => Number::U8(left.parse()?),
                    "u16" => Number::U16(left.parse()?),
                    "u32" => Number::U32(left.parse()?),
                    "u64" => Number::U64(left.parse()?),
                    "f32" => Number::F32(left.parse()?),
                    "f64" => Number::F64(left.parse()?),
                    _ if string.contains(".") => Number::F64(string.parse()?),
                    _ => Number::I32(string.parse()?),
                })
            }
            Ok(None) => Err(Error::MalformedToken),
            Err(e) => Err(e),
        }
    }

    pub fn parse_symbol(&mut self) -> Result<Token> {
        match self.take_while(String::new(), Self::is_valid_symbol) {
            Ok(Some(string)) => Ok(Token::Symbol(string)),
            Ok(None) => Err(Error::MalformedToken),
            Err(e) => Err(e),
        }
    }

    pub fn peek(&self) -> Option<u8> {
        self.buf
    }

    pub fn skip_whitespace(&mut self) -> Result<()> {
        while matches!(self.peek(), Some(ch) if ch.is_ascii_whitespace()) {
            _ = self.pop()?;
        }
        Ok(())
    }

    pub fn skip_until(&mut self, end: u8) -> Result<()> {
        while matches!(self.peek(), Some(ch) if ch != end) {
            _ = self.pop()?;
        }
        Ok(())
    }

    pub fn pop(&mut self) -> Result<Option<u8>> {
        let old = self.buf;
        self.buf = self.rdr.next().transpose()?;
        Ok(old)
    }

    pub fn take_while(
        &mut self,
        mut string: String,
        mut f: impl FnMut(u8) -> bool,
    ) -> Result<Option<String>> {
        while matches!(self.peek(), Some(ch) if f(ch)) {
            if let Some(ch) = self.pop()? {
                string.push(ch as char);
            }
        }
        Ok(if string.is_empty() {
            None
        } else {
            Some(string)
        })
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("malformed token")]
    MalformedToken,
    #[error("parse int: {0}")]
    ParseInt(#[from] ParseIntError),
    #[error("parse float: {0}")]
    ParseFloat(#[from] ParseFloatError),
}

#[derive(Debug)]
pub enum Token {
    Symbol(String),
    Operand(char),
    Operand2(char, char),
    Number(Number),
}
