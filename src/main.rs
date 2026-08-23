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
mod select;
mod selection;
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
    /// Manage plugins (passthrough to upstream `omarchy plugin`)
    Plugin {
        /// Arguments forwarded verbatim, e.g. `opb plugin add owner/repo`
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Enable/disable components (edits the generated shell.json)
    Select {
        #[command(subcommand)]
        cmd: SelectCmd,
    },
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

#[derive(Subcommand)]
enum SelectCmd {
    /// Enable a component (bar-widgets take --section, default right)
    Enable {
        /// Plugin id, e.g. omarchy.clock
        id: String,
        /// Bar layout section for bar-widgets
        #[arg(long, value_parser = ["left", "center", "right"])]
        section: Option<String>,
    },
    /// Disable a component
    Disable { id: String },
}

fn parse_section(s: Option<&str>) -> selection::Section {
    match s {
        Some("left") => selection::Section::Left,
        Some("center") => selection::Section::Center,
        _ => selection::Section::Right,
    }
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
        Command::Select { cmd } => {
            let paths = paths::Paths::from_env();
            let action = match cmd {
                SelectCmd::Enable { id, section } => select::Action::Enable {
                    id,
                    section: parse_section(section.as_deref()),
                },
                SelectCmd::Disable { id } => select::Action::Disable { id },
            };
            match select::apply(&paths, &action) {
                Ok(outcome) => {
                    println!("opb select: {} ({})", outcome.note(), if outcome.changed() { "changed" } else { "unchanged" });
                    if outcome.changed() {
                        match shell::reload_if_running(&paths) {
                            Ok(None) => println!("opb select: shell not running — change applies on next `opb up`"),
                            Ok(Some(warning)) if warning.is_empty() => println!("opb select: shell reloaded"),
                            Ok(Some(warning)) => println!("opb select: {warning} — restart with `opb down && opb up` to be sure"),
                            Err(e) => eprintln!("opb select: reload check failed: {e:#}"),
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("opb select: {e:#}");
                    ExitCode::from(exit::FAIL)
                }
            }
        }
        Command::Update => not_implemented("update"),
        Command::Theme => not_implemented("theme"),
    }
}
