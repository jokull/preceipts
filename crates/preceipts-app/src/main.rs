//! The cockpit.
//!
//! One window per project; a tab per workspace. Step 3 of
//! docs/direction-2026-08.md — the diff surface — is what fills a tab, and the
//! tabs themselves are step 5's workspace list, moved to the top edge.

mod cockpit;
mod surface_view;
mod theme;
mod workspace_pane;

use cockpit::Cockpit;
use gpui::{
    point, px, size, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
};
use gpui_component::{Theme as UiTheme, ThemeMode};
use preceipts_core::workspace;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `--stats` loads and reports without opening a window. It is how the
    // scroll path gets timed honestly, and it is the seed of the CLI that
    // step 7 grows into an agent-facing surface.
    let stats_only = args.iter().any(|a| a == "--stats");
    let path = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("a working directory"));

    // Which workspace this window opens on is decided by where it was
    // launched from — self-location, not configuration. The window is the
    // project's; the first tab is the worktree you were standing in.
    let first = match workspace::locate(&path) {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!("preceipts: {error}");
            std::process::exit(1);
        }
    };
    let project_root = first.project_root.clone();
    let project_name = project_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());

    if stats_only {
        let started = std::time::Instant::now();
        let changeset =
            match preceipts_core::load(&first.path, preceipts_core::DiffScope::Branch, true) {
                Ok(changeset) => changeset,
                Err(error) => {
                    eprintln!("preceipts: {error}");
                    std::process::exit(1);
                }
            };
        let rows = preceipts_core::Surface::build(&changeset).rows.len();
        println!(
            "{} — {}\n{rows} rows built in {:.0}ms",
            changeset
                .branch
                .clone()
                .unwrap_or_else(|| "detached".to_string()),
            surface_view::summary(&changeset),
            started.elapsed().as_secs_f64() * 1000.0
        );
        return;
    }

    Application::new().run(move |cx| {
        // gpui-component keeps its widget state in globals — theme registry,
        // popover and menu layers, input bindings. Nothing from the library
        // renders before this runs.
        gpui_component::init(cx);
        // The diff surface is a fixed dark palette, so the shell is too rather
        // than following the system and clashing with it half the time.
        UiTheme::change(ThemeMode::Dark, None, cx);
        paint_shell_to_match_the_surface(cx);

        // Keystrokes reach the surface by context name, so the bindings have
        // to exist before the window that dispatches them.
        surface_view::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1100.0), px(760.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    // Still set: this is the name the window carries into the
                    // Window menu, Mission Control and the app switcher, and
                    // `appears_transparent` does not take that away. What it
                    // takes away is the system *drawing* it, which is why the
                    // cockpit draws its own strip under the traffic lights.
                    title: Some(project_name.clone().into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(9.0), px(9.0))),
                }),
                ..Default::default()
            },
            |window, cx| {
                let cockpit = cx.new(|cx| {
                    Cockpit::new(
                        project_root.clone(),
                        project_name.clone(),
                        first.clone(),
                        cx,
                    )
                });
                // Root has to be the window's first layer: it is what renders
                // the dialog, sheet and notification layers, and the library
                // panics rather than guessing if it is not there.
                cx.new(|cx| gpui_component::Root::new(cockpit, window, cx))
            },
        )
        .expect("open window");
        cx.activate(true);
    });
}

/// Bend gpui-component's dark theme onto the diff surface's palette.
///
/// Two dark greys next to each other read as two applications sharing a
/// window. The surface's colours are the ones that cannot move — they are
/// tuned against the added/removed tints — so the shell moves to them.
fn paint_shell_to_match_the_surface(cx: &mut gpui::App) {
    let ours = theme::Theme::default();
    let theme = UiTheme::global_mut(cx);
    theme.background = ours.background;
    theme.foreground = ours.text;
    theme.border = ours.border;
    theme.muted_foreground = ours.text_dim;
    theme.title_bar = ours.header_bg;
    theme.title_bar_border = ours.border;
    theme.sidebar = ours.surface;
    theme.sidebar_border = ours.border;
    theme.sidebar_foreground = ours.text;
    theme.sidebar_accent = ours.header_bg;
    theme.sidebar_accent_foreground = ours.text;
    // The HUD's neutral chip is a `Tag::secondary`, and `pending` is the
    // colour that chip has always been: grey, deliberately not red.
    theme.secondary = ours.pending;
    theme.secondary_foreground = ours.text;
    // Same for the verdicts — green and red stay the exact greens and reds
    // the diff gutter uses, so a passing receipt matches an added line.
    theme.success = ours.added_emphasis;
    theme.success_foreground = ours.text;
    theme.danger = ours.removed_emphasis;
    theme.danger_foreground = ours.text;

    // The scrollbar's own colours fall back to the theme's background, and we
    // have just moved the background out from under them — leaving a thumb
    // painted in the colour of the thing behind it. Stated outright: the track
    // sinks into the page, the thumb is the gutter grey, and hovering lifts it
    // to the colour of dimmed text.
    theme.scrollbar = ours.background;
    theme.scrollbar_thumb = ours.gutter;
    theme.scrollbar_thumb_hover = ours.text_dim;
}
