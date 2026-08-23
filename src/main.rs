use clap::{Parser, Subcommand};
use std::process::ExitCode;

mod exit {
    #![allow(dead_code)]

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
    Bootstrap,
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

fn not_implemented(cmd: &str) -> ! {
    eprintln!("opb {cmd}: not implemented yet");
    std::process::exit(i32::from(exit::ERROR));
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Doctor => not_implemented("doctor"),
        Command::Bootstrap => not_implemented("bootstrap"),
        Command::Up => not_implemented("up"),
        Command::Down => not_implemented("down"),
        Command::Plugin => not_implemented("plugin"),
        Command::Select => not_implemented("select"),
        Command::Update => not_implemented("update"),
        Command::Theme => not_implemented("theme"),
    }
}
