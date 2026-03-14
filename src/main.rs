#![allow(clippy::eq_op)]

use std::{fs::File, io, path::PathBuf, process::exit};

use clap::Parser as _;
use codegen::Codegen;
use parse::Parser;

mod ast;
mod codegen;
mod parse;
mod tokenize;

#[derive(clap::Parser)]
struct Cli {
    /// Path to the input file to be compiled.
    path: PathBuf,
    /// Path to output the NASM assembly file.
    #[clap(long, short)]
    out: Option<PathBuf>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Cli { path, out } = Cli::parse();
    let out = out.unwrap_or(PathBuf::from("a.asm"));

    let rdr = File::open(path)?;
    let mut parser = Parser::new(rdr)?;
    let wtr = File::create(out)?;

    match run(&mut parser, wtr) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("{e} {}", parser.loc());
            exit(1);
        }
    };

    Ok(())
}

fn run<R: io::Read, W: io::Write>(
    parser: &mut Parser<R>,
    wtr: W,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut codegen = Codegen::new(wtr);

    codegen.init()?;

    while let Some((name, value)) = parser.next_global()? {
        codegen.codegen(name, value)?;
    }

    codegen.deinit()?;

    Ok(())
}
