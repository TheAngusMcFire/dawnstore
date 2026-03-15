use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::app::App;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let popup = centered_rect(60, 7, area);

    let text = match app.selected_object() {
        Some(obj) => format!(
            "\n  Delete  {}/{}/{}?\n\n  [y] confirm     [n / Esc] cancel",
            obj.namespace, obj.kind, obj.name
        ),
        None => "  Nothing selected".to_string(),
    };

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .title(" Confirm Delete ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Red)),
            )
            .style(Style::default().fg(Color::White)),
        popup,
    );
}

/// Returns a `Rect` of fixed height `rows` centred horizontally with
/// `percent_x` width, positioned vertically centred in `r`.
fn centered_rect(percent_x: u16, rows: u16, r: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(rows),
        Constraint::Fill(1),
    ])
    .split(r);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1])[1]
}
