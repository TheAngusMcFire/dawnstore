use ratatui::{Frame, layout::Rect, widgets::Paragraph};

use crate::app::App;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    // TODO: render centred confirmation popup
    let text = app
        .selected_object()
        .map(|o| format!("Delete {}/{}/{}? (y/n)", o.namespace, o.kind, o.name))
        .unwrap_or_default();
    frame.render_widget(Paragraph::new(text), area);
}
