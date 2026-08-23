//! The cockpit.
//!
//! Today it opens one repository and renders its diff — step 3 of
//! docs/direction-2026-08.md, the port worth proving before anything is built
//! on top of it. Tabs, the workspace list, and the env panel come after.

mod surface_view;
mod theme;

use gpui::{
    px, size, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
};
use preceipts_core::{load, DiffScope};
use std::path::PathBuf;
use surface_view::SurfaceView;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `--stats` loads and reports without opening a window. It is how the
    // scroll path gets timed honestly, and it is the seed of the CLI that
    // step 7 grows into an agent-facing surface.
    let stats_only = args.iter().any(|a| a == "--stats");
    let repo = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("a working directory"));

    // Load before opening a window: an unreadable repo should fail on the
    // command line, not as an empty window.
    let started = std::time::Instant::now();
    let changeset = match load(&repo, DiffScope::Branch, true) {
        Ok(changeset) => changeset,
        Err(error) => {
            eprintln!("preceipts: {error}");
            std::process::exit(1);
        }
    };

    let title = format!(
        "{} — {}",
        changeset
            .branch
            .clone()
            .unwrap_or_else(|| "detached".to_string()),
        surface_view::summary(&changeset),
    );
    let rows = preceipts_core::Surface::build(&changeset).rows.len();
    println!(
        "{title}\n{rows} rows built in {:.0}ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    if stats_only {
        return;
    }

    Application::new().run(move |cx| {
        let bounds = Bounds::centered(None, size(px(1100.0), px(760.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.clone().into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_window, cx| cx.new(|_cx| SurfaceView::new(changeset)),
        )
        .expect("open window");
        cx.activate(true);
    });
}
