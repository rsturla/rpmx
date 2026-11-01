use std::process::ExitCode;

use clap::Parser;

mod cli;
mod commands;
mod exit;
mod render;

use cli::Cli;

fn main() -> ExitCode {
    // Behave like a normal Unix filter when the reader closes the pipe.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    let json = cli.json();
    let command = cli.command_name();
    // Guest output must never share stdout with a JSON document.
    rpmx_core::output::set_logs_to_stderr(json);
    rpmx_core::output::set_keep_workdir(cli.keep_workdir);
    rpmx_core::output::set_debug(cli.debug);
    match commands::dispatch::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if error.reported {
                return ExitCode::from(exit::code_for(error.code));
            }
            if json {
                let value = serde_json::json!({
                    "schema": 1,
                    "ok": false,
                    "command": command,
                    "diagnostics": [{
                        "severity": "error",
                        "code": error.code,
                        "message": error.message,
                        "path": error.path,
                        "line": error.line,
                        "column": error.column,
                    }],
                });
                println!("{value}");
            } else {
                eprintln!("{error}");
            }
            ExitCode::from(exit::code_for(error.code))
        }
    }
}
