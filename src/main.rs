mod cli;
mod config;
mod container;
mod detector;
mod error;
mod git;
mod orchestrator;
mod tmux;

use clap::Parser;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Failed to create async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match rt.block_on(cli::dispatch(cli.command)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}
