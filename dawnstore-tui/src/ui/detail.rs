use std::sync::OnceLock;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Padding, Paragraph, Wrap},
};
use syntect::{
    easy::HighlightLines,
    highlighting::{Theme, ThemeSet},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

use crate::app::App;

struct Highlighter {
    ss: SyntaxSet,
    theme: Theme,
}

static HIGHLIGHTER: OnceLock<Highlighter> = OnceLock::new();

fn highlighter() -> &'static Highlighter {
    HIGHLIGHTER.get_or_init(|| {
        let ss = SyntaxSet::load_defaults_newlines();
        let mut ts = ThemeSet::load_defaults();
        // "base16-ocean.dark" ships with syntect's default themes and looks
        // good on a dark terminal background.
        let theme = ts.themes.remove("base16-ocean.dark")
            .unwrap_or_else(|| ts.themes.into_values().next().expect("no themes"));
        Highlighter { ss, theme }
    })
}

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let yaml = match app.selected_object() {
        Some(obj) => serde_yml::to_string(obj)
            .unwrap_or_else(|e| format!("serialisation error: {e}")),
        None => "no object selected".to_string(),
    };

    let content = highlight_yaml(&yaml);

    let block = Block::default()
        .title(" Detail  [e] edit  [D] delete  [r] refresh  [q/Esc] back ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .padding(Padding::new(1, 1, 1, 0));

    frame.render_widget(
        Paragraph::new(content)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0)),
        area,
    );
}

fn highlight_yaml(yaml: &str) -> Text<'static> {
    let hl = highlighter();
    let syntax = hl.ss.find_syntax_by_extension("yaml")
        .unwrap_or_else(|| hl.ss.find_syntax_plain_text());
    let mut h = HighlightLines::new(syntax, &hl.theme);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for line in LinesWithEndings::from(yaml) {
        let Ok(ranges) = h.highlight_line(line, &hl.ss) else {
            lines.push(Line::raw(line.trim_end_matches('\n').to_owned()));
            continue;
        };
        let spans: Vec<Span<'static>> = ranges
            .into_iter()
            .filter_map(|(style, text)| {
                let text = text.trim_end_matches('\n').to_owned();
                if text.is_empty() {
                    return None;
                }
                let fg = style.foreground;
                Some(Span::styled(
                    text,
                    Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b)),
                ))
            })
            .collect();
        lines.push(Line::from(spans));
    }
    Text::from(lines)
}
