use tokio::sync::mpsc;
use tracing::{error, info};

use dawnstore_lib::{DeleteObject, GetObjectsFilter};

use crate::event::{Command, Event};

/// Runs the API task. Receives [`Command`]s from the main loop, executes them
/// against the dawnstore API, and sends results back as [`Event`]s.
pub async fn run(
    api: dawnstore_client_lib::Api,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
) {
    info!("api task started");
    while let Some(cmd) = commands.recv().await {
        info!(?cmd, "received command");
        match cmd {
            Command::Refresh { namespace, kind } => {
                let filter = GetObjectsFilter { namespace, kind, ..Default::default() };
                match api.get_objects(&filter).await {
                    Ok(objects) => {
                        info!(count = objects.len(), "refreshed objects");
                        events.send(Event::ApiObjects(objects)).await.ok();
                    }
                    Err(e) => {
                        error!(?e, "refresh failed");
                        events.send(Event::ApiError(format!("{e:?}"))).await.ok();
                    }
                }
            }

            Command::RefreshNamespaces => {
                let filter = GetObjectsFilter {
                    namespace: Some("system".to_string()),
                    kind: Some("namespace".to_string()),
                    ..Default::default()
                };
                match api.get_objects(&filter).await {
                    Ok(objects) => {
                        let names = objects.into_iter().map(|o| o.name).collect();
                        events.send(Event::ApiNamespaces(names)).await.ok();
                    }
                    Err(e) => {
                        error!(?e, "namespace refresh failed");
                        events.send(Event::ApiError(format!("{e:?}"))).await.ok();
                    }
                }
            }

            Command::Delete { namespace, kind, name } => {
                let req = DeleteObject { namespace: Some(namespace), kind, name: name.clone() };
                match api.delete_object(&req).await {
                    Ok(()) => {
                        info!(name, "deleted");
                        events.send(Event::ApiSuccess(format!("deleted {name}"))).await.ok();
                    }
                    Err(e) => {
                        error!(?e, "delete failed");
                        events.send(Event::ApiError(format!("{e:?}"))).await.ok();
                    }
                }
            }

            Command::Apply { path } => {
                match std::fs::read_to_string(&path) {
                    Err(e) => {
                        events.send(Event::ApiError(format!("cannot read {path}: {e}"))).await.ok();
                    }
                    Ok(content) => apply_content(&api, content, &events).await,
                }
            }

            Command::ApplyContent(content) => {
                apply_content(&api, content, &events).await;
            }

            // Handled by the main loop — should never reach here.
            Command::OpenEditor | Command::Quit => {}
        }
    }
    info!("api task stopped");
}

async fn apply_content(
    api: &dawnstore_client_lib::Api,
    content: String,
    events: &mpsc::Sender<Event>,
) {
    let value = match serde_yml::from_str::<serde_json::Value>(&content) {
        Ok(v) => v,
        Err(e) => {
            events.send(Event::ApiError(format!("parse error: {e}"))).await.ok();
            return;
        }
    };
    let json = match serde_json::to_string(&value) {
        Ok(j) => j,
        Err(e) => {
            events.send(Event::ApiError(format!("serialise error: {e}"))).await.ok();
            return;
        }
    };
    match api.apply_str(json).await {
        Ok(applied) => {
            let names: Vec<&str> = applied.iter().map(|o| o.name.as_str()).collect();
            let msg = if names.is_empty() {
                "applied (no objects returned)".to_string()
            } else {
                format!("applied {}", names.join(", "))
            };
            info!(msg, "apply succeeded");
            events.send(Event::ApiSuccess(msg)).await.ok();
        }
        Err(e) => {
            error!(?e, "apply failed");
            events.send(Event::ApiError(format!("{e:?}"))).await.ok();
        }
    }
}
