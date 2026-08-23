//! The diff surface: one virtualized scroll list over every changed file.
//!
//! This is the port that had to be proven first. The Swift cockpit reached
//! ~0.7s on a 181-file changeset with an `NSTableView` recycling row views;
//! `uniform_list` is the same bargain — fixed-height rows, only the visible
//! window built — and `preceipts_core::Surface` was already shaped for it,
//! since it precomputes one flat row list with anchors.
//!
//! Text is selectable to the character, which is why each line is a single
//! `StyledText` carrying its syntax colours as highlight runs rather than a
//! row of coloured `div`s. A div per segment cannot be selected across, and
//! cannot be hit-tested into: asking "which character is under this x" needs
//! one shaped line, not eleven boxes that happen to sit side by side.

use crate::theme::Theme;
use gpui::prelude::*;
use gpui::{
    actions, canvas, div, font, px, uniform_list, App, Bounds, ClipboardItem, Context, Entity,
    FocusHandle, FontWeight, Hsla, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    ScrollStrategy, SharedString, StyledText, TextRun, UniformListScrollHandle, Window,
};
use preceipts_core::model::RowKind;
use preceipts_core::segments::line_segments;
use preceipts_core::{Changeset, Surface, SurfaceRow};
use std::ops::Range;

/// Row height in pixels. Fixed for the same reason the model is fixed-height:
/// the list must be able to place a row without laying it out.
const ROW_HEIGHT: f32 = 20.0;
const GUTTER_WIDTH: f32 = 52.0;
const FONT_SIZE: f32 = 12.0;

actions!(diff_surface, [CopySelection, SelectAll, ClearSelection]);

/// Bound once, on the application rather than the view, because a keystroke is
/// dispatched by context name and the context is what scopes it here.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-c", CopySelection, Some("DiffSurface")),
        KeyBinding::new("cmd-a", SelectAll, Some("DiffSurface")),
        KeyBinding::new("escape", ClearSelection, Some("DiffSurface")),
    ]);
}

/// A point in the surface: which row, and how many bytes into what that row
/// *displays*. Display offsets, not source offsets — tabs are expanded before
/// they reach the screen, and a selection has to mean what the eye saw.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Cursor {
    row: usize,
    column: usize,
}

/// One row's text and how to paint it. The single seam between rendering and
/// copying: both ask this what the row says, so they cannot disagree about it.
struct DisplayLine {
    text: String,
    /// Non-overlapping, in order, covering every byte.
    runs: Vec<(Range<usize>, Hsla, Option<Hsla>)>,
    background: Option<Hsla>,
}

/// One row of the file list: enough to render it without handing out the
/// changeset.
pub struct FileEntry {
    pub path: SharedString,
    pub status: SharedString,
    pub added: usize,
    pub removed: usize,
}

pub struct SurfaceView {
    changeset: Changeset,
    surface: Surface,
    theme: Theme,
    /// Resolved once, because asking the text system what is installed is not
    /// a question to answer on every frame.
    code_font: SharedString,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// Where the list sits in the window. Captured each frame by a `canvas`,
    /// because a mouse position arrives in window coordinates and a column is
    /// measured from the left edge of the text.
    bounds: Bounds<Pixels>,
    /// The first row the list built last frame. `uniform_list` keeps its scroll
    /// offset private, so this — not arithmetic on a scroll position — is how
    /// the sticky header knows which file is under the top of the viewport.
    first_visible: usize,
    anchor: Option<Cursor>,
    head: Option<Cursor>,
    selecting: bool,
    /// Dragging the scrollbar. Distinct from `selecting` so a scrub never
    /// leaves a stray text selection behind it.
    scrubbing: bool,
}

impl SurfaceView {
    pub fn new(changeset: Changeset, code_font: SharedString, cx: &mut Context<Self>) -> Self {
        let surface = Surface::build(&changeset);
        Self {
            changeset,
            surface,
            theme: Theme::default(),
            code_font,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            bounds: Bounds::default(),
            first_visible: 0,
            anchor: None,
            head: None,
            selecting: false,
            scrubbing: false,
        }
    }

    /// Swap in a freshly loaded changeset, keeping the reader where they were.
    ///
    /// Row indices do not survive a reload — a file that grew by ten lines
    /// moves every row beneath it — so the thing held onto is the *file* the
    /// top of the viewport was inside, found again by path. That is also what
    /// a person means by "where I was" while an agent edits underneath them.
    ///
    /// The selection is dropped rather than remapped. A pair of row indices
    /// into a surface that no longer exists is not a selection that got
    /// slightly wrong; it is a highlight over unrelated text.
    pub fn replace(&mut self, changeset: Changeset, cx: &mut Context<Self>) {
        let anchor_path = self
            .file_of(self.first_visible)
            .and_then(|index| self.changeset.files.get(index))
            .map(|file| file.path.clone());

        self.surface = Surface::build(&changeset);
        self.changeset = changeset;
        self.anchor = None;
        self.head = None;
        self.selecting = false;

        if let Some(row) = anchor_path
            .and_then(|path| self.changeset.files.iter().position(|f| f.path == path))
            .and_then(|index| self.surface.file_anchors.get(index).copied())
        {
            self.first_visible = row;
            self.scroll.scroll_to_item(row, ScrollStrategy::Top);
        }
        cx.notify();
    }

    pub fn row_count(&self) -> usize {
        self.surface.rows.len()
    }

    /// Every changed file, in surface order.
    pub fn file_entries(&self) -> Vec<FileEntry> {
        self.changeset
            .files
            .iter()
            .map(|file| FileEntry {
                path: SharedString::from(file.path.clone()),
                status: SharedString::from(file.status.code().to_string()),
                added: file.added,
                removed: file.removed,
            })
            .collect()
    }

    /// The file the top of the viewport is inside — which file you are in,
    /// for a list that wants to say so.
    pub fn current_file(&self) -> Option<usize> {
        self.file_of(self.first_visible)
    }

    /// Scroll so `file` starts at the top. `Surface` already records where
    /// each file's header landed, so this is a lookup rather than a search.
    pub fn jump_to_file(&mut self, file: usize, cx: &mut Context<Self>) {
        if let Some(&row) = self.surface.file_anchors.get(file) {
            self.scroll.scroll_to_item(row, ScrollStrategy::Top);
            cx.notify();
        }
    }

    /// The selection as an ordered pair, or `None` when it is empty. Dragging
    /// upwards puts the head before the anchor, and everything downstream
    /// wants start-to-end.
    fn selection(&self) -> Option<(Cursor, Cursor)> {
        let (anchor, head) = (self.anchor?, self.head?);
        if anchor == head {
            return None;
        }
        Some(if anchor <= head {
            (anchor, head)
        } else {
            (head, anchor)
        })
    }

    /// What row `index` shows, and in what colours. `None` for rows that are
    /// not code — a header or a gap has nothing to select or copy.
    fn display_line(&self, index: usize) -> Option<DisplayLine> {
        let SurfaceRow::Line { file, hunk, row } = self.surface.rows[index] else {
            return None;
        };
        let file_diff = &self.changeset.files[file];
        let diff_row = &file_diff.hunks[hunk].rows[row];

        // A removal shows its old side; everything else shows the new one.
        let (line, changed, highlight, background) = match diff_row.kind {
            RowKind::Removal => (
                diff_row.old.as_ref(),
                &diff_row.old_changed,
                file_diff.old_highlight.as_ref(),
                Some(self.theme.removed_bg),
            ),
            RowKind::Addition | RowKind::Change => (
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
        let line = line?;

        let emphasis = match diff_row.kind {
            RowKind::Removal => self.theme.removed_emphasis,
            _ => self.theme.added_emphasis,
        };

        let syntax = highlight.map(|h| h.line(line.number)).unwrap_or(&[]);
        let mut text = String::new();
        let mut runs = Vec::new();
        for segment in line_segments(&line.text, syntax, changed) {
            let start = text.len();
            text.push_str(&segment.text);
            runs.push((
                start..text.len(),
                self.theme.syntax(segment.kind),
                segment.emphasized.then_some(emphasis),
            ));
        }

        Some(DisplayLine {
            text,
            runs,
            background,
        })
    }

    /// The part of `index` that is selected, in display-byte offsets.
    fn selected_range(&self, index: usize, len: usize) -> Option<Range<usize>> {
        let (start, end) = self.selection()?;
        if index < start.row || index > end.row {
            return None;
        }
        let from = if index == start.row { start.column } else { 0 };
        let to = if index == end.row { end.column } else { len };
        (from < to).then(|| from.min(len)..to.min(len))
    }

    /// Which character sits under `x` on row `index`.
    ///
    /// Shaped rather than divided by an assumed character width: the font is
    /// monospace, but a line containing anything wide — a box-drawing rune, a
    /// CJK identifier — would put every column after it in the wrong place,
    /// and the selection would look right until exactly the moment it mattered.
    fn cursor_at(&self, index: usize, x: Pixels, window: &Window) -> Cursor {
        let Some(line) = self.display_line(index) else {
            return Cursor {
                row: index,
                column: 0,
            };
        };
        let content_left = self.bounds.origin.x + px(GUTTER_WIDTH);
        let local = (x - content_left).max(px(0.0));
        let run = TextRun {
            len: line.text.len(),
            font: font(self.code_font.clone()),
            color: self.theme.text,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window.text_system().shape_line(
            SharedString::from(line.text.clone()),
            px(FONT_SIZE),
            &[run],
            None,
        );
        Cursor {
            row: index,
            column: shaped.closest_index_for_x(local),
        }
    }

    /// The selected text, exactly as the screen shows it: no gutter, no `+`/`-`
    /// marker, no file header. What you paste should be something you could
    /// have pasted from the file it came from.
    fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection()?;
        let mut out = String::new();
        for index in start.row..=end.row {
            let Some(line) = self.display_line(index) else {
                // Headers and gaps are furniture, not content. Skipping them
                // means a selection spanning two files yields their lines and
                // nothing between, which is what a paste wants.
                continue;
            };
            let Some(range) = self.selected_range(index, line.text.len()) else {
                continue;
            };
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&line.text[range]);
        }
        (!out.is_empty()).then_some(out)
    }

    fn copy_selection(&mut self, _: &CopySelection, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn select_all(&mut self, _: &SelectAll, _window: &mut Window, cx: &mut Context<Self>) {
        let last = self.row_count().saturating_sub(1);
        let end = self.display_line(last).map(|l| l.text.len()).unwrap_or(0);
        self.anchor = Some(Cursor { row: 0, column: 0 });
        self.head = Some(Cursor {
            row: last,
            column: end,
        });
        cx.notify();
    }

    fn clear_selection(
        &mut self,
        _: &ClearSelection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.anchor = None;
        self.head = None;
        self.selecting = false;
        cx.notify();
    }

    /// The file a row belongs to, walking back past gaps, which carry no file
    /// of their own.
    fn file_of(&self, index: usize) -> Option<usize> {
        self.surface.rows[..=index.min(self.row_count().saturating_sub(1))]
            .iter()
            .rev()
            .find_map(|row| match row {
                SurfaceRow::FileHeader { file } => Some(*file),
                SurfaceRow::Line { file, .. } => Some(*file),
                SurfaceRow::Gap { .. } => None,
            })
    }

    /// A scrollbar, drawn here rather than taken from gpui-component.
    ///
    /// Its `Scrollbar` binds to `UniformListScrollHandle` and ought to have
    /// been a one-liner, but it renders nothing in this window — proven by
    /// putting a plain coloured bar at the same coordinates, which paints
    /// fine. Rather than debug someone else's element from the outside, this
    /// draws the two rectangles it needs, and it can be honest in a way the
    /// generic one cannot: the position comes from the first row the list
    /// actually built, so the thumb tracks rows rather than a pixel offset a
    /// virtualized list never exposes.
    fn render_scrollbar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        const WIDTH: f32 = 11.0;
        const MIN_THUMB: f32 = 28.0;

        let rows = self.row_count().max(1) as f32;
        let track = f32::from(self.bounds.size.height).max(1.0);
        let visible = (track / ROW_HEIGHT).min(rows);
        // Nothing to scroll, nothing to say. A bar that always reads "all of
        // it" is furniture.
        if visible >= rows {
            return div().into_any_element();
        }

        let thumb = (visible / rows * track).max(MIN_THUMB).min(track);
        let progress = (self.first_visible as f32 / (rows - visible)).clamp(0.0, 1.0);
        let top = progress * (track - thumb);

        // Clicking or dragging anywhere on the track goes there. The row is
        // the unit, so it lands on a row boundary rather than mid-line.
        fn scrub(this: &mut SurfaceView, y: Pixels, cx: &mut Context<SurfaceView>) {
            let track = f32::from(this.bounds.size.height).max(1.0);
            let rows = this.row_count().max(1) as f32;
            let visible = (track / ROW_HEIGHT).min(rows);
            if visible >= rows {
                return;
            }
            let thumb = (visible / rows * track).max(MIN_THUMB).min(track);
            let local = f32::from(y - this.bounds.origin.y).clamp(0.0, track);
            let usable = (track - thumb).max(1.0);
            let progress = ((local - thumb / 2.0) / usable).clamp(0.0, 1.0);
            let row = (progress * (rows - visible)).round() as usize;
            this.scroll.scroll_to_item(row, ScrollStrategy::Top);
            cx.notify();
        }

        div()
            .absolute()
            .top_0()
            .right_0()
            .w(px(WIDTH))
            .h_full()
            .bg(self.theme.surface)
            .border_l_1()
            .border_color(self.theme.border)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.scrubbing = true;
                    scrub(this, event.position.y, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.scrubbing {
                    scrub(this, event.position.y, cx);
                }
            }))
            .child(
                div()
                    .absolute()
                    .left(px(2.0))
                    .right(px(2.0))
                    .top(px(top))
                    .h(px(thumb))
                    .rounded_full()
                    .bg(self.theme.gutter),
            )
            .into_any_element()
    }

    fn render_row(&self, index: usize, handle: &Entity<Self>) -> gpui::AnyElement {
        match self.surface.rows[index] {
            SurfaceRow::FileHeader { file } => self.render_header(file, false),
            SurfaceRow::Gap { skipped } => self.render_gap(skipped),
            SurfaceRow::Line { .. } => self.render_line(index, handle),
        }
    }

    /// `floating` is the sticky copy pinned over the top of the list. It reads
    /// the same, but casts a shadow so it is legible as an overlay rather than
    /// looking like a row that refuses to scroll.
    fn render_header(&self, file: usize, floating: bool) -> gpui::AnyElement {
        let file = &self.changeset.files[file];
        let status = file.status.code().to_string();
        // The letter carries the verdict, so it should not be the same grey as
        // everything else: added green, removed red, modified plain.
        let status_color = match status.as_str() {
            "A" => self.theme.added_emphasis,
            "D" => self.theme.removed_emphasis,
            _ => self.theme.text_dim,
        };
        let mut root = div()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .bg(self.theme.header_bg)
            .border_b_1()
            .border_color(self.theme.border);
        if floating {
            root = root.shadow_md();
        } else {
            // A file starts here. The top rule is what turns a continuous wall
            // of rows into a stack of files.
            root = root.border_t_2().border_color(self.theme.border);
        }
        root.child(
            div()
                .w(px(14.0))
                .font_weight(FontWeight::BOLD)
                .text_color(status_color)
                .child(SharedString::from(status)),
        )
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(self.theme.text)
                .child(SharedString::from(file.path.clone())),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .justify_end()
                .child(
                    div()
                        .text_color(self.theme.added_emphasis)
                        .child(SharedString::from(format!("+{}", file.added))),
                )
                .child(
                    div()
                        .ml_2()
                        .text_color(self.theme.removed_emphasis)
                        .child(SharedString::from(format!("−{}", file.removed))),
                ),
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

    fn render_line(&self, index: usize, handle: &Entity<Self>) -> gpui::AnyElement {
        let SurfaceRow::Line { file, hunk, row } = self.surface.rows[index] else {
            return div().h(px(ROW_HEIGHT)).into_any_element();
        };
        let diff_row = &self.changeset.files[file].hunks[hunk].rows[row];

        let Some(line) = self.display_line(index) else {
            return div().h(px(ROW_HEIGHT)).into_any_element();
        };

        // Selection is painted by splitting the syntax runs at its edges, not
        // by layering a translucent box over them: one run list, every byte
        // covered exactly once, and the text keeps its colour underneath.
        let selected = self.selected_range(index, line.text.len());
        let mut highlights = Vec::with_capacity(line.runs.len() + 2);
        for (range, color, emphasis) in &line.runs {
            let pieces = match &selected {
                Some(sel) => split_run(range, sel),
                None => vec![(range.clone(), false)],
            };
            for (piece, is_selected) in pieces {
                if piece.is_empty() {
                    continue;
                }
                let background = if is_selected {
                    Some(self.theme.selection)
                } else {
                    *emphasis
                };
                highlights.push((
                    piece,
                    gpui::HighlightStyle {
                        color: Some(*color),
                        background_color: background,
                        ..Default::default()
                    },
                ));
            }
        }

        let down = handle.clone();
        let moved = handle.clone();
        let mut root = div()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .on_mouse_down(
                MouseButton::Left,
                move |event: &MouseDownEvent, window, cx| {
                    down.update(cx, |this, cx| {
                        let cursor = this.cursor_at(index, event.position.x, window);
                        // Shift extends the existing selection; a plain click
                        // starts a new one.
                        if event.modifiers.shift && this.anchor.is_some() {
                            this.head = Some(cursor);
                        } else {
                            this.anchor = Some(cursor);
                            this.head = Some(cursor);
                        }
                        this.selecting = true;
                        let focus = this.focus.clone();
                        window.focus(&focus);
                        cx.notify();
                    });
                },
            )
            .on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
                moved.update(cx, |this, cx| {
                    if this.selecting {
                        this.head = Some(this.cursor_at(index, event.position.x, window));
                        cx.notify();
                    }
                });
            })
            .child(
                div()
                    .w(px(GUTTER_WIDTH))
                    .flex_none()
                    .px_2()
                    .text_color(self.theme.gutter)
                    .child(SharedString::from(gutter_label(diff_row))),
            )
            .child(StyledText::new(line.text.clone()).with_highlights(highlights));
        if let Some(background) = line.background {
            root = root.bg(background);
        }
        root.into_any_element()
    }
}

/// Cut `run` against `selection`, tagging each piece with whether it is inside.
fn split_run(run: &Range<usize>, selection: &Range<usize>) -> Vec<(Range<usize>, bool)> {
    let start = selection.start.clamp(run.start, run.end);
    let end = selection.end.clamp(run.start, run.end);
    let mut pieces = Vec::with_capacity(3);
    if run.start < start {
        pieces.push((run.start..start, false));
    }
    if start < end {
        pieces.push((start..end, true));
    }
    if end < run.end {
        pieces.push((end..run.end, false));
    }
    pieces
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

impl gpui::Focusable for SurfaceView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SurfaceView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.row_count();
        let view = cx.entity().clone();
        let measure = cx.entity().clone();

        // The header of whatever file the top of the viewport is inside, unless
        // that file's real header is on screen already.
        let sticky = match self.surface.rows.get(self.first_visible) {
            Some(SurfaceRow::FileHeader { .. }) | None => None,
            _ => self.file_of(self.first_visible),
        };

        let mut root = div()
            .track_focus(&self.focus)
            .key_context("DiffSurface")
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::clear_selection))
            .size_full()
            .relative()
            .bg(self.theme.background)
            .text_color(self.theme.text)
            .font_family(self.code_font.clone())
            .text_size(px(FONT_SIZE))
            // A drag that ends anywhere — including outside the window — has
            // to end the selection, or the next mouse move keeps extending it.
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.selecting = false;
                    this.scrubbing = false;
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.selecting = false;
                    this.scrubbing = false;
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    move |bounds, _window, cx| {
                        measure.update(cx, |this, _| this.bounds = bounds);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                uniform_list("diff-surface", count, move |range, _window, cx| {
                    let start = range.start;
                    view.update(cx, |this, cx| {
                        if this.first_visible != start {
                            this.first_visible = start;
                            cx.notify();
                        }
                    });
                    let handle = view.clone();
                    let surface = view.read(cx);
                    range
                        .map(|index| surface.render_row(index, &handle))
                        .collect::<Vec<_>>()
                })
                .track_scroll(self.scroll.clone())
                .size_full(),
            );

        if let Some(file) = sticky {
            root = root.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .child(self.render_header(file, true)),
            );
        }

        // Last child, so the thumb sits above the sticky header and stays
        // grabbable where the two meet.
        root.child(self.render_scrollbar(cx))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The three-way cut a selection makes in a syntax run. Getting this wrong
    /// paints selection over the wrong characters, and it is the one piece of
    /// the selection that is pure arithmetic and can be pinned down here.
    #[test]
    fn a_run_splits_around_the_selected_part() {
        assert_eq!(
            split_run(&(0..10), &(3..6)),
            vec![(0..3, false), (3..6, true), (6..10, false)]
        );
        // Selection covering the whole run leaves one selected piece.
        assert_eq!(split_run(&(0..10), &(0..10)), vec![(0..10, true)]);
        // Disjoint selection leaves the run untouched and unselected.
        assert_eq!(split_run(&(10..20), &(0..5)), vec![(10..20, false)]);
    }

    /// Every byte of the line has to appear exactly once, or text goes missing
    /// on screen without anything erroring.
    #[test]
    fn splitting_a_run_never_loses_or_repeats_a_byte() {
        for selection in [0..0, 0..4, 2..3, 4..9, 7..12, 0..12] {
            let pieces = split_run(&(2..9), &selection);
            let mut covered = 2;
            for (piece, _) in &pieces {
                assert_eq!(piece.start, covered, "gap or overlap in {pieces:?}");
                covered = piece.end;
            }
            assert_eq!(covered, 9, "run not fully covered by {pieces:?}");
        }
    }
}
