# TUI Application Plan (k9s-style)

## TUI Framework

**Ratatui** is the clear choice. It's the maintained fork of tui-rs, has a large ecosystem, and is what most serious Rust TUIs (including k9s-like tools) use. Pair it with **crossterm** as the backend (cross-platform terminal handling).

## Architecture

The k9s model maps naturally to dawnstore:

- **Resource list view** — table of objects (namespace, name, kind, created_at), navigable with arrow keys. Mirrors `get all`.
- **Detail/YAML view** — show the selected object's full spec, similar to `edit` but read-only.
- **Command bar** — vim-style `:` prompt for switching resource kind, namespace, applying files.
- **Namespace switcher** — dropdown/popup fed by `get namespace`.
- **Log/event pane** — apply/delete results shown at the bottom.

The core loop: fetch objects from the API on a timer (e.g. every 2s), diff against current state, re-render only changed rows.

## Problems

**1. Async + Ratatui don't compose easily.**
Ratatui's render loop is synchronous. You'd run it in a dedicated thread/task and communicate via `tokio::sync::mpsc` channels — one channel for events (keypress, tick, API response), one for sending commands back to the API task. This is doable but requires careful design upfront.

**2. Terminal input conflicts with async.**
Crossterm's event polling blocks. You'd need a dedicated thread reading input events and forwarding them to the main channel, separate from your API polling task.

**3. Editing objects.**
k9s spawns `$EDITOR` for edits. The TUI must suspend itself (restore the terminal), wait for the editor, then resume. Ratatui has helpers for this but it's fiddly — you need to properly call `disable_raw_mode` / `restore terminal` and handle the case where the editor crashes or the user aborts.

**4. Schema-driven rendering.**
Dawnstore objects are generic (`serde_json::Value` specs). You can't hardcode column layouts. Either you show a fixed set of metadata columns (namespace, name, kind) and dump the spec as JSON in the detail pane, or you make columns configurable per kind — which is significantly more work.

**5. RBAC-aware display.**
If the token lacks permission to list a kind, the API returns a `Forbidden` error. The TUI needs to degrade gracefully rather than crash or show a blank screen.

**6. Large result sets.**
If there are thousands of objects, you need server-side pagination (the `page`/`page_size` fields on `GetObjectsFilter` already exist). The TUI needs virtual scrolling — only rendering visible rows.

## Views

### 1. Resource List (main view)
The default view on startup, shows all objects in the current namespace.

- Scrollable table with columns: namespace, name, kind, age
- Arrow keys / `j`/`k` to navigate rows
- `:` to open the command bar
- `<Enter>` to open the detail view for the selected object
- `d` to delete the selected object (with confirmation prompt)
- `/` to filter the list by name (live, client-side)
- `a` to toggle all-namespaces mode
- Auto-refresh every 2s in the background
- Status bar showing current namespace, kind filter, and last refresh time

### 2. Detail View
Opens when pressing `<Enter>` on a resource in the list.

- Full object rendered as YAML (including spec)
- Scroll up/down through the YAML
- `e` to open the object in `$EDITOR` (suspends TUI, same behaviour as CLI `edit`)
- `d` to delete the object (with confirmation prompt)
- `<Esc>` / `q` to return to the resource list

### 3. Command Bar
Activated with `:`, dismissed with `<Esc>`.

- Switch kind: `:pod`, `:namespace`, `:all`, etc. — accepts canonical names and aliases
- Switch namespace: `:ns <name>`
- Apply a file: `:apply <path>`
- Quit: `:q`

### 4. Confirmation Prompt
Inline popup before any destructive action.

- Shows the resource being deleted (`namespace/kind/name`)
- `y` to confirm, `n` / `<Esc>` to cancel

### 5. Namespace Switcher (popup)
Activated with `n` from the resource list.

- Scrollable list of all namespaces fetched from the API
- `<Enter>` to switch, `<Esc>` to cancel

### 6. Event / Error Log (bottom bar)
Persistent one-line status area at the bottom of every view.

- Shows result of last action (e.g. `applied deployment/api-v1`, `deleted secret/db-pass`)
- Shows API errors in red (including `Forbidden` so the user knows why a list is empty)
- Clears after 5s

## Logging

Because the TUI takes over stdout/stderr, all diagnostic output must go to a file. This is also essential for debugging since the rendered UI is not observable from the outside.

- Use `tracing` + `tracing-appender` with a non-blocking file writer
- Log path: `~/.local/share/dawnstore-tui/app.log` (follows XDG convention)
- Log every state transition, every API call (request + response summary), every key event handled, and every error
- Log level controlled via `RUST_LOG` env var (default: `info`)
- On panic, the panic hook should flush the log before exiting so the last event is always captured

```toml
tracing = "0.1"
tracing-subscriber = { features = ["env-filter"] }
tracing-appender = "0.2"
```

## File Structure

```
dawnstore-tui/
├── Cargo.toml
└── src/
    ├── main.rs           # Entry point: init logging, parse args/context, start event loop
    ├── app.rs            # App struct — owns all state (current view, selected row, namespace,
    │                     # kind filter, last error). Pure data, no I/O. Unit-testable.
    ├── event.rs          # Event enum (Key, Tick, ApiResponse, Error) and the input thread
    │                     # that reads crossterm events and forwards them via mpsc channel
    ├── api_task.rs       # Async task that owns the dawnstore_client_lib::Api, receives
    │                     # commands (Refresh, Delete, Apply), sends ApiResponse events back
    ├── ui/
    │   ├── mod.rs        # Top-level render fn: routes to the right view based on app state
    │   ├── resource_list.rs  # Renders the main scrollable object table
    │   ├── detail.rs     # Renders the YAML detail pane
    │   ├── command_bar.rs    # Renders and handles the `:` command input
    │   ├── confirm.rs    # Renders the delete confirmation popup
    │   └── ns_switcher.rs   # Renders the namespace switcher popup
    └── update.rs         # Pure state-transition fn: (App, Event) -> App (or mutates in place)
                          # Contains all keybinding logic. Easy to unit test without a terminal.
```

### Key design principle

`update.rs` and `app.rs` contain no I/O — they only transform state. `api_task.rs` and `event.rs` contain all I/O. `ui/` only reads state and renders. This separation means the logic layer is fully unit-testable and the file logger captures everything that crosses the I/O boundary.

## Rough crate list

```toml
ratatui = "0.29"
crossterm = "0.28"
tokio = { features = ["full"] }
tracing = "0.1"
tracing-subscriber = { features = ["env-filter"] }
tracing-appender = "0.2"
dawnstore-client-lib = { path = "../dawnstore-client-lib" }
```

## Design Decisions

### 1. Screen Layout

Every view shares the same three-zone layout:

```
┌─────────────────────────────────────────────┐
│ header  │ namespace: default  kind: all      │  1 line
├─────────────────────────────────────────────┤
│                                             │
│  main area  (resource list / detail / popup)│  terminal height - 3
│                                             │
├─────────────────────────────────────────────┤
│ status bar  │ last action or error          │  1 line
│ command bar │ only visible when active      │  1 line (conditional)
└─────────────────────────────────────────────┘
```

- The detail view replaces the full main area (not a split) to keep the layout simple for the MVP.
- Popups (confirmation, namespace switcher) render as a centred overlay on top of the main area.
- The command bar occupies the bottom line only when active; otherwise the status bar spans both lines.

### 2. Event Loop Concurrency Model

Three concurrent actors communicating over a single `tokio::sync::mpsc` channel of an `Event` enum:

```
┌─────────────┐    Event::Key      ┌──────────────┐
│ input task  │ ────────────────►  │              │
│ (blocking   │                    │  main loop   │  ── renders on every event
│  thread)    │                    │  (main task) │  ── calls update(app, event)
└─────────────┘                    │              │  ── sends Command to api task
                                   └──────┬───────┘
┌─────────────┐    Event::Tick              │ Command
│ timer task  │ ────────────────►           ▼
│ (interval   │               ┌─────────────────────┐
│  2s)        │               │      api task        │
└─────────────┘               │  owns Api client     │
                              │  sends Event::Api*   │
┌─────────────┐               │  back to main loop   │
│  api task   │ ──────────────►                      │
│             │  Event::ApiResult / Event::ApiError  │
└─────────────┘               └─────────────────────┘
```

- **One channel** (`mpsc::Sender<Event>` cloned for each producer). Simpler than multiple channels and sufficient for this workload.
- **Input task** runs on a dedicated `std::thread` (not a tokio task) because `crossterm::event::read()` blocks and must not starve the async runtime.
- **Timer task** is a `tokio::spawn` running `tokio::time::interval(Duration::from_secs(2))` that sends `Event::Tick`; the main loop handles the tick by sending a `Command::Refresh` to the api task.
- **Api task** is a `tokio::spawn` with its own `mpsc::Receiver<Command>`; it owns the `Api` client and sends results back as events.
- **Main loop** runs on the main thread inside `#[tokio::main]`, receives events, calls `update(&mut app, event)` to mutate state, then calls `ui::render(&app, frame)`.

### 3. Startup and Context Loading

- Reuse the existing `config::Context` type from `dawnstore-cli` by moving it to `dawnstore-client-lib` so both the CLI and TUI can share it without a circular dependency.
- The TUI accepts the same `--context-path` / `DAWNSTORE_CONTEXT` env var as the CLI.
- On startup, if the context file is missing or malformed, print the error to stderr (terminal not yet taken over) and exit with a non-zero code — same behaviour as the CLI.

### 4. Graceful Shutdown

Two shutdown paths must both restore the terminal:

- **Normal quit** (`q` / `:q`): main loop breaks, runs cleanup, exits.
- **Panic**: install a panic hook before entering raw mode that calls `crossterm::terminal::disable_raw_mode()` and `crossterm::execute!(stdout, LeaveAlternateScreen)` before printing the panic message. Without this the terminal is left broken after a crash.

Use an RAII guard struct (`TerminalGuard`) that implements `Drop` to ensure cleanup runs in both paths:

```rust
struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
}
```

The panic hook flushes the tracing log before printing so the last recorded event is always on disk.

### 5. Keybinding Help View

A `?` key opens a full-screen overlay listing all keybindings grouped by context (global, list view, detail view). It is rendered as a static table — no logic required. Dismissed with `?` or `<Esc>`. Counts as a single `ui/help.rs` file and one match arm in `update.rs`.

### 6. Crate Placement in the Workspace

- Add `dawnstore-tui` as a new workspace member alongside `dawnstore-cli`.
- Move `config.rs` (context file parsing) from `dawnstore-cli/src/` into `dawnstore-client-lib` so it can be shared.
- `dawnstore-tui` depends on `dawnstore-client-lib` only — no dependency on `dawnstore-cli`.

## Estimate

Feasible in maybe 2–3 weeks for a usable MVP (list + detail + delete + apply-from-file). The async/render split is the hardest architectural decision to get right early — if you get that channel design wrong it becomes very painful to refactor. The schema-generic rendering is the second biggest risk for polish.
