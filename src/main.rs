#![allow(clippy::eq_op)]

use std::{
    fs::File,
    io::{self, Cursor},
    process::exit,
};

use codegen::Codegen;
use parse::Parser;

mod ast;
mod codegen;
mod parse;
mod tokenize;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut rdr = Cursor::new(include_str!("../input"));

    match run(&mut rdr) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("{e}");
            exit(1);
        }
    };

    Ok(())
}

fn run<R: io::Read>(rdr: R) -> Result<(), Box<dyn std::error::Error>> {
    let mut parser = Parser::new(rdr)?;
    let mut codegen = Codegen::new(File::create("output")?);

    codegen.init()?;

    while let Some((name, value)) = parser.next_global()? {
        codegen.codegen(name, value)?;
    }

    codegen.deinit()?;

    Ok(())
}
