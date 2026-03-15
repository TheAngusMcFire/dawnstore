use crossterm::event::KeyEvent;
use dawnstore_lib::{ResourceDefinition, ReturnObject};

/// All events produced by background tasks and forwarded to the main loop.
#[derive(Debug)]
pub enum Event {
    /// A key was pressed on the keyboard.
    Key(KeyEvent),
    /// Periodic tick from the timer task — triggers a background refresh.
    Tick,
    /// The API returned a fresh list of objects.
    ApiObjects(Vec<ReturnObject<serde_json::Value>>),
    /// The API returned a list of namespace names.
    ApiNamespaces(Vec<String>),
    /// The API returned all resource definitions.
    ApiResourceDefinitions(Vec<ResourceDefinition>),
    /// An API call succeeded; the string is a human-readable summary.
    ApiSuccess(String),
    /// An API call failed.
    ApiError(String),
}

/// Commands sent from the main loop to the API task, or handled by the
/// main loop itself (`Quit`, `OpenEditor`).
#[derive(Debug)]
pub enum Command {
    // ── Forwarded to api_task ─────────────────────────────────────────────
    /// Fetch objects matching the current namespace / kind filter.
    Refresh {
        namespace: Option<String>,
        kind: Option<String>,
    },
    /// Fetch all namespaces (for the namespace switcher).
    RefreshNamespaces,
    /// Fetch all resource definitions.
    RefreshResourceDefinitions,
    /// Delete the identified object.
    Delete {
        namespace: String,
        kind: String,
        name: String,
    },
    /// Apply the YAML/JSON file at `path`.
    Apply { path: String },
    /// Apply already-read YAML/JSON content (used after editor).
    ApplyContent(String),

    // ── Handled by the main loop ──────────────────────────────────────────
    /// Open the selected object in $EDITOR and apply changes on save.
    OpenEditor,
    /// Exit the application.
    Quit,
}
