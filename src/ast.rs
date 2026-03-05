use std::fmt;

use crate::codegen::Error;

#[derive(Debug, Clone)]
pub enum Number {
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),

    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),

    F32(f32),
    F64(f64),
}

impl Number {
    pub fn size_bytes(&self) -> u16 {
        self.numtype().size_bytes()
    }

    pub fn numtype(&self) -> NumberType {
        match self {
            Number::I8(_) => NumberType::I8,
            Number::I16(_) => NumberType::I16,
            Number::I32(_) => NumberType::I32,
            Number::I64(_) => NumberType::I64,
            Number::U8(_) => NumberType::U8,
            Number::U16(_) => NumberType::U16,
            Number::U32(_) => NumberType::U32,
            Number::U64(_) => NumberType::U64,
            Number::F32(_) => NumberType::F32,
            Number::F64(_) => NumberType::F64,
        }
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum NumberType {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
}

impl NumberType {
    pub fn size_bytes(&self) -> u16 {
        match self {
            NumberType::I8 => 8 / 8,
            NumberType::I16 => 16 / 8,
            NumberType::I32 => 32 / 8,
            NumberType::I64 => 64 / 8,
            NumberType::U8 => 8 / 8,
            NumberType::U16 => 16 / 8,
            NumberType::U32 => 32 / 8,
            NumberType::U64 => 64 / 8,
            NumberType::F32 => 32 / 8,
            NumberType::F64 => 64 / 8,
        }
    }
}

impl fmt::Display for NumberType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NumberType::I8 => write!(f, "i8"),
            NumberType::I16 => write!(f, "i16"),
            NumberType::I32 => write!(f, "i32"),
            NumberType::I64 => write!(f, "i64"),
            NumberType::U8 => write!(f, "u8"),
            NumberType::U16 => write!(f, "u16"),
            NumberType::U32 => write!(f, "u32"),
            NumberType::U64 => write!(f, "u64"),
            NumberType::F32 => write!(f, "f32"),
            NumberType::F64 => write!(f, "f64"),
        }
    }
}

pub enum Expression {
    Call(String, Vec<Expression>),
    Number(Number),
    Symbol(String),
    BinOp(Box<(Expression, char, Expression)>),
    Ref(Box<Expression>),
    Deref(Box<Expression>),
    As(Box<(ExpressionType, Expression)>),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ExpressionType {
    Number(NumberType),
    Struct(String),
    Ref(Box<ExpressionType>),
    Void,
}

impl ExpressionType {
    pub fn to_number(self) -> Result<NumberType, Error> {
        match self {
            ExpressionType::Number(number) => Ok(number),
            _ => Err(Error::ExpectedNumber(self)),
        }
    }

    pub fn to_ref(self) -> Result<ExpressionType, Error> {
        match self {
            ExpressionType::Ref(expr) => Ok(*expr),
            _ => Err(Error::ExpectedRef(self)),
        }
    }
}

impl fmt::Display for ExpressionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExpressionType::Number(x) => write!(f, "{x}"),
            ExpressionType::Struct(x) => write!(f, "{x}"),
            ExpressionType::Ref(x) => write!(f, "&{x}"),
            ExpressionType::Void => write!(f, "void"),
        }
    }
}

pub enum GlobalValue {
    Function(Function),
    Struct(Struct),
}

pub struct Function {
    pub return_type: ExpressionType,
    pub params: Vec<(ExpressionType, String)>,
    pub body: Vec<Statement>,
}

pub struct Struct(pub Vec<(ExpressionType, String)>);

pub enum Statement {
    DefAssign(DefAssignment),
    SetAssign(SetAssignment),
    Return(Option<Expression>),
    Expr(Expression),
    Block(Vec<Statement>),
}

pub struct DefAssignment {
    pub var_type: ExpressionType,
    pub var_name: String,
    pub var_value: Option<Expression>,
}

pub struct SetAssignment {
    pub var_name: String,
    pub var_value: Expression,
    pub deref: bool,
}
