mod atomic;
mod bootstrap;
mod check;
mod doctor;
mod env;
mod git;
mod hypr;
mod paths;
mod pin;
mod plugin;
mod plugin_list;
mod shell;
mod shelljson;
mod status;
mod update;

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
    /// Snapshot of pin state, generations, and channel distance
    Status,
    /// Manage plugins — acquire/activate/inspect; the single mutation path
    /// (passthrough to upstream, D13)
    Plugin {
        /// Arguments forwarded verbatim, e.g. `opb plugin add owner/repo`
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Update the upstream pin (preview → confirm → flip, with down-window
    /// shell.json reconciliation)
    Update(UpdateArgs),
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

#[derive(Args)]
struct UpdateArgs {
    #[command(subcommand)]
    command: Option<UpdateCommand>,
    /// Target ref to pin (a release tag); defaults to the newest tag
    #[arg(long = "ref", value_name = "REF", global = true)]
    reference: Option<String>,
    /// First-party id renamed upstream, as OLD=NEW (repeatable)
    #[arg(long = "rename", value_name = "OLD=NEW", global = true)]
    renames: Vec<String>,
    /// Skip the confirmation prompt
    #[arg(long, global = true)]
    yes: bool,
}

#[derive(Subcommand)]
enum UpdateCommand {
    /// Flip back to the previous pin generation
    Rollback,
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
        Command::Plugin { args } => {
            let paths = paths::Paths::from_env();
            match plugin::run(&paths, &args) {
                Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(exit::FAIL)),
                Err(e) => {
                    eprintln!("opb plugin: {e:#}");
                    ExitCode::from(exit::FAIL)
                }
            }
        }
        Command::Status => {
            let paths = paths::Paths::from_env();
            let report = status::report(&paths);
            print!("{}", report.render());
            ExitCode::from(report.exit_code())
        }
        Command::Update(args) => {
            let paths = paths::Paths::from_env();
            let renames = match args
                .renames
                .iter()
                .map(|r| update::parse_rename(r))
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("opb update: {e:#}");
                    return ExitCode::from(exit::FAIL);
                }
            };
            let result = match args.command {
                Some(UpdateCommand::Rollback) => update::rollback(
                    &paths,
                    &update::RollbackOptions { renames, yes: args.yes },
                ),
                None => update::run(
                    &paths,
                    &update::UpdateOptions {
                        reference: args.reference,
                        renames,
                        yes: args.yes,
                    },
                ),
            };
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("opb update: {e:#}");
                    ExitCode::from(exit::FAIL)
                }
            }
        }
        Command::Theme => not_implemented("theme"),
    }
}
