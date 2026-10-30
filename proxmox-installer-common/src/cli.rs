//! Provides a simple command line parsing interface, with special support for
//! (one-level deep) subcommands.

use std::process;

use anyhow::Result;

pub use pico_args::Arguments;

pub trait Subcommand {
    /// Parses the arguments for this command from an [`pico_args::Arguments`].
    fn parse(args: &mut Arguments) -> Result<Self>
    where
        Self: Sized;

    /// Print command usage to stderr.
    fn print_usage()
    where
        Self: Sized;

    /// Runs the commands action.
    fn run(&self) -> Result<()>;
}

pub struct AppInfo<'a> {
    pub global_help: &'a str,
    pub on_command: fn(Option<&str>, &mut Arguments) -> Result<()>,
}

/// Error of a command that was already reported.
#[derive(Debug)]
struct CommandFailed;

impl std::fmt::Display for CommandFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("command failed")
    }
}

impl std::error::Error for CommandFailed {}

pub fn run(info: AppInfo) -> process::ExitCode {
    match parse_args(&info) {
        Ok(()) => process::ExitCode::SUCCESS,
        Err(err) if err.is::<CommandFailed>() => process::ExitCode::FAILURE,
        Err(err) => {
            eprintln!("Error: {err:#}\n\n{}", info.global_help);
            process::ExitCode::FAILURE
        }
    }
}

fn parse_args(info: &AppInfo) -> Result<()> {
    let mut args = pico_args::Arguments::from_env();
    let subcommand = args.subcommand()?;

    if subcommand.is_none() && args.contains(["-h", "--help"]) {
        eprintln!("{}", info.global_help);
        Ok(())
    } else if args.contains(["-V", "--version"]) {
        eprintln!("{} v{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        Ok(())
    } else {
        (info.on_command)(subcommand.as_deref(), &mut args)
    }
}

pub fn handle_command<T: Subcommand>(args: &mut pico_args::Arguments) -> Result<()> {
    if args.contains(["-h", "--help"]) {
        T::print_usage();
    } else if let Err(err) = T::parse(args).and_then(|cmd| cmd.run()) {
        eprintln!("Error: {err:#}");
        return Err(CommandFailed.into());
    }

    Ok(())
}
