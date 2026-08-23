mod atomic;
mod bootstrap;
mod check;
mod doctor;
mod git;
mod hypr;
mod paths;
mod pin;
mod shell;
mod shelljson;

use clap::{Args, Parser, Subcommand};
use std::process::ExitCode;

mod exit {
    #![allow(dead_code)] // ERROR consumed by stubs; OK/FAIL by doctor/report

    pub const OK: u8 = 0;
    pub const FAIL: u8 = 1;
    pub const ERROR: u8 = 2;
}

#[derive(Parser)]
#[command(
    name = "opb",
    bin_name = "opb",
    about = "Bridge to upstream omarchy-shell on raw Arch + Hyprland"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check dependencies and conflicts on this machine
    Doctor,
    /// Clone and pin upstream omarchy, generate shell.json, wire Hyprland
    Bootstrap(BootstrapArgs),
    /// Launch the pinned shell
    Up,
    /// Stop the shell
    Down,
    /// Manage plugins (passthrough to upstream)
    Plugin,
    /// Enable/disable components
    Select,
    /// Update the upstream pin
    Update,
    /// Delegate to upstream theme scripts
    Theme,
}

#[derive(Args)]
struct BootstrapArgs {
    /// Upstream ref to pin (a release tag); defaults to the newest tag
    #[arg(long = "ref", value_name = "TAG")]
    reference: Option<String>,
    /// Regenerate generated artifacts against the existing pin (no re-clone)
    #[arg(long)]
    redo: bool,
}

fn not_implemented(cmd: &str) -> ! {
    eprintln!("opb {cmd}: not implemented yet");
    std::process::exit(i32::from(exit::ERROR));
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Doctor => {
            let report = doctor::report();
            print!("{}", report.render());
            ExitCode::from(report.exit_code())
        }
        Command::Bootstrap(args) => {
            let paths = paths::Paths::from_env();
            let opts = bootstrap::BootstrapOptions {
                reference: args.reference,
                redo: args.redo,
            };
            match bootstrap::run(&paths, &opts) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("opb bootstrap: {e:#}");
                    ExitCode::from(exit::FAIL)
                }
            }
        }
        Command::Up => {
            let paths = paths::Paths::from_env();
            match shell::up(&paths) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("opb up: {e:#}");
                    ExitCode::from(exit::FAIL)
                }
            }
        }
        Command::Down => {
            let paths = paths::Paths::from_env();
            match shell::down(&paths) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("opb down: {e:#}");
                    ExitCode::from(exit::FAIL)
                }
            }
        }
        Command::Plugin => not_implemented("plugin"),
        Command::Select => not_implemented("select"),
        Command::Update => not_implemented("update"),
        Command::Theme => not_implemented("theme"),
    }
}
