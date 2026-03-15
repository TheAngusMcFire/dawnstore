use ratatui::{Frame, layout::Rect, widgets::Paragraph};

use crate::app::App;

pub fn render(_app: &App, frame: &mut Frame, area: Rect) {
    // TODO: render scrollable object table
    frame.render_widget(Paragraph::new("resource list (not yet implemented)"), area);
}
