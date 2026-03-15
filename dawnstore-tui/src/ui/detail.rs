use ratatui::{Frame, layout::Rect, widgets::Paragraph};

use crate::app::App;

pub fn render(_app: &App, frame: &mut Frame, area: Rect) {
    // TODO: render selected object as scrollable YAML
    frame.render_widget(Paragraph::new("detail view (not yet implemented)"), area);
}
