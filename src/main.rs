use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "opb", bin_name = "opb", about = "Omarchy plugin bridge")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the doctor checks
    Doctor,
}

fn doctor() {
    println!("Running doctor...");
    // Add your doctor logic here
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Doctor => doctor(),
    }
}