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

#[derive(Subcommand, Debug, Clone)]
pub enum ConfigureFrontend {
    /// Edit in the terminal (the default)
    Tui,
    /// Edit in a browser: serves a local page and a JSON API until you save or quit
    Web(WebArgs),
}

#[derive(Args, Debug, Clone)]
pub struct WebArgs {
    /// Address to listen on, `IP` or `IP:PORT` (default 127.0.0.1 on a random port). An address
    /// other than loopback serves plain HTTP: the access token is visible on that network.
    #[arg(long, value_name = "ADDR")]
    pub bind: Option<String>,

    /// Stop after this many minutes without a request (0 = never)
    #[arg(long, value_name = "MINUTES", default_value_t = 30)]
    pub idle_timeout: u64,

    /// Print the URL but do not open a browser
    #[arg(long)]
    pub no_open: bool,
}

pub fn run_configure(args: ConfigureArgs) -> Result<(), CliError> {
    let path = super::resolve_config_path(args.config.as_deref());
    match args.frontend.unwrap_or(ConfigureFrontend::Tui) {
        ConfigureFrontend::Tui => run_tui(path),
        ConfigureFrontend::Web(web) => run_web(path, &web),
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

#[cfg(feature = "web")]
fn run_web(path: PathBuf, args: &WebArgs) -> Result<(), CliError> {
    use std::net::{IpAddr, SocketAddr};
    use std::sync::Arc;
    use std::time::Duration;

    use audigma_configurator::web::{EndReason, ServeOptions, Server, Session, SystemBackend};

    let bind: SocketAddr = match args.bind.as_deref() {
        None => SocketAddr::from(([127, 0, 0, 1], 0)),
        Some(text) => text
            .parse::<SocketAddr>()
            .or_else(|_| text.parse::<IpAddr>().map(|ip| SocketAddr::new(ip, 0)))
            .map_err(|_| CliError::Config(format!("--bind `{text}`: expected IP or IP:PORT")))?,
    };
    // A file that cannot be loaded is refused before the server starts.
    let session = Session::open(&path).map_err(|e| CliError::Config(e.to_string()))?;
    let opts = ServeOptions {
        bind,
        idle_timeout: (args.idle_timeout > 0).then(|| Duration::from_secs(args.idle_timeout * 60)),
    };
    let server = Server::start(session, Arc::new(SystemBackend), &opts)
        .map_err(|e| CliError::Config(format!("cannot listen on {bind}: {e}")))?;
    if audigma_configurator::web::is_exposed(&server.addr) {
        eprintln!(
            "warning: {} is not a loopback address. The server speaks plain HTTP, so anyone on \
             that network can read the access token. Prefer SSH port forwarding or a TLS proxy.",
            server.addr
        );
    }
    let url = server.url();
    println!("Configuring {}", path.display());
    println!("Open {url}");
    println!("The page signs you in with the token in that URL. Stop with Ctrl-C.");
    if !args.no_open {
        open_browser(&url);
    }
    match server.wait() {
        EndReason::Saved(done) => {
            if done.unchanged {
                println!("{} is already up to date.", done.path);
            } else {
                println!("Wrote {}.", done.path);
            }
            if let Some(b) = &done.backup {
                println!("Backup: {b}");
            }
            println!("Next:\n  {}\n  {}", done.next[0], done.next[1]);
        }
        EndReason::Quit => println!("Quit without saving."),
        EndReason::Idle => println!(
            "Stopped after {} minutes without activity. Nothing was written.",
            args.idle_timeout
        ),
    }
    Ok(())
}

/// Opens `url` in the default browser when there is a graphical session; failures are ignored
/// (the URL is printed anyway).
#[cfg(feature = "web")]
fn open_browser(url: &str) {
    let graphical =
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    if !graphical {
        return;
    }
    let _ = std::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(not(feature = "web"))]
fn run_web(_path: PathBuf, _args: &WebArgs) -> Result<(), CliError> {
    Err(CliError::Config(
        "this build has no web configurator (feature `web`)".into(),
    ))
}

#[cfg(not(feature = "tui"))]
fn run_tui(_path: PathBuf) -> Result<(), CliError> {
    Err(CliError::Config(
        "this build has no terminal configurator (feature `tui`)".into(),
    ))
}
