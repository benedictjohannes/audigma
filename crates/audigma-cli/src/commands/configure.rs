//! Handler for `audigma configure`.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::error::CliError;

#[derive(Args, Debug)]
pub struct ConfigureArgs {
    /// Configuration file to edit or create (default: $AUDIGMA_CONFIG, else
    /// ~/.config/audigma/config.toml)
    #[arg(
        short = 'c',
        long = "config",
        visible_alias = "out",
        env = "AUDIGMA_CONFIG",
        global = true
    )]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub frontend: Option<ConfigureFrontend>,
}

#[derive(Subcommand, Debug, Clone, Copy)]
pub enum ConfigureFrontend {
    /// Edit in the terminal (the default)
    Tui,
}

pub fn run_configure(args: ConfigureArgs) -> Result<(), CliError> {
    let path = super::resolve_config_path(args.config.as_deref());
    match args.frontend.unwrap_or(ConfigureFrontend::Tui) {
        ConfigureFrontend::Tui => run_tui(path),
    }
}

#[cfg(feature = "tui")]
fn run_tui(path: PathBuf) -> Result<(), CliError> {
    use std::io::IsTerminal;

    use audigma_configurator::tui::{self, App};

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(CliError::Config(
            "`audigma configure` needs an interactive terminal".into(),
        ));
    }
    // A file that cannot be loaded is reported here, before the screen switches, so the
    // message stays in the terminal. Nothing is edited or written in that case.
    let mut app = App::open(&path).map_err(|e| CliError::Config(e.to_string()))?;
    tui::run(&mut app)?;
    if let Some(Ok(report)) = &app.save.outcome {
        let shown = app.save.path.as_str();
        if report.unchanged {
            println!("{shown} is already up to date.");
        } else {
            println!("Wrote {shown}.");
        }
        if let Some(b) = &report.backup {
            println!("Backup: {}", b.display());
        }
        println!("Next:\n  audigma play -i <file> -c {shown}\n  audigma run -c {shown}");
    }
    Ok(())
}

#[cfg(not(feature = "tui"))]
fn run_tui(_path: PathBuf) -> Result<(), CliError> {
    Err(CliError::Config(
        "this build has no terminal configurator (feature `tui`)".into(),
    ))
}
