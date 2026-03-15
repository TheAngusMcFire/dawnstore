use ratatui::{Frame, layout::Rect, widgets::Paragraph};

use crate::app::App;

pub fn render(_app: &App, frame: &mut Frame, area: Rect) {
    // TODO: render centred scrollable namespace list popup
    frame.render_widget(Paragraph::new("namespace switcher (not yet implemented)"), area);
}
