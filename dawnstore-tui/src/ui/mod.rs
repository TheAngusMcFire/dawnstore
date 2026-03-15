use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
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
/// ```text
/// ┌──────────────────────────────────────┐
/// │ header  namespace / kind / filter    │  1 line
/// ├──────────────────────────────────────┤
/// │                                      │
/// │  main area                           │  fill
/// │                                      │
/// ├──────────────────────────────────────┤
/// │ status / error / command bar         │  1 line
/// └──────────────────────────────────────┘
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
    let kind = app
        .kind_filter
        .as_deref()
        .unwrap_or("all");
    let ns = if app.all_namespaces {
        "<all>".to_string()
    } else {
        app.namespace.clone()
    };
    let filter = if app.filtering {
        format!("   /{}_", app.name_filter)
    } else if !app.name_filter.is_empty() {
        format!("   filter:{}", app.name_filter)
    } else {
        String::new()
    };

    let count = app.visible_objects().len();
    let text = format!(" namespace:{ns}   kind:{kind}{filter}   ({count} objects)");

    frame.render_widget(
        Paragraph::new(text).style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        area,
    );
}

fn render_main(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    match &app.view {
        // Full-screen views — don't render the list behind them.
        View::Detail => detail::render(app, frame, area),
        View::Help => help::render(app, frame, area),
        // List is the base; popups overlay on top.
        _ => {
            resource_list::render(app, frame, area);
            match &app.view {
                View::Confirm => confirm::render(app, frame, area),
                View::NsSwitcher => ns_switcher::render(app, frame, area),
                _ => {}
            }
        }
    }
}

fn render_footer(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let (text, style) = match &app.view {
        View::CommandBar => (
            format!(":{}", app.command_input),
            Style::default().fg(Color::Yellow),
        ),
        _ => {
            if let Some(err) = &app.error {
                (format!(" {err}"), Style::default().fg(Color::Red))
            } else if let Some(status) = &app.status {
                (format!(" {status}"), Style::default().fg(Color::Green))
            } else {
                (
                    " [j/k] navigate  [Enter] detail  [d] delete  [/] filter  [n] ns  [:] cmd  [?] help  [q] quit".to_string(),
                    Style::default().fg(Color::DarkGray),
                )
            }
        }
    };
    frame.render_widget(Paragraph::new(text).style(style), area);
}
