use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Padding, Paragraph, Wrap},
};

use crate::app::App;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let yaml = match app.selected_object() {
        Some(obj) => serde_yml::to_string(obj)
            .unwrap_or_else(|e| format!("serialisation error: {e}")),
        None => "no object selected".to_string(),
    };

    let block = Block::default()
        .title(" Detail  [e] edit  [D] delete  [r] refresh  [q/Esc] back ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .padding(Padding::new(1, 1, 1, 0));

    frame.render_widget(
        Paragraph::new(yaml)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0)),
        area,
    );
}
