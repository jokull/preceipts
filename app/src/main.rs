//! preceipts.app — macOS desktop cockpit (GPUI). See
//! docs/desktop-app-design.md. One scroll surface, file sidebar, ⌘F find,
//! draft comments with copy-to-agent, PR feedback panel, watch-driven
//! reloads.

mod cockpit;
mod render;
mod segments;
mod theme;

use std::path::PathBuf;

use cockpit::{
    Cockpit, ComposeComment, CopyAllFeedback, Dismiss, FindNext, FindOpen, FindPrev, Quit,
    ToggleFeedback, ToggleScope,
};
use gpui::{
    px, size, App, AppContext as _, Application, Bounds, KeyBinding, Menu, MenuItem,
    WindowBounds, WindowOptions,
};

fn main() {
    let repo = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    Application::new().run(move |cx: &mut App| {
        cx.activate(true);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-shift-d", ToggleScope, None),
            KeyBinding::new("cmd-f", FindOpen, None),
            KeyBinding::new("cmd-g", FindNext, None),
            KeyBinding::new("cmd-shift-g", FindPrev, None),
            KeyBinding::new("escape", Dismiss, None),
            KeyBinding::new("cmd-shift-c", ToggleFeedback, None),
            KeyBinding::new("cmd-shift-m", ComposeComment, None),
        ]);
        cx.set_menus(vec![
            Menu {
                name: "preceipts".into(),
                items: vec![MenuItem::action("Quit preceipts", Quit)],
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Toggle Diff Scope", ToggleScope),
                    MenuItem::action("Find in Diff", FindOpen),
                    MenuItem::action("Toggle Feedback Panel", ToggleFeedback),
                ],
            },
            Menu {
                name: "Review".into(),
                items: vec![
                    MenuItem::action("Comment on Selected Line", ComposeComment),
                    MenuItem::action("Copy All Feedback", CopyAllFeedback),
                ],
            },
        ]);
        let bounds = Bounds::centered(None, size(px(1380.0), px(880.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| Cockpit::new(repo.clone(), cx)),
        )
        .unwrap();
    });
}
