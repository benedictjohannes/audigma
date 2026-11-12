//! `audigma completions` and `audigma man`: shell completions and manpages generated from the
//! same clap command tree the binary parses with, so they cannot drift from the real CLI.

use std::io::Write;
use std::path::PathBuf;

use clap::{Args, CommandFactory};
use clap_complete::Shell;

use super::Cli;
use crate::error::CliError;

#[derive(Args, Debug)]
pub struct CompletionsArgs {
    /// Shell to generate completions for
    #[arg(value_enum)]
    pub shell: Shell,
}

#[derive(Args, Debug)]
pub struct ManArgs {
    /// Write audigma.1 plus one page per subcommand (audigma-run.1, ...) into this directory
    /// instead of printing the top-level page to stdout
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
}

/// Prints the completion script for `shell` to stdout.
pub fn run_completions(args: CompletionsArgs) -> Result<(), CliError> {
    let mut out = Vec::new();
    write_completions(args.shell, &mut out);
    std::io::stdout().write_all(&out)?;
    Ok(())
}

pub fn write_completions(shell: Shell, out: &mut dyn Write) {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "audigma", out);
}

/// Prints the roff manpage to stdout, or writes one page per command into `--out`.
pub fn run_man(args: ManArgs) -> Result<(), CliError> {
    let cmd = Cli::command();
    match args.out {
        None => {
            let mut out = Vec::new();
            clap_mangen::Man::new(cmd).render(&mut out)?;
            std::io::stdout().write_all(&out)?;
        }
        Some(dir) => {
            std::fs::create_dir_all(&dir)?;
            clap_mangen::generate_to(cmd, &dir)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;

    const SUBCOMMANDS: &[&str] = &[
        "run",
        "play",
        "convert",
        "benchmark",
        "probe",
        "config",
        "configure",
        "completions",
        "man",
        "tui",
        "web",
    ];

    #[test]
    fn completions_cover_every_shell_and_subcommand() {
        for shell in Shell::value_variants() {
            let mut out = Vec::new();
            write_completions(*shell, &mut out);
            let text = String::from_utf8(out).unwrap();
            assert!(!text.is_empty(), "{shell}: empty");
            for name in SUBCOMMANDS {
                assert!(text.contains(name), "{shell}: missing `{name}`");
            }
        }
    }

    #[test]
    fn manpage_is_roff_with_subcommands_and_env() {
        let mut out = Vec::new();
        clap_mangen::Man::new(Cli::command())
            .render(&mut out)
            .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.lines().any(|l| l.starts_with(".TH audigma")),
            "not roff"
        );
        for name in SUBCOMMANDS.iter().filter(|n| !["tui", "web"].contains(n)) {
            assert!(text.contains(name), "missing `{name}`");
        }
        assert!(text.contains("AUDIGMA_CONFIG"), "config path order missing");
        let mut sub = Vec::new();
        let cmd = Cli::command();
        let run = cmd.find_subcommand("run").unwrap().clone();
        clap_mangen::Man::new(run).render(&mut sub).unwrap();
        assert!(String::from_utf8(sub).unwrap().contains("AUDIGMA_CONFIG"));
    }

    #[test]
    fn man_out_dir_writes_a_page_per_command() {
        let dir = tempfile::tempdir().unwrap();
        clap_mangen::generate_to(Cli::command(), dir.path()).unwrap();
        for page in [
            "audigma.1",
            "audigma-run.1",
            "audigma-configure.1",
            "audigma-configure-web.1",
        ] {
            assert!(dir.path().join(page).is_file(), "missing {page}");
        }
    }
}
