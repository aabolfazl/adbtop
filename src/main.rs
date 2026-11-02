mod actions;
mod adb;
mod agent;
mod cli;
mod collect;
mod config;
mod headless;
mod model;
mod parse;
mod report;
mod session;
mod skill;
mod theme;
mod tui;
mod ui;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };
    let code = runtime.block_on(async move {
        match cli.command {
            Some(command) => headless::run(command, cli.common).await,
            None => tui::run(cli.common, cli.theme).await,
        }
    });
    ExitCode::from(code.clamp(0, 255) as u8)
}
