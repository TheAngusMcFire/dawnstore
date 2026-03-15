use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::Paragraph,
};

use crate::app::{App, View};

mod command_bar;
mod confirm;
mod detail;
mod help;
mod ns_switcher;
mod resource_list;

/// Render the full UI for the current app state.
///
/// Layout (3 zones, shared across all views):
/// ```text
/// ┌──────────────────────────────────┐
/// │ header  namespace / kind         │  1 line
/// ├──────────────────────────────────┤
/// │                                  │
/// │  main area                       │  fill
/// │                                  │
/// ├──────────────────────────────────┤
/// │ status / error bar               │  1 line
/// └──────────────────────────────────┘
/// ```
pub fn render(app: &App, frame: &mut Frame) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(0),    // main area
            Constraint::Length(1), // footer
        ])
        .split(frame.area());

    render_header(app, frame, areas[0]);
    render_main(app, frame, areas[1]);
    render_footer(app, frame, areas[2]);
}

fn render_header(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let kind = app.kind_filter.as_deref().unwrap_or("all");
    let ns = if app.all_namespaces {
        "<all>".to_string()
    } else {
        app.namespace.clone()
    };
    let filter = if app.name_filter.is_empty() {
        String::new()
    } else {
        format!("  filter: {}", app.name_filter)
    };
    frame.render_widget(
        Paragraph::new(format!(" namespace: {ns}  kind: {kind}{filter}"))
            .style(Style::default().bg(Color::DarkGray)),
        area,
    );
}

fn render_main(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    match app.view {
        // Popups overlay the resource list.
        View::ResourceList | View::Confirm | View::NsSwitcher | View::CommandBar => {
            resource_list::render(app, frame, area);
            match app.view {
                View::Confirm => confirm::render(app, frame, area),
                View::NsSwitcher => ns_switcher::render(app, frame, area),
                View::CommandBar => command_bar::render(app, frame, area),
                _ => {}
            }
        }
        View::Detail => detail::render(app, frame, area),
        View::Help => help::render(app, frame, area),
    }
}

fn render_footer(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let (text, style) = if let Some(err) = &app.error {
        (format!(" {err}"), Style::default().fg(Color::Red))
    } else if let Some(status) = &app.status {
        (format!(" {status}"), Style::default().fg(Color::Green))
    } else {
        (" Press ? for help".to_string(), Style::default().fg(Color::DarkGray))
    };
    frame.render_widget(Paragraph::new(text).style(style), area);
}
