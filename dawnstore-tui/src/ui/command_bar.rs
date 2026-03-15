use ratatui::{Frame, layout::Rect, widgets::Paragraph};

use crate::app::App;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    // TODO: render as an overlay at the bottom of `area`
    frame.render_widget(
        Paragraph::new(format!(":{}", app.command_input)),
        area,
    );
}
