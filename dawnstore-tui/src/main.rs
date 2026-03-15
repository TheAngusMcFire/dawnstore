#![allow(dead_code)] // remove once update.rs drives all variants

use std::{io::stdout, path::PathBuf, time::Duration};

use clap::Parser;
use color_eyre::eyre::Result;
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;
use tracing::info;

mod api_task;
mod app;
mod event;
mod ui;
mod update;

#[derive(Parser, Debug)]
#[command(name = "dawnstore-tui", version, about = "TUI for dawnstore")]
struct Args {
    /// Path to the context YAML file (url + optional token).
    #[arg(short, long, env = "DAWNSTORE_CONTEXT")]
    context_path: String,

    /// Bearer token. Takes precedence over the token in the context file.
    #[arg(long, env = "DAWNSTORE_TOKEN")]
    token: Option<String>,
}

// ── Terminal lifecycle ────────────────────────────────────────────────────────

/// RAII guard that restores the terminal on drop (normal exit or panic).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
}

/// Install a panic hook that restores the terminal and flushes the log before
/// printing the panic message, so the last recorded event is always on disk.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        default(info);
    }));
}

// ── Logging ───────────────────────────────────────────────────────────────────

/// Initialise file-based logging. Returns the guard that must be held for the
/// duration of the program to keep the non-blocking writer alive.
fn init_logging() -> tracing_appender::non_blocking::WorkerGuard {
    let log_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dawnstore-tui");
    let _ = std::fs::create_dir_all(&log_dir);
    let file_appender = tracing_appender::rolling::never(log_dir, "app.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(non_blocking)
        .with_ansi(false) // no colour codes in the log file
        .init();
    guard
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    // Hold the guard for the entire program — dropping it flushes the log.
    let _log_guard = init_logging();
    install_panic_hook();

    let args = Args::parse();
    let file = std::fs::read_to_string(&args.context_path).map_err(|e| {
        color_eyre::eyre::eyre!(
            "could not read context file '{}': {e}\nHint: set --context-path or DAWNSTORE_CONTEXT",
            args.context_path
        )
    })?;
    let context = serde_yml::from_str::<dawnstore_client_lib::Context>(&file)?;

    let token = args.token.or(context.token);
    let api = match token {
        Some(t) => dawnstore_client_lib::Api::new_with_token(&context.url, t),
        None => dawnstore_client_lib::Api::new(&context.url),
    };

    info!("dawnstore-tui starting");

    // ── Channel setup ─────────────────────────────────────────────────────────
    // Single event channel: all producers (input thread, timer, api task) send
    // `Event`s to the main loop via cloned senders.
    let (event_tx, mut event_rx) = mpsc::channel::<event::Event>(64);
    // Command channel: main loop sends `Command`s to the api task.
    let (cmd_tx, cmd_rx) = mpsc::channel::<event::Command>(32);

    // ── Spawn producers ───────────────────────────────────────────────────────

    // Input thread: crossterm::event::read() blocks, so it runs on a dedicated
    // OS thread rather than inside the async runtime.
    let input_tx = event_tx.clone();
    std::thread::spawn(move || {
        loop {
            match crossterm::event::read() {
                Ok(crossterm::event::Event::Key(key)) => {
                    if input_tx.blocking_send(event::Event::Key(key)).is_err() {
                        break; // receiver dropped — main loop exited
                    }
                }
                Ok(_) => {} // ignore resize / mouse / etc. for now
                Err(_) => break,
            }
        }
    });

    // Timer task: sends a Tick every 2 seconds to trigger a background refresh.
    let tick_tx = event_tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            interval.tick().await;
            if tick_tx.send(event::Event::Tick).await.is_err() {
                break;
            }
        }
    });

    // API task: executes commands against the dawnstore API.
    tokio::spawn(api_task::run(api, cmd_rx, event_tx));

    // ── Terminal setup ────────────────────────────────────────────────────────
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let _terminal_guard = TerminalGuard; // restores terminal on drop
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    // ── Main loop ─────────────────────────────────────────────────────────────
    let mut app = app::App::default();

    // Trigger an initial fetch.
    let _ = cmd_tx
        .send(event::Command::Refresh {
            namespace: Some(app.namespace.clone()),
            kind: app.kind_filter.clone(),
        })
        .await;

    loop {
        terminal.draw(|f| ui::render(&app, f))?;

        let Some(ev) = event_rx.recv().await else {
            break; // all senders dropped
        };

        // Check for quit before passing to the generic update function.
        if let event::Event::Key(key) = &ev {
            use crossterm::event::KeyCode;
            if matches!(key.code, KeyCode::Char('q')) && app.view == app::View::ResourceList {
                break;
            }
        }

        if let Some(cmd) = update::update(&mut app, ev) {
            let _ = cmd_tx.send(cmd).await;
        }
    }

    info!("dawnstore-tui exiting");
    Ok(())
}
