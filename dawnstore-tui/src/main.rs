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

/// Install a panic hook that restores the terminal before printing the message,
/// so the last log entry is always captured and the shell is left usable.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        default(info);
    }));
}

// ── Logging ───────────────────────────────────────────────────────────────────

/// Initialise file-based logging. The returned guard must be held for the
/// entire duration of the program to keep the non-blocking writer alive.
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
        .with_ansi(false)
        .init();
    guard
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
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

    // ── Channels ──────────────────────────────────────────────────────────────
    let (event_tx, mut event_rx) = mpsc::channel::<event::Event>(64);
    let (cmd_tx, cmd_rx) = mpsc::channel::<event::Command>(32);

    // ── Input thread (blocking — must not run inside tokio) ───────────────────
    let input_tx = event_tx.clone();
    std::thread::spawn(move || loop {
        match crossterm::event::read() {
            Ok(crossterm::event::Event::Key(key)) => {
                if input_tx.blocking_send(event::Event::Key(key)).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    });

    // ── Timer task ────────────────────────────────────────────────────────────
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

    // ── API task ──────────────────────────────────────────────────────────────
    tokio::spawn(api_task::run(api, cmd_rx, event_tx));

    // ── Terminal setup ────────────────────────────────────────────────────────
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let _guard = TerminalGuard;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    // ── App state ─────────────────────────────────────────────────────────────
    let mut app = app::App::default();

    // Initial fetch.
    let _ = cmd_tx
        .send(event::Command::Refresh {
            namespace: Some(app.namespace.clone()),
            kind: None,
        })
        .await;

    // ── Main loop ─────────────────────────────────────────────────────────────
    loop {
        terminal.draw(|f| ui::render(&app, f))?;

        let Some(ev) = event_rx.recv().await else {
            break;
        };

        match update::update(&mut app, ev) {
            Some(event::Command::Quit) => break,

            Some(event::Command::OpenEditor) => {
                open_editor(&mut app, &mut terminal, &cmd_tx).await?;
            }

            Some(cmd) => {
                let _ = cmd_tx.send(cmd).await;
            }

            None => {}
        }
    }

    info!("dawnstore-tui exiting");
    Ok(())
}

// ── Editor integration ────────────────────────────────────────────────────────

async fn open_editor(
    app: &mut app::App,
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    cmd_tx: &mpsc::Sender<event::Command>,
) -> Result<()> {
    let Some(obj) = app.selected_object() else {
        return Ok(());
    };
    let original_yaml = serde_yml::to_string(obj).unwrap_or_default();

    // Write to a named temp file so the editor can open it.
    let tmp = tempfile::Builder::new().suffix(".yaml").tempfile()?;
    std::fs::write(tmp.path(), &original_yaml)?;
    let tmp_path = tmp.path().to_owned();

    // Suspend the TUI.
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;

    // Launch $EDITOR (or vi as fallback).
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let _ = tokio::process::Command::new(&editor)
        .arg(&tmp_path)
        .status()
        .await;

    // Resume the TUI.
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    terminal.clear()?;

    // If the content changed, apply it.
    if let Ok(new_yaml) = std::fs::read_to_string(&tmp_path) {
        if new_yaml != original_yaml {
            let _ = cmd_tx.send(event::Command::ApplyContent(new_yaml)).await;
        }
    }

    Ok(())
}
