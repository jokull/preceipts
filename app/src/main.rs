//! preceipts.app — macOS desktop cockpit (GPUI). v0 skeleton:
//! one scroll surface (flattened multibuffer of every changed file),
//! file sidebar, statusbar chrome, ⌘⇧D scope toggle. See
//! docs/desktop-app-design.md.

use std::path::PathBuf;

use gpui::{
    actions, div, prelude::*, px, rgb, size, uniform_list, App, Application, Bounds, Context,
    Menu, MenuItem, ScrollStrategy, SharedString, UniformListScrollHandle, Window, WindowBounds,
    WindowOptions,
};
use preceipts_core::{Changeset, DiffScope, FileStatus, Row, RowKind};

actions!(preceipts, [Quit, ToggleScope]);

const ROW_HEIGHT: f32 = 22.0;

// Dark theme, hardcoded for v0 (Zed-adjacent).
const BG: u32 = 0x1e2227;
const BG_PANEL: u32 = 0x23272e;
const BG_HEADER: u32 = 0x2c313a;
const FG: u32 = 0xc8ccd4;
const FG_MUTED: u32 = 0x7f848e;
const ADD_BG: u32 = 0x1e3a28;
const EMPTY_BG: u32 = 0x22282c;
const DEL_BG: u32 = 0x3f2528;
const ACCENT: u32 = 0x61afef;
const GREEN: u32 = 0x98c379;
const RED: u32 = 0xe06c75;
const YELLOW: u32 = 0xe5c07b;

/// One entry in the flattened scroll surface. Fixed-height by construction
/// (the load-bearing decision from docs/desktop-foundations.md).
enum SurfaceRow {
    FileHeader { file: usize },
    Gap { skipped: u32 },
    Line { file: usize, hunk: usize, row: usize },
}

struct Cockpit {
    repo: PathBuf,
    scope: DiffScope,
    changeset: Option<Changeset>,
    error: Option<String>,
    surface: Vec<SurfaceRow>,
    /// surface index of each file's header, aligned with changeset.files.
    file_anchors: Vec<usize>,
    scroll: UniformListScrollHandle,
}

impl Cockpit {
    fn new(repo: PathBuf) -> Self {
        let mut cockpit = Cockpit {
            repo,
            scope: DiffScope::Branch,
            changeset: None,
            error: None,
            surface: Vec::new(),
            file_anchors: Vec::new(),
            scroll: UniformListScrollHandle::new(),
        };
        cockpit.reload();
        cockpit
    }

    fn reload(&mut self) {
        match Changeset::load(&self.repo, self.scope) {
            Ok(changeset) => {
                self.error = None;
                self.rebuild_surface(&changeset);
                self.changeset = Some(changeset);
            }
            Err(err) => {
                self.error = Some(err.to_string());
                self.changeset = None;
                self.surface.clear();
                self.file_anchors.clear();
            }
        }
    }

    fn rebuild_surface(&mut self, changeset: &Changeset) {
        self.surface.clear();
        self.file_anchors.clear();
        for (file_idx, file) in changeset.files.iter().enumerate() {
            self.file_anchors.push(self.surface.len());
            self.surface.push(SurfaceRow::FileHeader { file: file_idx });
            for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
                if hunk.skipped_before > 0 {
                    self.surface.push(SurfaceRow::Gap {
                        skipped: hunk.skipped_before,
                    });
                }
                for row_idx in 0..hunk.rows.len() {
                    self.surface.push(SurfaceRow::Line {
                        file: file_idx,
                        hunk: hunk_idx,
                        row: row_idx,
                    });
                }
            }
        }
    }

    fn toggle_scope(&mut self, _: &ToggleScope, _window: &mut Window, cx: &mut Context<Self>) {
        self.scope = match self.scope {
            DiffScope::Branch => DiffScope::Uncommitted,
            DiffScope::Uncommitted => DiffScope::Branch,
        };
        self.reload();
        cx.notify();
    }

    fn render_surface_row(&self, ix: usize) -> gpui::Div {
        let Some(changeset) = &self.changeset else {
            return div();
        };
        let base = div()
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .text_size(px(12.0));
        match &self.surface[ix] {
            SurfaceRow::FileHeader { file } => {
                let f = &changeset.files[*file];
                let status_color = match f.status {
                    FileStatus::Added => GREEN,
                    FileStatus::Deleted => RED,
                    _ => YELLOW,
                };
                base.bg(rgb(BG_HEADER))
                    .px_3()
                    .gap_2()
                    .child(
                        div()
                            .text_color(rgb(status_color))
                            .child(SharedString::from(f.status.glyph())),
                    )
                    .child(
                        div()
                            .text_color(rgb(FG))
                            .child(SharedString::from(f.path.clone())),
                    )
                    .child(
                        div()
                            .text_color(rgb(GREEN))
                            .child(SharedString::from(format!("+{}", f.added))),
                    )
                    .child(
                        div()
                            .text_color(rgb(RED))
                            .child(SharedString::from(format!("\u{2212}{}", f.removed))),
                    )
            }
            SurfaceRow::Gap { skipped } => base
                .px_3()
                .bg(rgb(BG_PANEL))
                .text_color(rgb(FG_MUTED))
                .child(SharedString::from(format!(
                    "\u{22ef} {skipped} unchanged lines"
                ))),
            SurfaceRow::Line { file, hunk, row } => {
                let r = &changeset.files[*file].hunks[*hunk].rows[*row];
                base.child(render_side(r, true)).child(render_side(r, false))
            }
        }
    }
}

/// One side (old/new) of a diff row: gutter line number + text.
fn render_side(row: &Row, old_side: bool) -> gpui::Div {
    let line = if old_side { &row.old } else { &row.new };
    let bg = match (row.kind, old_side, line.is_some()) {
        (RowKind::Context, _, _) => BG,
        (_, _, false) => EMPTY_BG,
        (RowKind::Removal, true, true) => DEL_BG,
        (RowKind::Addition, false, true) => ADD_BG,
        (RowKind::Change, true, true) => DEL_BG,
        (RowKind::Change, false, true) => ADD_BG,
        _ => BG,
    };
    let number = line
        .as_ref()
        .map(|l| l.number.to_string())
        .unwrap_or_default();
    let text = line.as_ref().map(|l| l.text.clone()).unwrap_or_default();
    div()
        .w_1_2()
        .h_full()
        .flex()
        .items_center()
        .bg(rgb(bg))
        .child(
            div()
                .w(px(44.0))
                .flex_none()
                .pr_2()
                .text_color(rgb(FG_MUTED))
                .child(SharedString::from(number)),
        )
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .text_color(rgb(FG))
                .child(SharedString::from(text)),
        )
}

impl Render for Cockpit {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let changeset = self.changeset.as_ref();
        let scope_label = match self.scope {
            DiffScope::Branch => changeset
                .map(|c| format!("Branch diff: {}", c.info.base_name))
                .unwrap_or_else(|| "Branch diff".to_string()),
            DiffScope::Uncommitted => "Uncommitted".to_string(),
        };
        let branch = changeset
            .and_then(|c| c.info.branch.clone())
            .unwrap_or_else(|| "(detached)".to_string());
        let totals = changeset
            .map(|c| {
                format!(
                    "{} files  +{} \u{2212}{}",
                    c.files.len(),
                    c.total_added(),
                    c.total_removed()
                )
            })
            .unwrap_or_default();

        let file_count = changeset.map(|c| c.files.len()).unwrap_or(0);
        let anchors = self.file_anchors.clone();
        let scroll = self.scroll.clone();
        let sidebar = div()
            .w(px(260.0))
            .flex_none()
            .h_full()
            .bg(rgb(BG_PANEL))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(11.0))
                    .text_color(rgb(FG_MUTED))
                    .child(SharedString::from(totals.clone())),
            )
            .child(
                uniform_list(
                    "files",
                    file_count,
                    cx.processor(move |this: &mut Cockpit,
                                       range: std::ops::Range<usize>,
                                       _window,
                                       _cx| {
                        let Some(changeset) = &this.changeset else {
                            return Vec::new();
                        };
                        let mut items = Vec::new();
                        for ix in range {
                            let file = &changeset.files[ix];
                            let color = match file.status {
                                FileStatus::Added => GREEN,
                                FileStatus::Deleted => RED,
                                _ => YELLOW,
                            };
                            let anchors = anchors.clone();
                            let scroll = scroll.clone();
                            items.push(
                                div()
                                    .id(ix)
                                    .h(px(ROW_HEIGHT))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(BG_HEADER)))
                                    .on_click(move |_event, _window, _cx| {
                                        if let Some(anchor) = anchors.get(ix) {
                                            scroll.scroll_to_item(*anchor, ScrollStrategy::Top);
                                        }
                                    })
                                    .text_size(px(12.0))
                                    .child(
                                        div()
                                            .text_color(rgb(color))
                                            .child(SharedString::from(file.status.glyph())),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .overflow_hidden()
                                            .text_color(rgb(FG))
                                            .child(SharedString::from(file.path.clone())),
                                    ),
                            );
                        }
                        items
                    }),
                )
                .flex_1(),
            );

        let surface: gpui::Div = if let Some(error) = &self.error {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(FG_MUTED))
                .child(SharedString::from(error.clone()))
        } else {
            div().flex_1().h_full().overflow_hidden().child(
                uniform_list(
                    "surface",
                    self.surface.len(),
                    cx.processor(|this: &mut Cockpit,
                                  range: std::ops::Range<usize>,
                                  _window,
                                  _cx| {
                        range.map(|ix| this.render_surface_row(ix)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(self.scroll.clone())
                .h_full(),
            )
        };

        let statusbar = div()
            .h(px(26.0))
            .flex_none()
            .w_full()
            .bg(rgb(BG_PANEL))
            .flex()
            .items_center()
            .px_3()
            .gap_3()
            .text_size(px(11.0))
            .text_color(rgb(FG_MUTED))
            .child(
                div()
                    .text_color(rgb(ACCENT))
                    .child(SharedString::from(scope_label)),
            )
            .child(SharedString::from(branch))
            .child(div().flex_1())
            .child(SharedString::from(totals))
            .child(SharedString::from("\u{2318}\u{21e7}D scope"));

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .font_family("Menlo")
            .on_action(cx.listener(Self::toggle_scope))
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .overflow_hidden()
                    .child(sidebar)
                    .child(surface),
            )
            .child(statusbar)
    }
}

fn main() {
    let repo = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    Application::new().run(move |cx: &mut App| {
        cx.activate(true);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([
            gpui::KeyBinding::new("cmd-q", Quit, None),
            gpui::KeyBinding::new("cmd-shift-d", ToggleScope, None),
        ]);
        cx.set_menus(vec![
            Menu {
                name: "preceipts".into(),
                items: vec![MenuItem::action("Quit preceipts", Quit)],
            },
            Menu {
                name: "View".into(),
                items: vec![MenuItem::action("Toggle Diff Scope", ToggleScope)],
            },
        ]);
        let bounds = Bounds::centered(None, size(px(1280.0), px(860.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Cockpit::new(repo.clone())),
        )
        .unwrap();
    });
}
