use std::process::ExitCode;

use clap::Parser;

mod cli;
mod commands;
mod render;

use cli::Cli;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json();
    match commands::dispatch::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if json {
                let value = serde_json::json!({
                    "severity": "error",
                    "code": error.code,
                    "message": error.message,
                    "path": error.path,
                    "line": error.line,
                    "column": error.column,
                });
                eprintln!("{value}");
            } else {
                eprintln!("{error}");
            }
            ExitCode::from(2)
        }
    }
}
