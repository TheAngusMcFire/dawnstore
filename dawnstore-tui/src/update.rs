use crate::{app::App, event::{Command, Event}};

/// Apply `event` to `app` and optionally return a `Command` to send to the
/// API task. Pure function — no I/O, fully unit-testable.
pub fn update(_app: &mut App, _event: Event) -> Option<Command> {
    // TODO: implement keybinding logic per view
    None
}
