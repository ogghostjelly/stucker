use std::{collections::HashSet, fmt, io, mem};

use crate::{
    ast::{
        DefAssignment, Expression, ExpressionType, ForStatement, Function, GlobalValue, Number,
        NumberType, SetAssignment, Statement, Struct,
    },
    tokenize::{self, Token, Tokenizer},
};

pub struct Parser<R: io::Read> {
    tokenizer: Tokenizer<R>,
    buf: Option<Token>,
}

impl<R: io::Read> Parser<R> {
    pub fn new(rdr: R) -> Result<Self> {
        Self::from_tokenizer(Tokenizer::new(rdr)?)
    }

    pub fn from_tokenizer(mut tokenizer: Tokenizer<R>) -> Result<Self> {
        let buf = tokenizer.next_token()?;
        Ok(Self { tokenizer, buf })
    }

    pub fn next_global(&mut self) -> Result<Option<(String, GlobalValue)>> {
        match self.peek_token() {
            Some(Token::Symbol(sym)) if sym == "struct" => {
                let (name, struc) = self.parse_struct()?;
                Ok(Some((name, GlobalValue::Struct(struc))))
            }
            Some(Token::Symbol(_)) => {
                let (name, function) = self.parse_func()?;
                Ok(Some((name, GlobalValue::Function(function))))
            }
            None => Ok(None),
            _ => Err(Error::ExpectedSymbol),
        }
    }

    pub fn parse_func(&mut self) -> Result<(String, Function)> {
        let return_type = self.parse_type()?;
        let func_name = self.next_symbol()?;
        let params = self.parse_type_array('(', ')')?;

        if let Some(Token::Operand(';')) = self.peek_token() {
            _ = self.next_token()?;
            return Ok((
                func_name,
                Function {
                    return_type,
                    params,
                    body: None,
                },
            ));
        }

        let body = self.parse_block()?;

        Ok((
            func_name,
            Function {
                return_type,
                params,
                body: Some(body),
            },
        ))
    }

    pub fn parse_struct(&mut self) -> Result<(String, Struct)> {
        if self.next_symbol()? != "struct" {
            return Err(Error::ExpectedStruct);
        }

        let name = self.next_symbol()?;

        let body = self.parse_type_array('{', '}')?;
        let mut keys = HashSet::new();

        for (_, field) in &body {
            if !keys.insert(field) {
                return Err(Error::DuplicateField(field.clone()));
            }
        }

        Ok((name, Struct(body)))
    }

    pub fn parse_expr(&mut self) -> Result<Expression> {
        let lhs = self.parse_primary()?;
        self.parse_binop(0, lhs)
    }

    pub fn parse_binop(&mut self, min_prec: u64, mut lhs: Expression) -> Result<Expression> {
        loop {
            let Some((op, prec)) = self.peek_precedence()? else {
                return Ok(lhs);
            };

            if prec < min_prec {
                return Ok(lhs);
            }

            _ = self.next_token()?; // eat the operand token

            let mut rhs = self.parse_primary()?;

            if let Some((_, next_prec)) = self.peek_precedence()?
                && prec < next_prec
            {
                rhs = self.parse_binop(prec + 1, rhs)?;
            }

            lhs = Expression::BinOp(Box::new((lhs, op, rhs)))
        }
    }

    pub fn peek_precedence(&mut self) -> Result<Option<(String, u64)>> {
        match self.peek_token() {
            Some(Token::Operand('+')) => Ok(Some(("+".into(), 20))),
            Some(Token::Operand('-')) => Ok(Some(("-".into(), 20))),
            Some(Token::Operand('*')) => Ok(Some(("*".into(), 40))),
            Some(Token::Operand('/')) => Ok(Some(("/".into(), 40))),
            Some(Token::Operand('%')) => Ok(Some(("%".into(), 40))),

            Some(Token::Operand('&')) => Ok(Some(("&".into(), 60))),
            Some(Token::Operand('|')) => Ok(Some(("|".into(), 60))),
            Some(Token::Operand('^')) => Ok(Some(("^".into(), 60))),
            Some(Token::Operand2('<', '<')) => Ok(Some(("<<".into(), 60))),
            Some(Token::Operand2('>', '>')) => Ok(Some((">>".into(), 60))),

            Some(Token::Operand2('&', '&')) => Ok(Some(("&&".into(), 80))),
            Some(Token::Operand2('|', '|')) => Ok(Some(("||".into(), 80))),

            Some(Token::Operand('>')) => Ok(Some((">".into(), 100))),
            Some(Token::Operand2('>', '=')) => Ok(Some((">=".into(), 100))),
            Some(Token::Operand('<')) => Ok(Some(("<".into(), 100))),
            Some(Token::Operand2('<', '=')) => Ok(Some(("<=".into(), 100))),
            Some(Token::Operand2('=', '=')) => Ok(Some(("==".into(), 100))),
            Some(Token::Operand2('!', '=')) => Ok(Some(("!=".into(), 100))),

            _ => Ok(None),
        }
    }

    pub fn parse_primary(&mut self) -> Result<Expression> {
        let mut expr = self.parse_primary_inner()?;
        loop {
            if let Some(Token::Operand('.')) = self.peek_token() {
                _ = self.next_token()?;
                let field = self.next_symbol()?;
                expr = Expression::FieldAccess(Box::new((expr, field)));
            } else if let Some(Token::Operand('[')) = self.peek_token() {
                _ = self.next_token()?;
                let idx = self.parse_expr()?;
                self.expect_operand(']')?;
                expr = Expression::ArrayAccess(Box::new((expr, idx)));
            } else {
                break;
            }
        }
        Ok(expr)
    }

    pub fn parse_primary_inner(&mut self) -> Result<Expression> {
        match self.peek_token() {
            Some(Token::Operand('&')) => {
                _ = self.next_token()?;
                Ok(Expression::Ref(Box::new(self.parse_primary()?)))
            }
            Some(Token::Operand('*')) => {
                _ = self.next_token()?;
                Ok(Expression::Deref(Box::new(self.parse_primary()?)))
            }
            Some(Token::Operand('[')) => {
                _ = self.next_token()?;
                let len = self.parse_expr()?;
                self.expect_operand(']')?;
                Ok(Expression::InitArray(Box::new((len, self.parse_type()?))))
            }
            Some(Token::Symbol(s)) if s == "as" => {
                _ = self.next_token()?;
                self.expect_operand('(')?;
                let ty = self.parse_type()?;
                self.expect_operand(')')?;
                Ok(Expression::As(Box::new((ty, self.parse_primary()?))))
            }
            Some(Token::String(s)) => {
                let s = s.clone();
                _ = self.next_token()?;
                Ok(Expression::String(s))
            }
            Some(Token::Symbol(_)) => self.parse_ident(),
            Some(Token::Number(_)) => self.parse_number(),
            Some(Token::Operand('(')) => self.parse_paren(),
            _ => Err(Error::UnknownToken),
        }
    }

    fn parse_ident(&mut self) -> Result<Expression> {
        let ident = self.next_symbol()?;

        if !matches!(self.peek_token(), Some(Token::Operand('('))) {
            return Ok(Expression::Symbol(ident));
        }
        _ = self.next_token()?; // eat the '('

        let mut args = vec![];
        loop {
            match self.peek_token() {
                Some(Token::Operand(')')) => {
                    _ = self.next_token()?;
                    break Ok(Expression::Call(ident, args));
                }
                None => return Err(Error::ExpectedOperand(')')),
                _ => args.push(self.parse_expr()?),
            }

            if let Some(Token::Operand(',')) = self.peek_token() {
                _ = self.next_token()?;
            }
        }
    }

    pub fn parse_number(&mut self) -> Result<Expression> {
        let Some(Token::Number(number)) = self.next_token()? else {
            return Err(Error::ExpectedNumber);
        };
        Ok(Expression::Number(number))
    }

    pub fn parse_paren(&mut self) -> Result<Expression> {
        self.expect_operand('(')?;
        let expr = self.parse_expr()?;
        self.expect_operand(')')?;
        Ok(expr)
    }

    pub fn parse_stmt(&mut self) -> Result<Statement> {
        match self.peek_token() {
            Some(Token::Symbol(sym)) if sym == "return" => {
                _ = self.next_token()?;
                if let Some(Token::Operand(';')) = self.peek_token() {
                    Ok(Statement::Return(None))
                } else {
                    Ok(Statement::Return(Some(self.parse_expr()?)))
                }
            }
            Some(Token::Symbol(sym)) if sym == "def" => {
                _ = self.next_token()?;
                Ok(Statement::DefAssign(self.parse_def_assignment()?))
            }
            Some(Token::Symbol(sym)) if sym == "set" => {
                _ = self.next_token()?;
                Ok(Statement::SetAssign(self.parse_set_assignment()?))
            }
            Some(Token::Symbol(sym)) if sym == "breakpoint" => {
                _ = self.next_token()?;
                if let Some(Token::Operand(';')) = self.peek_token() {
                    _ = self.next_token()?;
                }
                Ok(Statement::Breakpoint)
            }
            Some(Token::Symbol(sym)) if sym == "if" => {
                _ = self.next_token()?;
                let condition = self.parse_expr()?;
                let true_block = self.parse_stmt()?;
                let false_block = match self.peek_token() {
                    Some(Token::Symbol(sym)) if sym == "else" => {
                        _ = self.next_token()?;
                        Some(self.parse_stmt()?)
                    }
                    _ => None,
                };
                Ok(Statement::If(Box::new((
                    condition,
                    true_block,
                    false_block,
                ))))
            }
            Some(Token::Symbol(sym)) if sym == "while" => {
                _ = self.next_token()?;
                let condition = self.parse_expr()?;
                let body = self.parse_stmt()?;
                Ok(Statement::While(Box::new((condition, body))))
            }
            Some(Token::Symbol(sym)) if sym == "for" => {
                _ = self.next_token()?;
                self.expect_operand('(')?;
                let init = self.parse_stmt()?;
                self.expect_operand(';')?;
                let cond = self.parse_expr()?;
                self.expect_operand(';')?;
                let inc = self.parse_stmt()?;
                self.expect_operand(')')?;
                let body = self.parse_stmt()?;
                Ok(Statement::For(Box::new(ForStatement {
                    init,
                    cond,
                    inc,
                    body,
                })))
            }
            Some(Token::Operand('{')) => Ok(Statement::Block(self.parse_block()?)),
            None => Err(Error::UnexpectedEof),
            _ => Ok(Statement::Expr(self.parse_expr()?)),
        }
    }

    pub fn parse_set_assignment(&mut self) -> Result<SetAssignment> {
        let deref = if let Some(Token::Operand('*')) = self.peek_token() {
            _ = self.next_token()?;
            true
        } else {
            false
        };

        let var_dest = self.parse_expr()?;

        self.expect_operand('=')?;
        let var_src = self.parse_expr()?;
        Ok(SetAssignment {
            var_dest,
            var_src,
            deref,
        })
    }

    pub fn parse_def_assignment(&mut self) -> Result<DefAssignment> {
        let var_type = self.parse_type()?;
        let var_name = self.next_symbol()?;

        if let Some(Token::Operand(';')) = self.peek_token() {
            return Ok(DefAssignment {
                var_type,
                var_name,
                var_value: None,
            });
        }

        self.expect_operand('=')?;
        let var_value = self.parse_expr()?;
        Ok(DefAssignment {
            var_type,
            var_name,
            var_value: Some(var_value),
        })
    }

    pub fn parse_type(&mut self) -> Result<ExpressionType> {
        if let Some(Token::Operand('[')) = self.peek_token() {
            _ = self.next_token()?;
            self.expect_operand(']')?;
            return Ok(ExpressionType::Array(Box::new(self.parse_type()?)));
        }

        if let Some(Token::Operand('&')) = self.peek_token() {
            _ = self.next_token()?;
            return Ok(ExpressionType::Ref(Box::new(self.parse_type()?)));
        }

        let sym = self.next_symbol()?;
        Ok(match sym.as_str() {
            "i8" => ExpressionType::Number(NumberType::I8),
            "i16" => ExpressionType::Number(NumberType::I16),
            "i32" => ExpressionType::Number(NumberType::I32),
            "i64" => ExpressionType::Number(NumberType::I64),
            "u8" => ExpressionType::Number(NumberType::U8),
            "u16" => ExpressionType::Number(NumberType::U16),
            "u32" => ExpressionType::Number(NumberType::U32),
            "u64" => ExpressionType::Number(NumberType::U64),
            "f32" => ExpressionType::Number(NumberType::F32),
            "f64" => ExpressionType::Number(NumberType::F64),
            "void" => ExpressionType::Void,
            "str" => ExpressionType::Str,
            _ => ExpressionType::Struct(sym),
        })
    }

    pub fn parse_block(&mut self) -> Result<Vec<Statement>> {
        self.expect_operand('{')?;
        let mut body = vec![];
        loop {
            match self.peek_token() {
                Some(Token::Operand('}')) => {
                    _ = self.next_token()?;
                    break Ok(body);
                }
                None => return Err(Error::ExpectedOperand('}')),
                _ => {
                    let stmt = self.parse_stmt()?;
                    if let Some(Token::Operand(';')) = self.peek_token() {
                        _ = self.next_token()?;
                    }
                    body.push(stmt);
                }
            }
        }
    }

    pub fn expect_operand(&mut self, expect: char) -> Result<()> {
        match self.next_token()? {
            Some(Token::Operand(ch)) if ch == expect => Ok(()),
            Some(_) => Err(Error::ExpectedOperand(expect)),
            None => Err(Error::UnexpectedEof),
        }
    }

    pub fn parse_type_array(
        &mut self,
        begin: char,
        end: char,
    ) -> Result<Vec<(ExpressionType, String)>> {
        self.expect_operand(begin)?;

        let mut params = vec![];

        loop {
            if let Some(Token::Operand(ch)) = self.peek_token()
                && *ch == end
            {
                _ = self.next_token()?;
                break Ok(params);
            }

            let param_type = self.parse_type()?;
            let param_name = self.next_symbol()?;

            params.push((param_type, param_name));

            if let Some(Token::Operand(',')) = self.peek_token() {
                _ = self.next_token()?
            }
        }
    }

    pub fn next_symbol(&mut self) -> Result<String> {
        match self.next_token()? {
            Some(Token::Symbol(symbol)) => Ok(symbol),
            Some(_) => Err(Error::ExpectedSymbol),
            None => Err(Error::UnexpectedEof),
        }
    }

    pub fn peek_token(&mut self) -> Option<&Token> {
        self.buf.as_ref()
    }

    pub fn next_token(&mut self) -> Result<Option<Token>> {
        let new = self.tokenizer.next_token()?;
        Ok(mem::replace(&mut self.buf, new))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Tokenizer(#[from] tokenize::Error),
    #[error("unexpected eof")]
    UnexpectedEof,
    #[error("expected symbol")]
    ExpectedSymbol,
    #[error("expected {0}")]
    ExpectedOperand(char),
    #[error("expected struct definition")]
    ExpectedStruct,
    #[error("unknown token")]
    UnknownToken,
    #[error("expected number")]
    ExpectedNumber,
    #[error("duplicate field '{0}'")]
    DuplicateField(String),
}

impl fmt::Debug for GlobalValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Function(x) => write!(f, "fn {x:?}"),
            Self::Struct(x) => write!(f, "struct {x:?}"),
        }
    }
}

impl fmt::Debug for Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DefAssign(x) => write!(f, "def {x:?};"),
            Self::SetAssign(x) => write!(f, "set {x:?};"),
            Self::Return(x) => match x {
                Some(x) => write!(f, "return {x:?};"),
                None => write!(f, "return;"),
            },
            Self::Expr(x) => write!(f, "{x:?};"),
            Self::Block(x) => {
                write!(f, "{{ ")?;
                let mut iter = x.iter();
                if let Some(x) = iter.next() {
                    write!(f, "{x:?}")?;
                }
                for x in iter {
                    write!(f, " {x:?}")?;
                }
                write!(f, " }}")
            }
            Statement::If(inn) => {
                let (cond, true_block, false_block) = inn.as_ref();
                write!(f, "if ({cond:?}) {true_block:?} else {false_block:?}")
            }
            Statement::While(inn) => {
                let (cond, body) = inn.as_ref();
                write!(f, "while ({cond:?}) {body:?}")
            }
            Statement::For(inn) => {
                let ForStatement {
                    init,
                    cond,
                    inc,
                    body,
                } = inn.as_ref();
                write!(f, "for ({init:?}; {cond:?}; {inc:?}) {body:?}")
            }
            Statement::Breakpoint => write!(f, "breakpoint;"),
        }
    }
}

impl fmt::Debug for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expression::Call(x, expressions) => {
                write!(f, "{x}(")?;
                let mut iter = expressions.iter();
                if let Some(expr) = iter.next() {
                    write!(f, "{expr:?}")?;
                }
                for expr in iter {
                    write!(f, ", {expr:?}")?;
                }
                write!(f, ")")
            }
            Expression::Number(number) => match number {
                Number::I8(x) => write!(f, "{x}i8"),
                Number::I16(x) => write!(f, "{x}i16"),
                Number::I32(x) => write!(f, "{x}i32"),
                Number::I64(x) => write!(f, "{x}i64"),
                Number::U8(x) => write!(f, "{x}u8"),
                Number::U16(x) => write!(f, "{x}u16"),
                Number::U32(x) => write!(f, "{x}u32"),
                Number::U64(x) => write!(f, "{x}u64"),
                Number::F32(x) => write!(f, "{x}f32"),
                Number::F64(x) => write!(f, "{x}f64"),
            },
            Expression::Symbol(sym) => write!(f, "{sym:?}"),
            Expression::BinOp(inn) => {
                let (lhs, op, rhs) = inn.as_ref();
                write!(f, "({lhs:?} {op} {rhs:?})")
            }
            Expression::Ref(expr) => write!(f, "&({expr:?})"),
            Expression::Deref(expr) => write!(f, "*({expr:?})"),
            Expression::As(inn) => {
                let (ty, expr) = inn.as_ref();
                write!(f, "as({ty})({expr:?})")
            }
            Expression::InitArray(inn) => {
                let (size, ty) = inn.as_ref();
                write!(f, "[{size:?}]{ty}")
            }
            Expression::FieldAccess(inn) => {
                let (expr, field) = inn.as_ref();
                write!(f, "({expr:?}).{field}")
            }
            Expression::ArrayAccess(inn) => {
                let (expr, idx) = inn.as_ref();
                write!(f, "({expr:?})[{idx:?}]")
            }
            Expression::String(s) => {
                write!(f, "\"{}\"", s.replace("\"", r#"\""#).replace("\\", r#"\\"#))
            }
        }
    }
}

impl fmt::Debug for DefAssignment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.var_value {
            Some(x) => write!(f, "{} {:?} = {x:?}", self.var_type, self.var_name),
            None => write!(f, "{} {:?};", self.var_type, self.var_name),
        }
    }
}

impl fmt::Debug for SetAssignment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.deref {
            write!(f, "*")?;
        }
        write!(f, "{:?} = {:?}", self.var_dest, self.var_src)
    }
}

impl fmt::Debug for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (", self.return_type)?;
        let mut params = self.params.iter();
        if let Some((k, v)) = params.next() {
            write!(f, "{k} {v}")?;
        }
        for (k, v) in params {
            write!(f, ", {k} {v}")?;
        }
        write!(f, ")")?;
        if let Some(body) = &self.body {
            write!(f, " {{ ")?;
            let mut body = body.iter();
            if let Some(x) = body.next() {
                write!(f, "{x:?}")?;
            }
            for x in body {
                write!(f, " {x:?}")?;
            }
            write!(f, " }}")?;
        } else {
            write!(f, ";")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Struct {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{ ")?;
        let mut iter = self.0.iter();
        if let Some((k, v)) = iter.next() {
            write!(f, "{k} {v}")?;
        }
        for (k, v) in iter {
            write!(f, ", {k} {v}")?;
        }
        write!(f, " }}")?;
        Ok(())
    }
}
