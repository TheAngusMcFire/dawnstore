use tokio::sync::mpsc;
use tracing::{error, info};

use crate::event::{Command, Event};

/// Runs the API task. Receives [`Command`]s from the main loop, executes them
/// against the dawnstore API, and sends results back as [`Event`]s.
///
/// This is the only place that calls `dawnstore_client_lib::Api` — keeping all
/// network I/O out of the main loop and the state layer.
pub async fn run(
    api: dawnstore_client_lib::Api,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
) {
    info!("api task started");
    while let Some(cmd) = commands.recv().await {
        info!(?cmd, "received command");
        match cmd {
            Command::Refresh { .. } => {
                // TODO: call api.get_objects and send Event::ApiObjects
                let _ = &api;
                let _ = events.send(Event::ApiObjects(vec![])).await;
            }
            Command::RefreshNamespaces => {
                // TODO: call api.get_objects filtered to kind "namespace"
                let _ = events.send(Event::ApiNamespaces(vec![])).await;
            }
            Command::Delete { namespace, kind, name } => {
                // TODO: call api.delete_object
                info!(namespace, kind, name, "delete requested");
            }
            Command::Apply { path } => {
                // TODO: read file and call api.apply_str
                info!(path, "apply requested");
                let _ = events
                    .send(Event::ApiError("apply not yet implemented".into()))
                    .await
                    .inspect_err(|e| error!(?e, "failed to send event"));
            }
        }
    }
    info!("api task stopped");
}
