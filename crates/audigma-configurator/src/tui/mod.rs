//! Terminal frontend of `audigma configure`.
//!
//! [`App`] is the pure state machine and [`draw`] the pure renderer (both tested on a
//! `TestBackend`); [`run`] is the only part that owns the terminal and the worker threads.

pub mod app;
mod markdown;
mod view;

use std::io;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use ratatui::crossterm::event::{self, Event};

pub use app::{App, Effect, ListItem, LoadError, Mode, Popup, Screen};
pub use view::{MIN_HEIGHT, MIN_WIDTH, draw, help_lines};

use crate::assist::benchmark::{self, BenchmarkHandle, BenchmarkRequest};
use crate::assist::probe::{ProbeReport, probe_all};

/// Runs the form until the user quits. Restores the terminal on return and on panic.
pub fn run(app: &mut App) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<()> {
    let mut probe_rx: Option<Receiver<ProbeReport>> = None;
    let mut bench: Option<BenchmarkHandle> = None;
    loop {
        terminal.draw(|f| draw(f, app))?;
        if app.quit {
            if let Some(b) = bench.take() {
                b.cancel();
                let _ = b.wait();
            }
            return Ok(());
        }
        let mut effects = Vec::new();
        if event::poll(Duration::from_millis(80))? {
            if let Event::Key(key) = event::read()? {
                effects.extend(app.handle_key(key));
            }
        }
        perform(effects, &mut probe_rx, &mut bench);
        if let Some(rx) = &probe_rx {
            if let Ok(report) = rx.try_recv() {
                app.on_probe(report);
                probe_rx = None;
            }
        }
        if let Some(b) = &bench {
            let mut finished = false;
            let mut effects = Vec::new();
            while let Ok(ev) = b.events.try_recv() {
                finished |= !matches!(ev, benchmark::BenchmarkEvent::Progress { .. });
                effects.extend(app.on_bench(ev));
            }
            if finished {
                if let Some(b) = bench.take() {
                    let _ = b.wait();
                }
            }
            perform(effects, &mut probe_rx, &mut bench);
        }
    }
}

/// Runs the work the app asked for.
fn perform(
    effects: Vec<Effect>,
    probe_rx: &mut Option<Receiver<ProbeReport>>,
    bench: &mut Option<BenchmarkHandle>,
) {
    for effect in effects {
        match effect {
            Effect::Probe => {
                let (tx, rx) = channel();
                std::thread::spawn(move || {
                    let _ = tx.send(probe_all());
                });
                *probe_rx = Some(rx);
            }
            Effect::StartBenchmark(settings) => {
                *bench = Some(benchmark::start(BenchmarkRequest::new(*settings)));
            }
            Effect::CancelBenchmark => {
                if let Some(b) = bench {
                    b.cancel();
                }
            }
        }
    }
}
