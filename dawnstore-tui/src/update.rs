use crossterm::event::{KeyCode, KeyModifiers};
use tracing::debug;

use crate::{
    app::{App, View},
    event::{Command, Event},
};

/// Apply `event` to `app` and return an optional `Command` to forward.
/// Pure — no I/O. All keybinding logic lives here.
pub fn update(app: &mut App, event: Event) -> Option<Command> {
    match event {
        Event::Tick => handle_tick(app),
        Event::Key(key) => handle_key(app, key),
        Event::ApiObjects(objects) => {
            debug!(count = objects.len(), "received objects");
            app.objects = objects;
            app.clamp_selection();
            None
        }
        Event::ApiNamespaces(names) => {
            debug!(count = names.len(), "received namespaces");
            app.namespaces = names;
            app.ns_selected = app.ns_selected.min(app.namespaces.len().saturating_sub(1));
            None
        }
        Event::ApiSuccess(msg) => {
            debug!(msg, "api success");
            app.status = Some(msg);
            app.error = None;
            app.status_ticks = 5;
            // Auto-refresh so the list reflects the change immediately.
            Some(Command::Refresh {
                namespace: if app.all_namespaces { None } else { Some(app.namespace.clone()) },
                kind: app.kind_filter.clone(),
            })
        }
        Event::ApiError(err) => {
            debug!(err, "api error");
            app.error = Some(err);
            app.status = None;
            app.status_ticks = 5;
            None
        }
    }
}

// ── Tick ─────────────────────────────────────────────────────────────────────

fn handle_tick(app: &mut App) -> Option<Command> {
    if app.status_ticks > 0 {
        app.status_ticks -= 1;
        if app.status_ticks == 0 {
            app.status = None;
            app.error = None;
        }
    }
    Some(Command::Refresh {
        namespace: if app.all_namespaces { None } else { Some(app.namespace.clone()) },
        kind: app.kind_filter.clone(),
    })
}

// ── Key dispatch ─────────────────────────────────────────────────────────────

fn handle_key(app: &mut App, key: crossterm::event::KeyEvent) -> Option<Command> {
    // Ctrl-C always quits regardless of view.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Some(Command::Quit);
    }
    match app.view.clone() {
        View::ResourceList => handle_resource_list(app, key),
        View::Detail => handle_detail(app, key),
        View::CommandBar => handle_command_bar(app, key),
        View::Confirm => handle_confirm(app, key),
        View::NsSwitcher => handle_ns_switcher(app, key),
        View::Help => handle_help(app, key),
    }
}

// ── Resource list ─────────────────────────────────────────────────────────────

fn handle_resource_list(app: &mut App, key: crossterm::event::KeyEvent) -> Option<Command> {
    // While typing the name filter, most keys feed into the filter string.
    if app.filtering {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                app.filtering = false;
            }
            KeyCode::Backspace => {
                app.name_filter.pop();
            }
            KeyCode::Char(c) => {
                app.name_filter.push(c);
            }
            _ => {}
        }
        app.clamp_selection();
        return None;
    }

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            let max = app.visible_objects().len().saturating_sub(1);
            if app.selected < max {
                app.selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.selected > 0 {
                app.selected -= 1;
            }
        }
        KeyCode::Enter => {
            if app.selected_object().is_some() {
                app.detail_scroll = 0;
                app.view = View::Detail;
            }
        }
        KeyCode::Char('e') => {
            if app.selected_object().is_some() {
                app.detail_scroll = 0;
                app.view = View::Detail;
                return Some(Command::OpenEditor);
            }
        }
        KeyCode::Char('D') => {
            if app.selected_object().is_some() {
                app.confirm_return_view = View::ResourceList;
                app.view = View::Confirm;
            }
        }
        KeyCode::Char('r') => {
            return Some(Command::Refresh {
                namespace: if app.all_namespaces { None } else { Some(app.namespace.clone()) },
                kind: app.kind_filter.clone(),
            });
        }
        KeyCode::Char('/') => {
            app.filtering = true;
            app.name_filter.clear();
        }
        KeyCode::Esc => {
            // Clear filter on Esc when not in filter-typing mode.
            if !app.name_filter.is_empty() {
                app.name_filter.clear();
                app.clamp_selection();
            }
        }
        KeyCode::Char('a') => {
            app.all_namespaces = !app.all_namespaces;
            app.selected = 0;
            return Some(Command::Refresh {
                namespace: if app.all_namespaces { None } else { Some(app.namespace.clone()) },
                kind: app.kind_filter.clone(),
            });
        }
        KeyCode::Char('n') => {
            app.ns_selected = 0;
            app.view = View::NsSwitcher;
            return Some(Command::RefreshNamespaces);
        }
        KeyCode::Char(':') => {
            app.command_input.clear();
            app.view = View::CommandBar;
        }
        KeyCode::Char('?') => {
            app.view = View::Help;
        }
        KeyCode::Char('q') => {
            return Some(Command::Quit);
        }
        _ => {}
    }
    None
}

// ── Detail view ───────────────────────────────────────────────────────────────

fn handle_detail(app: &mut App, key: crossterm::event::KeyEvent) -> Option<Command> {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            app.detail_scroll = app.detail_scroll.saturating_add(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.detail_scroll = app.detail_scroll.saturating_sub(1);
        }
        KeyCode::Char('e') => {
            if app.selected_object().is_some() {
                return Some(Command::OpenEditor);
            }
        }
        KeyCode::Char('D') => {
            if app.selected_object().is_some() {
                app.confirm_return_view = View::Detail;
                app.view = View::Confirm;
            }
        }
        KeyCode::Char('r') => {
            return Some(Command::Refresh {
                namespace: if app.all_namespaces { None } else { Some(app.namespace.clone()) },
                kind: app.kind_filter.clone(),
            });
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            app.view = View::ResourceList;
        }
        KeyCode::Char('?') => {
            app.view = View::Help;
        }
        _ => {}
    }
    None
}

// ── Command bar ───────────────────────────────────────────────────────────────

fn handle_command_bar(app: &mut App, key: crossterm::event::KeyEvent) -> Option<Command> {
    match key.code {
        KeyCode::Esc => {
            app.view = View::ResourceList;
            app.command_input.clear();
        }
        KeyCode::Backspace => {
            app.command_input.pop();
        }
        KeyCode::Enter => {
            let input = app.command_input.trim().to_string();
            app.command_input.clear();
            app.view = View::ResourceList;
            return execute_command(app, &input);
        }
        KeyCode::Char(c) => {
            app.command_input.push(c);
        }
        _ => {}
    }
    None
}

fn execute_command(app: &mut App, input: &str) -> Option<Command> {
    if input == "q" {
        return Some(Command::Quit);
    }
    if let Some(ns) = input.strip_prefix("ns ") {
        app.namespace = ns.trim().to_string();
        app.all_namespaces = false;
        app.selected = 0;
        return Some(Command::Refresh {
            namespace: Some(app.namespace.clone()),
            kind: app.kind_filter.clone(),
        });
    }
    if let Some(path) = input.strip_prefix("apply ") {
        return Some(Command::Apply { path: path.trim().to_string() });
    }
    if input == "all" {
        app.kind_filter = None;
        app.selected = 0;
        return Some(Command::Refresh {
            namespace: if app.all_namespaces { None } else { Some(app.namespace.clone()) },
            kind: None,
        });
    }
    // Anything else is treated as a kind filter.
    if !input.is_empty() {
        app.kind_filter = Some(input.to_string());
        app.selected = 0;
        return Some(Command::Refresh {
            namespace: if app.all_namespaces { None } else { Some(app.namespace.clone()) },
            kind: Some(input.to_string()),
        });
    }
    None
}

// ── Confirm popup ─────────────────────────────────────────────────────────────

fn handle_confirm(app: &mut App, key: crossterm::event::KeyEvent) -> Option<Command> {
    match key.code {
        KeyCode::Char('y') => {
            if let Some(obj) = app.selected_object() {
                let cmd = Command::Delete {
                    namespace: obj.namespace.clone(),
                    kind: obj.kind.clone(),
                    name: obj.name.clone(),
                };
                app.view = View::ResourceList;
                return Some(cmd);
            }
            app.view = View::ResourceList;
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.view = app.confirm_return_view.clone();
        }
        _ => {}
    }
    None
}

// ── Namespace switcher ────────────────────────────────────────────────────────

fn handle_ns_switcher(app: &mut App, key: crossterm::event::KeyEvent) -> Option<Command> {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            let max = app.namespaces.len().saturating_sub(1);
            if app.ns_selected < max {
                app.ns_selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.ns_selected > 0 {
                app.ns_selected -= 1;
            }
        }
        KeyCode::Enter => {
            if let Some(ns) = app.namespaces.get(app.ns_selected).cloned() {
                app.namespace = ns;
                app.all_namespaces = false;
                app.selected = 0;
                app.view = View::ResourceList;
                return Some(Command::Refresh {
                    namespace: Some(app.namespace.clone()),
                    kind: app.kind_filter.clone(),
                });
            }
            app.view = View::ResourceList;
        }
        KeyCode::Esc => {
            app.view = View::ResourceList;
        }
        _ => {}
    }
    None
}

// ── Help overlay ──────────────────────────────────────────────────────────────

fn handle_help(app: &mut App, key: crossterm::event::KeyEvent) -> Option<Command> {
    match key.code {
        KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
            app.view = View::ResourceList;
        }
        _ => {}
    }
    None
}
