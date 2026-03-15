use ratatui::{Frame, layout::Rect, widgets::Paragraph};

use crate::app::App;

pub fn render(_app: &App, frame: &mut Frame, area: Rect) {
    // TODO: render static keybinding table
    let text = "\
Global
  ?          toggle this help
  q / :q     quit

Resource list
  j / ↓      move down
  k / ↑      move up
  Enter      open detail view
  d          delete selected (confirm required)
  /          filter by name
  a          toggle all-namespaces
  n          open namespace switcher
  :          open command bar

Detail view
  j / ↓      scroll down
  k / ↑      scroll up
  e          open in $EDITOR
  d          delete (confirm required)
  Esc / q    back to list

Command bar
  :q         quit
  :ns <name> switch namespace
  :apply <p> apply file
  :<kind>    filter by kind
  Esc        cancel
";
    frame.render_widget(Paragraph::new(text), area);
}
