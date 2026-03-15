use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, List, ListItem, ListState},
};

use crate::app::App;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let popup = centered_rect(50, 50, area);

    let items: Vec<ListItem> = app
        .namespaces
        .iter()
        .map(|ns| ListItem::new(format!(" {ns}")))
        .collect();

    let list = if items.is_empty() {
        List::new(vec![ListItem::new(" (loading...)")])
    } else {
        List::new(items)
    }
    .block(
        Block::default()
            .title(" Switch Namespace  [Enter] select  [Esc] cancel ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    )
    .highlight_style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default().with_selected(Some(app.ns_selected));
    frame.render_widget(Clear, popup);
    frame.render_stateful_widget(list, popup, &mut state);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(r);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1])[1]
}
