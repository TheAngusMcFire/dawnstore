use chrono::Utc;
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Cell, Row, Table, TableState},
};

use crate::app::App;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let visible = app.visible_objects();

    let header = Row::new([
        Cell::from("NAMESPACE").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("NAME").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("KIND").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("AGE").style(Style::default().add_modifier(Modifier::BOLD)),
    ])
    .bottom_margin(0);

    let rows: Vec<Row> = visible
        .iter()
        .map(|obj| {
            Row::new([
                Cell::from(obj.namespace.clone()),
                Cell::from(obj.name.clone()),
                Cell::from(obj.kind.clone()),
                Cell::from(age(&obj.created_at)),
            ])
        })
        .collect();

    let empty_msg = if app.objects.is_empty() {
        " No objects found"
    } else {
        " No objects match filter"
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(20),
            Constraint::Min(20),
            Constraint::Length(18),
            Constraint::Length(6),
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

    if visible.is_empty() {
        use ratatui::widgets::Paragraph;
        frame.render_widget(Paragraph::new(empty_msg), area);
    } else {
        let selected = if visible.is_empty() { None } else { Some(app.selected) };
        let mut state = TableState::default().with_selected(selected);
        frame.render_stateful_widget(table, area, &mut state);
    }
}

fn age(created_at: &chrono::DateTime<Utc>) -> String {
    let secs = (Utc::now() - *created_at).num_seconds().max(0) as u64;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}
