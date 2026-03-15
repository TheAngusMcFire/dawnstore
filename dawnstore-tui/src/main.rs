use std::{
    io::stdout,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

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
    // `editor_active` is set while an external editor has the terminal so the
    // input thread doesn't race with the editor for keystrokes.
    let editor_active = Arc::new(AtomicBool::new(false));
    let editor_active_input = editor_active.clone();
    let input_tx = event_tx.clone();
    std::thread::spawn(move || loop {
        if editor_active_input.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        match crossterm::event::poll(Duration::from_millis(100)) {
            Ok(true) => match crossterm::event::read() {
                Ok(crossterm::event::Event::Key(key)) => {
                    if input_tx.blocking_send(event::Event::Key(key)).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            },
            Ok(false) => {}
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
    let api_for_editor = api.clone();
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
                open_editor(&mut app, &mut terminal, &cmd_tx, &api_for_editor, &editor_active).await?;
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
    api: &dawnstore_client_lib::Api,
    editor_active: &Arc<AtomicBool>,
) -> Result<()> {
    let Some(obj) = app.selected_object() else {
        return Ok(());
    };
    let original_yaml = serde_yml::to_string(obj).unwrap_or_default();

    // Fetch JSON schema for yaml-language-server support (best effort).
    let schema_tmp = fetch_schema_tempfile(api, &obj.api_version, &obj.kind).await;

    // Prepend a yaml-language-server schema directive if we got a schema file.
    // Use a blank line separator between the directive and the YAML body so that
    // editors that strip the first line don't eat the first YAML field.
    let yaml_with_header = match &schema_tmp {
        Some((_, path)) => format!(
            "# yaml-language-server: $schema={}\n\n{}",
            path.display(),
            original_yaml
        ),
        None => original_yaml.clone(),
    };

    // Write to a named temp file so the editor can open it.
    let tmp = tempfile::Builder::new().suffix(".yaml").tempfile()?;
    std::fs::write(tmp.path(), &yaml_with_header)?;
    let tmp_path = tmp.path().to_owned();

    // Pause the input thread so it doesn't race with the editor for keystrokes.
    editor_active.store(true, Ordering::Relaxed);

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

    // Re-enable input now that we own the terminal again.
    editor_active.store(false, Ordering::Relaxed);

    // schema_tmp kept alive until here so the schema file exists while the editor runs.
    drop(schema_tmp);

    // Strip the schema comment and blank line before comparing / applying.
    if let Ok(new_content) = std::fs::read_to_string(&tmp_path) {
        let new_yaml = strip_schema_comment(&new_content);
        if new_yaml != original_yaml {
            let _ = cmd_tx.send(event::Command::ApplyContent(new_yaml)).await;
        }
    }

    Ok(())
}

/// Download the JSON schema for `kind`, augment it with the ReturnObject
/// metadata fields (id, created_at, …), and write it to a temp file.
/// Returns `(NamedTempFile, path)` so the caller keeps the file alive.
async fn fetch_schema_tempfile(
    api: &dawnstore_client_lib::Api,
    api_version: &str,
    kind: &str,
) -> Option<(tempfile::NamedTempFile, std::path::PathBuf)> {
    let defs = api
        .get_resource_definitions(&dawnstore_client_lib::GetResourceDefinitionFilter::default())
        .await
        .ok()?;
    let def = defs
        .into_iter()
        .find(|d| d.kind == kind && d.api_version == api_version)?;

    // The schema from the server only covers the spec fields.  Inject the
    // standard ReturnObject envelope fields so the language server doesn't
    // flag them as unknown properties — matching what the CLI does.
    let mut schema: serde_json::Value = serde_json::from_str(&def.json_schema).ok()?;
    if let Some(serde_json::Value::Object(props)) = schema.get_mut("properties") {
        for field in ["id", "created_at", "updated_at", "namespace", "api_version", "kind", "name"] {
            props.insert(
                field.to_string(),
                serde_json::json!({"type": "string"}),
            );
        }
    }

    let tmp = tempfile::Builder::new().suffix(".json").tempfile().ok()?;
    let path = tmp.path().to_owned();
    std::fs::write(&path, serde_json::to_string(&schema).ok()?).ok()?;
    Some((tmp, path))
}

/// Remove the `# yaml-language-server: $schema=...` line and the following
/// blank line that we insert between the directive and the YAML body.
fn strip_schema_comment(content: &str) -> String {
    match content.strip_prefix("# yaml-language-server:") {
        Some(rest) => match rest.find('\n') {
            Some(i) => {
                let after_directive = &rest[i + 1..];
                // Strip the one blank separator line we added.
                after_directive
                    .strip_prefix('\n')
                    .unwrap_or(after_directive)
                    .to_string()
            }
            None => String::new(),
        },
        None => content.to_string(),
    }
}
