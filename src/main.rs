mod atomic;
mod bootstrap;
mod check;
mod deps;
mod env;
mod fonts;
mod git;
mod hypr;
mod paths;
mod pin;
mod pin_update;
mod plugin;
mod plugin_edit;
mod plugin_list;
mod prompt;
mod selfupdate;
mod shell;
mod shelljson;
mod status;

use clap::{Args, Parser, Subcommand};
use std::process::ExitCode;

mod exit {
    pub const OK: u8 = 0;
    pub const FAIL: u8 = 1;
}

mod keys;

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
    /// Clone and pin upstream omarchy, generate shell.json
    Bootstrap(BootstrapArgs),
    /// Install session wiring: autostart the shell each Hyprland start
    /// (idempotent). Offers to add require("opb") to your Hyprland Lua config
    /// as a marker-managed block
    Enable(EnableArgs),
    /// Remove the session wiring (keybinds in keys.lua stay yours)
    Disable(DisableArgs),
    /// Launch the pinned shell
    Up,
    /// Stop the shell
    Down,
    /// Minimal doctor: pin state, session wiring, shell process
    Status,
    /// Manage keybinds for shell/plugin actions (zero binds by default, D11).
    Keys {
        #[command(subcommand)]
        command: KeysCommand,
    },
    /// Manage plugins — acquire/activate/inspect; the single mutation path
    /// (passthrough to upstream, D13)
    Plugin {
        #[command(subcommand)]
        command: Option<PluginCommand>,
    },
    /// Manage the upstream pin — update flips to a newer release, rollback
    /// undoes the last flip
    Pin {
        #[command(subcommand)]
        command: PinCommand,
    },
    /// Update the opb binary itself from the newest GitHub release
    Update(UpdateArgs),
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
struct EnableArgs {
    /// Accept prompts (write the require block without asking)
    #[arg(long)]
    yes: bool,
    /// Never touch hyprland.lua — print the activation line instead
    #[arg(long = "no-line")]
    no_line: bool,
    /// Also start the shell right now (autostart otherwise applies to the
    /// next Hyprland start only — exec-once semantics never fire on reload)
    #[arg(long)]
    now: bool,
}

#[derive(Args)]
struct DisableArgs {
    /// Also stop the running shell (mirror of `enable --now`)
    #[arg(long)]
    now: bool,
}

#[derive(Args)]
struct UpdateArgs {
    /// Skip the confirmation prompt
    #[arg(long)]
    yes: bool,
    /// Report whether a newer release exists without downloading it
    #[arg(long)]
    check: bool,
}

#[derive(Subcommand)]
enum PinCommand {
    /// Preview → confirm → flip to a newer pin (fresh validated clone,
    /// symlink flip, down-window shell.json reconciliation)
    Update(PinUpdateArgs),
    /// Flip back to the previous pin generation
    Rollback(PinRollbackArgs),
}

#[derive(Args)]
struct PinUpdateArgs {
    /// Target ref to pin (a release tag); defaults to the newest tag
    #[arg(long = "ref", value_name = "REF")]
    reference: Option<String>,
    /// First-party id renamed upstream, as OLD=NEW (repeatable)
    #[arg(long = "rename", value_name = "OLD=NEW")]
    renames: Vec<String>,
    /// Skip the confirmation prompt
    #[arg(long)]
    yes: bool,
}

#[derive(Args)]
struct PinRollbackArgs {
    /// First-party id renamed upstream, as OLD=NEW (repeatable)
    #[arg(long = "rename", value_name = "OLD=NEW")]
    renames: Vec<String>,
    /// Skip the confirmation prompt
    #[arg(long)]
    yes: bool,
}

#[derive(Subcommand)]
enum KeysCommand {
    /// Interactive editor: every bindable action with its bind state; enter
    /// to edit a combo (pre-filled), empty input to unbind, esc to quit
    Edit,
    /// List bindable actions × bind state (enabled plugins only unless
    /// --all; narrow with --plugin)
    List {
        /// Include disabled components' actions
        #[arg(long)]
        all: bool,
        /// Only actions for this component id
        #[arg(long = "plugin", value_name = "ID")]
        plugin: Option<String>,
    },
    /// Bind an action to a combo (upstream style, e.g. "SUPER + CTRL + E");
    /// rebinds the action if it is already bound
    Set {
        /// Action id from `opb keys list`, e.g. omarchy.emojis:toggle
        action: String,
        /// Combo, e.g. "SUPER + CTRL + E" or "XF86AudioPlay"
        combo: String,
    },
}

/// `opb plugin …` — known upstream verbs are modeled for help + completion;
/// each keeps `trailing_var_arg` so its inner args forward verbatim. Unknown
/// verbs fall through to `External` and forward verbatim too, preserving the
/// thin passthrough (anti-duplication §5.3, D13).
#[derive(Subcommand)]
enum PluginCommand {
    /// Add a plugin from git
    Add {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Clone a built-in plugin into your config
    Clone {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Remove an installed plugin
    Remove {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Update git-managed plugins
    Update {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Interactive editor: every plugin with its state; widgets get a
    /// left/center/right/off section, everything else on/off
    Edit {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Enable a plugin
    Enable {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Disable a plugin
    Disable {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List installed plugins
    List {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Validate a manifest folder
    Validate {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Unknown plugin subcommand — forwarded verbatim to upstream
    #[command(external_subcommand)]
    External(Vec<String>),
}

/// Rebuild the verbatim `plugin …` arg vector from a parsed subcommand so the
/// existing routing in [`crate::plugin`] stays untouched.
fn plugin_args(command: PluginCommand) -> Vec<String> {
    match command {
        PluginCommand::Add { args } => chain_verb("add", args),
        PluginCommand::Clone { args } => chain_verb("clone", args),
        PluginCommand::Remove { args } => chain_verb("remove", args),
        PluginCommand::Update { args } => chain_verb("update", args),
        PluginCommand::Edit { args } => chain_verb("edit", args),
        PluginCommand::Enable { args } => chain_verb("enable", args),
        PluginCommand::Disable { args } => chain_verb("disable", args),
        PluginCommand::List { args } => chain_verb("list", args),
        PluginCommand::Validate { args } => chain_verb("validate", args),
        PluginCommand::External(args) => args,
    }
}

fn chain_verb(verb: &str, args: Vec<String>) -> Vec<String> {
    std::iter::once(verb.to_string()).chain(args).collect()
}

/// `opb enable` — install/regenerate the managed opb.lua against the active
/// pin (D15), then handle the activation line: write it as a marker-managed
/// block on consent, or print it for manual pasting.
fn enable(paths: &paths::Paths, args: &EnableArgs) -> anyhow::Result<()> {
    let Some((commit, _pin_dir)) = pin::active_pin(paths)? else {
        anyhow::bail!("not bootstrapped yet — run `opb bootstrap` first");
    };
    let report = hypr::enable(paths)?;
    println!("opb enable: session wiring installed for pin @ {}", commit);
    if report.legacy_conf_removed {
        println!("  removed stale managed opb.conf (hyprlang era)");
    }
    if !report.needs_require_line {
        println!("  already activated via require(\"opb\") in your Hyprland config");
        return Ok(());
    }
    let hyprland_lua = paths.hyprland_lua();
    if args.no_line {
        print_require_hint(&hyprland_lua);
        return Ok(());
    }
    let write = args.yes
        || (std::io::IsTerminal::is_terminal(&std::io::stdin())
            && prompt::confirm(
                &format!(
                    "Add {} to {}",
                    hypr::require_hint(),
                    hyprland_lua.display()
                ),
                true,
            ));
    if write {
        if hypr::write_require_block(paths)? {
            println!("  added managed activation block to {}", hyprland_lua.display());
        } else {
            println!("  activation already present in {}", hyprland_lua.display());
        }
    } else {
        print_require_hint(&hyprland_lua);
    }
    if !args.now {
        println!(
            "  autostart applies from the next Hyprland start; use --now to start it right away"
        );
    }
    Ok(())
}

fn print_require_hint(hyprland_lua: &std::path::Path) {
    println!("  activate by adding this line to {}:", hyprland_lua.display());
    println!("    {}", hypr::require_hint());
}

/// `opb up` semantics — start the shell and the keybind keeper. The keeper is
/// the single registrar: it registers keys.lua on connect and re-registers
/// after every reload (hl.bind adds, never overwrites, so two registrars
/// would double-bind). Only when the keeper can't start is a bind registered
/// directly as a fallback.
fn up_all(paths: &paths::Paths) -> anyhow::Result<()> {
    shell::up(paths)?;
    match keys::spawn_watch(paths) {
        Ok(true) => println!(
            "opb up: keybind keeper started — keys.lua binds live and \
             survive `hyprctl reload`"
        ),
        Ok(false) => println!("opb up: keybind keeper already running"),
        Err(e) => {
            eprintln!("opb up: warning: {e:#}");
            match keys::apply_live(paths) {
                Ok(n) if n > 0 => println!("opb up: registered {n} bind(s) (no keeper)"),
                Ok(_) => {}
                Err(e2) => eprintln!("opb up: warning: {e2:#}"),
            }
        }
    }
    Ok(())
}

/// `opb down` semantics — stop the keeper, stop the shell, unbind.
fn down_all(paths: &paths::Paths) -> anyhow::Result<()> {
    keys::stop_watch(paths);
    shell::down(paths)?;
    if let Err(e) = keys::clear_live(paths) {
        eprintln!("opb down: warning: {e:#}");
    }
    Ok(())
}

/// `opb disable` — remove the managed wiring: activation block, opb.lua, and
/// any stale legacy artifacts, in an order that keeps every intermediate
/// state re-parse-consistent. keys.lua and the rest of the user's config are
/// untouched (D15).
fn disable(paths: &paths::Paths) -> anyhow::Result<()> {
    let report = hypr::disable(paths)?;
    if report.lua_removed {
        println!("opb disable: removed {}", paths.opb_lua().display());
    } else {
        println!("opb disable: no session wiring installed");
    }
    if report.block_removed {
        println!("  removed managed activation block from {}", paths.hyprland_lua().display());
    }
    println!("  keys.lua stays yours: {}", paths.keys_lua().display());
    Ok(())
}

fn main() -> ExitCode {
    // Internal keeper entry: `opb up` (and the boot autostart) re-exec this
    // binary with OPB_WATCH_DAEMON set. Deliberately not a subcommand — the
    // keeper is a subprocess of `opb up`, not something a user runs.
    if std::env::var_os("OPB_WATCH_DAEMON").is_some() {
        let paths = paths::Paths::from_env();
        return match keys::watch(&paths) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("opb keeper: {e:#}");
                ExitCode::from(exit::FAIL)
            }
        };
    }

    let cli = Cli::parse();
    match cli.command {
        Command::Bootstrap(args) => {
            let paths = paths::Paths::from_env();
            let opts = bootstrap::BootstrapOptions {
                reference: args.reference,
                redo: args.redo,
            };
            match bootstrap::require_dependencies().and_then(|()| bootstrap::run(&paths, &opts)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("opb bootstrap: {e:#}");
                    ExitCode::from(exit::FAIL)
                }
            }
        }
        Command::Enable(args) => {
            let paths = paths::Paths::from_env();
            let done = enable(&paths, &args).and_then(|()| {
                // --now: start immediately; autostart alone only covers the
                // next Hyprland start (exec-once semantics never fire on
                // reload). Starting now is exactly `opb up`.
                if args.now {
                    up_all(&paths)?;
                }
                Ok(())
            });
            match done {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("opb enable: {e:#}");
                    ExitCode::from(exit::FAIL)
                }
            }
        }
        Command::Disable(args) => {
            let paths = paths::Paths::from_env();
            match disable(&paths) {
                Ok(()) => {
                    // Mirror of enable --now: unwiring now is exactly `opb down`.
                    if args.now
                        && let Err(e) = down_all(&paths)
                    {
                        eprintln!("opb disable: {e:#}");
                        ExitCode::from(exit::FAIL)
                    } else {
                        ExitCode::SUCCESS
                    }
                }
                Err(e) => {
                    eprintln!("opb disable: {e:#}");
                    ExitCode::from(exit::FAIL)
                }
            }
        }
        Command::Up => {
            let paths = paths::Paths::from_env();
            match up_all(&paths) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("opb up: {e:#}");
                    ExitCode::from(exit::FAIL)
                }
            }
        }
        Command::Down => {
            let paths = paths::Paths::from_env();
            match down_all(&paths) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("opb down: {e:#}");
                    ExitCode::from(exit::FAIL)
                }
            }
        }
        Command::Plugin { command } => {
            let paths = paths::Paths::from_env();
            let args = match command {
                Some(cmd) => plugin_args(cmd),
                None => Vec::new(),
            };
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
            let report = status::report_with(&paths, status::opb_version_probe);
            print!("{}", report.render());
            ExitCode::from(report.exit_code())
        }
        Command::Keys { command } => {
            let paths = paths::Paths::from_env();
            match command {
                KeysCommand::Edit => match keys::edit(&paths) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("opb keys: {e:#}");
                        ExitCode::from(exit::FAIL)
                    }
                },
                KeysCommand::List { all, plugin } => match keys::catalog(&paths) {
                    Ok(entries) => {
                        let keys_src =
                            std::fs::read_to_string(paths.keys_lua()).unwrap_or_default();
                        print!("{}", keys::render(&entries, &keys_src, all, plugin.as_deref()));
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("opb keys list: {e:#}");
                        ExitCode::from(exit::FAIL)
                    }
                },
                KeysCommand::Set { action, combo } => {
                    match keys::set(&paths, &action, &combo) {
                        Ok(()) => ExitCode::SUCCESS,
                        Err(e) => {
                            eprintln!("opb keys set: {e:#}");
                            ExitCode::from(exit::FAIL)
                        }
                    }
                }
            }
        }
        Command::Pin { command } => {
            let paths = paths::Paths::from_env();
            let (label, result) = match command {
                PinCommand::Update(args) => {
                    let renames = match args
                        .renames
                        .iter()
                        .map(|r| pin_update::parse_rename(r))
                        .collect::<Result<Vec<_>, _>>()
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("opb pin update: {e:#}");
                            return ExitCode::from(exit::FAIL);
                        }
                    };
                    (
                        "opb pin update",
                        pin_update::run(
                            &paths,
                            &pin_update::UpdateOptions {
                                reference: args.reference,
                                renames,
                                yes: args.yes,
                            },
                        ),
                    )
                }
                PinCommand::Rollback(args) => {
                    let renames = match args
                        .renames
                        .iter()
                        .map(|r| pin_update::parse_rename(r))
                        .collect::<Result<Vec<_>, _>>()
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("opb pin rollback: {e:#}");
                            return ExitCode::from(exit::FAIL);
                        }
                    };
                    (
                        "opb pin rollback",
                        pin_update::rollback(
                            &paths,
                            &pin_update::RollbackOptions { renames, yes: args.yes },
                        ),
                    )
                }
            };
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{label}: {e:#}");
                    ExitCode::from(exit::FAIL)
                }
            }
        }
        Command::Update(args) => {
            let result = if args.check {
                selfupdate::check()
            } else {
                selfupdate::run(args.yes)
            };
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("opb update: {e:#}");
                    ExitCode::from(exit::FAIL)
                }
            }
        }
    }
}
