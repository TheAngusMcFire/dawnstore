use std::sync::OnceLock;

use ratatui::{
    style::{Color, Style},
    text::{Line, Span, Text},
};
use syntect::{easy::HighlightLines, highlighting::Theme, parsing::SyntaxSet, util::LinesWithEndings};
use syntect_assets::assets::HighlightingAssets;

struct Highlighter {
    ss: SyntaxSet,
    theme: Theme,
}

static HIGHLIGHTER: OnceLock<Highlighter> = OnceLock::new();

fn highlighter() -> &'static Highlighter {
    HIGHLIGHTER.get_or_init(|| {
        let assets = HighlightingAssets::from_binary();
        let ss = assets.get_syntax_set().expect("syntax set").clone();
        let theme = assets.get_theme("Monokai Extended").clone();
        Highlighter { ss, theme }
    })
}

pub fn highlight_yaml(yaml: &str) -> Text<'static> {
    let hl = highlighter();
    let syntax = hl
        .ss
        .find_syntax_by_extension("yaml")
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
