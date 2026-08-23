//! The diff surface: one virtualized scroll list over every changed file.
//!
//! This is the port that had to be proven first. The Swift cockpit reached
//! ~0.7s on a 181-file changeset with an `NSTableView` recycling row views;
//! `uniform_list` is the same bargain — fixed-height rows, only the visible
//! window built — and `preceipts_core::Surface` was already shaped for it,
//! since it precomputes one flat row list with anchors.

use crate::theme::Theme;
use gpui::prelude::*;
use gpui::{div, px, uniform_list, Context, FontWeight, SharedString, Window};
use preceipts_core::model::RowKind;
use preceipts_core::segments::line_segments;
use preceipts_core::{Changeset, Surface, SurfaceRow};

/// Row height in pixels. Fixed for the same reason the model is fixed-height:
/// the list must be able to place a row without laying it out.
const ROW_HEIGHT: f32 = 20.0;
const GUTTER_WIDTH: f32 = 52.0;

pub struct SurfaceView {
    changeset: Changeset,
    surface: Surface,
    theme: Theme,
    /// Resolved once, because asking the text system what is installed is not
    /// a question to answer on every frame.
    code_font: gpui::SharedString,
}

impl SurfaceView {
    pub fn new(changeset: Changeset, code_font: gpui::SharedString) -> Self {
        let surface = Surface::build(&changeset);
        Self {
            changeset,
            surface,
            theme: Theme::default(),
            code_font,
        }
    }

    pub fn row_count(&self) -> usize {
        self.surface.rows.len()
    }

    fn render_row(&self, index: usize) -> gpui::AnyElement {
        match self.surface.rows[index] {
            SurfaceRow::FileHeader { file } => self.render_header(file),
            SurfaceRow::Gap { skipped } => self.render_gap(skipped),
            SurfaceRow::Line { file, hunk, row } => self.render_line(file, hunk, row),
        }
    }

    fn render_header(&self, file: usize) -> gpui::AnyElement {
        let file = &self.changeset.files[file];
        let stats = format!("+{}  −{}", file.added, file.removed);
        div()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .bg(self.theme.header_bg)
            .border_b_1()
            .border_color(self.theme.border)
            .child(
                div()
                    .w(px(14.0))
                    .text_color(self.theme.text_dim)
                    .child(SharedString::from(file.status.code().to_string())),
            )
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(self.theme.text)
                    .child(SharedString::from(file.path.clone())),
            )
            .child(
                div()
                    .text_color(self.theme.text_dim)
                    .child(SharedString::from(stats)),
            )
            .into_any_element()
    }

    fn render_gap(&self, skipped: usize) -> gpui::AnyElement {
        div()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .px_2()
            .bg(self.theme.surface)
            .text_color(self.theme.text_dim)
            .child(SharedString::from(format!("⋯ {skipped} unchanged")))
            .into_any_element()
    }

    fn render_line(&self, file: usize, hunk: usize, row: usize) -> gpui::AnyElement {
        let file_diff = &self.changeset.files[file];
        let diff_row = &file_diff.hunks[hunk].rows[row];

        // A change row shows its new side here; the old side is the same row's
        // counterpart once the split view lands. Removals show the old side.
        let (line, changed, highlight, background) = match diff_row.kind {
            RowKind::Removal => (
                diff_row.old.as_ref(),
                &diff_row.old_changed,
                file_diff.old_highlight.as_ref(),
                Some(self.theme.removed_bg),
            ),
            RowKind::Addition => (
                diff_row.new.as_ref(),
                &diff_row.new_changed,
                file_diff.new_highlight.as_ref(),
                Some(self.theme.added_bg),
            ),
            RowKind::Change => (
                diff_row.new.as_ref(),
                &diff_row.new_changed,
                file_diff.new_highlight.as_ref(),
                Some(self.theme.added_bg),
            ),
            RowKind::Context => (
                diff_row.new.as_ref(),
                &diff_row.new_changed,
                file_diff.new_highlight.as_ref(),
                None,
            ),
        };

        let Some(line) = line else {
            return div().h(px(ROW_HEIGHT)).into_any_element();
        };

        let emphasis = match diff_row.kind {
            RowKind::Removal => self.theme.removed_emphasis,
            _ => self.theme.added_emphasis,
        };

        let syntax = highlight.map(|h| h.line(line.number)).unwrap_or(&[]);
        let segments = line_segments(&line.text, syntax, changed);

        let mut content = div().flex().items_center().flex_none();
        for segment in segments {
            let mut span = div()
                .text_color(self.theme.syntax(segment.kind))
                .child(SharedString::from(segment.text));
            if segment.emphasized {
                span = span.bg(emphasis);
            }
            content = content.child(span);
        }

        let mut root = div()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .child(
                div()
                    .w(px(GUTTER_WIDTH))
                    .flex_none()
                    .px_2()
                    .text_color(self.theme.gutter)
                    .child(SharedString::from(gutter_label(diff_row))),
            )
            .child(content);
        if let Some(background) = background {
            root = root.bg(background);
        }
        root.into_any_element()
    }
}

/// Old and new line numbers, whichever the row has.
fn gutter_label(row: &preceipts_core::DiffRow) -> String {
    match (&row.old, &row.new) {
        (Some(old), Some(new)) if old.number == new.number => format!("{:>5}", new.number),
        (Some(old), Some(new)) => format!("{:>2} {:>2}", old.number, new.number),
        (Some(old), None) => format!("{:>5}", old.number),
        (None, Some(new)) => format!("{:>5}", new.number),
        (None, None) => String::new(),
    }
}

impl Render for SurfaceView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.row_count();
        let view = cx.entity().clone();

        div()
            .size_full()
            .bg(self.theme.background)
            .text_color(self.theme.text)
            .font_family(self.code_font.clone())
            .text_size(px(12.0))
            .child(
                uniform_list("diff-surface", count, move |range, _window, cx| {
                    let surface = view.read(cx);
                    range
                        .map(|index| surface.render_row(index))
                        .collect::<Vec<_>>()
                })
                .size_full(),
            )
    }
}

/// Used by `main` to report what was loaded before the window opens.
pub fn summary(changeset: &Changeset) -> String {
    format!(
        "{} files, +{} −{} vs {}",
        changeset.files.len(),
        changeset.total_added(),
        changeset.total_removed(),
        changeset.base_name,
    )
}
