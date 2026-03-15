use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Padding, Paragraph, Wrap},
};

use super::highlight::highlight_yaml;
use crate::app::App;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let Some(rd) = app.resource_definitions.get(app.rd_selected) else {
        return;
    };

    // Convert the JSON schema to YAML for display.
    let schema_yaml = serde_json::from_str::<serde_json::Value>(&rd.json_schema)
        .ok()
        .and_then(|v| serde_yml::to_string(&v).ok())
        .unwrap_or_else(|| rd.json_schema.clone());

    // Prepend a header with kind / api_version / aliases as a YAML comment block.
    let aliases = if rd.aliases.is_empty() {
        "none".to_string()
    } else {
        rd.aliases.join(", ")
    };
    let header = format!(
        "# kind:        {}\n# api_version: {}\n# aliases:     {}\n\n",
        rd.kind, rd.api_version, aliases
    );
    let content = format!("{header}{schema_yaml}");

    let title = format!(
        " {} ({})  [Enter] list resources  [q/Esc] back ",
        rd.kind, rd.api_version
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .padding(Padding::new(1, 1, 1, 0));

    frame.render_widget(
        Paragraph::new(highlight_yaml(&content))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0)),
        area,
    );
}
