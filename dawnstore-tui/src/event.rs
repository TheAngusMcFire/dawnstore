use crossterm::event::KeyEvent;
use dawnstore_lib::ReturnObject;

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
    /// An API call failed.
    ApiError(String),
}

/// Commands sent from the main loop to the API task.
#[derive(Debug)]
pub enum Command {
    /// Fetch objects matching the current namespace / kind filter.
    Refresh {
        namespace: Option<String>,
        kind: Option<String>,
    },
    /// Fetch all namespaces (for the namespace switcher).
    RefreshNamespaces,
    /// Delete the identified object.
    Delete {
        namespace: String,
        kind: String,
        name: String,
    },
    /// Apply the YAML/JSON file at `path`.
    Apply { path: String },
}
