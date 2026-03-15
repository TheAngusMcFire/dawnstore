use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Cell, Row, Table, TableState},
};

use crate::app::App;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let header = Row::new([
        Cell::from("KIND").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("API VERSION").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("ALIASES").style(Style::default().add_modifier(Modifier::BOLD)),
    ]);

    let rows: Vec<Row> = app
        .resource_definitions
        .iter()
        .map(|rd| {
            Row::new([
                Cell::from(rd.kind.clone()).style(Style::new().fg(Color::Cyan)),
                Cell::from(rd.api_version.clone()).style(Style::new().fg(Color::Yellow)),
                Cell::from(rd.aliases.join(", ")),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(24),
            Constraint::Length(16),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .row_highlight_style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
    .block(Block::default());

    if app.resource_definitions.is_empty() {
        use ratatui::widgets::Paragraph;
        frame.render_widget(Paragraph::new(" Loading resource definitions…"), area);
    } else {
        let mut state = TableState::default().with_selected(Some(app.rd_selected));
        frame.render_stateful_widget(table, area, &mut state);
    }
}
