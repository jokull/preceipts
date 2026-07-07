//! All rendering for the Cockpit: scroll surface, sidebar, find/compose
//! bars, feedback panel, statusbar.

use gpui::{
    div, prelude::*, px, rgb, uniform_list, ClipboardItem, Context, Div, ScrollStrategy,
    SharedString, Window,
};
use preceipts_core::{
    comments::{format_comment, CommentContext},
    DiffScope, FeedbackKind, FileStatus, Row, RowKind,
};

use crate::cockpit::{Cockpit, InputMode, SurfaceRow};
use crate::segments::line_segments;
use crate::theme::*;

pub const ROW_HEIGHT: f32 = 22.0;

fn status_color(status: FileStatus) -> u32 {
    match status {
        FileStatus::Added => GREEN,
        FileStatus::Deleted => RED,
        _ => YELLOW,
    }
}

impl gpui::Render for Cockpit {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .font_family("Menlo")
            .on_action(cx.listener(Self::toggle_scope))
            .on_action(cx.listener(Self::find_open))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_prev))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::toggle_feedback))
            .on_action(cx.listener(Self::compose_comment))
            .on_action(cx.listener(Self::copy_all_feedback))
            .on_key_down(cx.listener(|this, event, window, cx| {
                this.handle_key(event, window, cx);
            }));

        let mut middle = div()
            .flex_1()
            .w_full()
            .flex()
            .overflow_hidden()
            .child(self.render_sidebar(cx))
            .child(self.render_surface(cx));
        if self.feedback_open {
            middle = middle.child(self.render_feedback_panel(cx));
        }
        root = root.child(middle);

        if self.input_mode == InputMode::Find {
            root = root.child(self.render_find_bar());
        }
        if self.input_mode == InputMode::Compose {
            root = root.child(self.render_compose_bar());
        }
        root.child(self.render_statusbar())
    }
}

impl Cockpit {
    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> Div {
        let file_count = self.changeset.as_ref().map(|c| c.files.len()).unwrap_or(0);
        let totals = self.totals_label();
        div()
            .w(px(260.0))
            .flex_none()
            .h_full()
            .bg(rgb(BG_PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(11.0))
                    .text_color(rgb(FG_MUTED))
                    .child(SharedString::from(totals)),
            )
            .child(
                uniform_list(
                    "files",
                    file_count,
                    cx.processor(
                        move |this: &mut Cockpit,
                              range: std::ops::Range<usize>,
                              _window,
                              _cx| {
                            let Some(changeset) = &this.changeset else {
                                return Vec::new();
                            };
                            let mut items = Vec::new();
                            for ix in range {
                                let file = &changeset.files[ix];
                                let anchor = this.file_anchors.get(ix).copied();
                                let scroll = this.scroll.clone();
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
                                            if let Some(anchor) = anchor {
                                                scroll
                                                    .scroll_to_item(anchor, ScrollStrategy::Top);
                                            }
                                        })
                                        .text_size(px(12.0))
                                        .child(
                                            div().text_color(rgb(status_color(file.status))).child(
                                                SharedString::from(file.status.glyph()),
                                            ),
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
                        },
                    ),
                )
                .flex_1(),
            )
    }

    fn render_surface(&mut self, cx: &mut Context<Self>) -> Div {
        if self.loading {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(FG_MUTED))
                .child("loading\u{2026}");
        }
        if let Some(error) = &self.error {
            if self.changeset.is_none() {
                return div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(FG_MUTED))
                    .child(SharedString::from(error.clone()));
            }
        }
        div().flex_1().h_full().overflow_hidden().child(
            uniform_list(
                "surface",
                self.surface.len(),
                cx.processor(
                    |this: &mut Cockpit, range: std::ops::Range<usize>, _window, cx| {
                        range
                            .map(|ix| this.render_surface_row(ix, cx))
                            .collect::<Vec<_>>()
                    },
                ),
            )
            .track_scroll(self.scroll.clone())
            .h_full(),
        )
    }

    fn render_surface_row(&mut self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let is_match = self.find_matches.binary_search(&ix).is_ok();
        let is_current_match =
            is_match && self.find_matches.get(self.find_current) == Some(&ix);
        let is_selected = self.selected_row == Some(ix);

        // Precompute highlight cache presence before borrowing changeset.
        if let Some(SurfaceRow::Line { file, .. }) = self.surface.get(ix) {
            let file = *file;
            self.highlight_for(file, true);
            self.highlight_for(file, false);
        }

        let Some(changeset) = &self.changeset else {
            return div().into_any_element();
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
                let mut header = base
                    .bg(rgb(if is_current_match { FIND_CURRENT_BG } else { BG_HEADER }))
                    .px_3()
                    .gap_2()
                    .child(
                        div()
                            .text_color(rgb(status_color(f.status)))
                            .child(SharedString::from(f.status.glyph())),
                    )
                    .child(div().text_color(rgb(FG)).child(SharedString::from(
                        match &f.old_path {
                            Some(old) => format!("{} \u{2190} {}", f.path, old),
                            None => f.path.clone(),
                        },
                    )))
                    .child(
                        div()
                            .text_color(rgb(GREEN))
                            .child(SharedString::from(format!("+{}", f.added))),
                    )
                    .child(
                        div()
                            .text_color(rgb(RED))
                            .child(SharedString::from(format!("\u{2212}{}", f.removed))),
                    );
                if f.is_binary {
                    header = header.child(
                        div().text_color(rgb(FG_MUTED)).child("binary"),
                    );
                }
                header.into_any_element()
            }
            SurfaceRow::Gap { skipped } => base
                .px_3()
                .bg(rgb(BG_PANEL))
                .text_color(rgb(FG_MUTED))
                .child(SharedString::from(format!(
                    "\u{22ef} {skipped} unchanged lines"
                )))
                .into_any_element(),
            SurfaceRow::Line { file, hunk, row } => {
                let file_idx = *file;
                let r = &changeset.files[file_idx].hunks[*hunk].rows[*row];
                let row_tint = if is_current_match {
                    Some(FIND_CURRENT_BG)
                } else if is_match {
                    Some(FIND_BG)
                } else if is_selected {
                    Some(BG_HEADER)
                } else {
                    None
                };
                let old_marker = r
                    .old
                    .as_ref()
                    .is_some_and(|l| self.draft_anchors.contains(&(file_idx, l.number, true)));
                let new_marker = r
                    .new
                    .as_ref()
                    .is_some_and(|l| self.draft_anchors.contains(&(file_idx, l.number, false)));
                base.id(ix)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.selected_row = Some(ix);
                        cx.notify();
                    }))
                    .child(self.render_side(file_idx, r, true, row_tint, old_marker))
                    .child(self.render_side(file_idx, r, false, row_tint, new_marker))
                    .into_any_element()
            }
        }
    }

    fn render_side(
        &self,
        file_idx: usize,
        row: &Row,
        old_side: bool,
        row_tint: Option<u32>,
        has_draft: bool,
    ) -> Div {
        let line = if old_side { &row.old } else { &row.new };
        let (line_bg, word_bg) = match (row.kind, old_side, line.is_some()) {
            (RowKind::Context, _, _) => (BG, BG),
            (_, _, false) => (EMPTY_BG, EMPTY_BG),
            (RowKind::Removal, true, true) | (RowKind::Change, true, true) => (DEL_BG, DEL_BG_WORD),
            (RowKind::Addition, false, true) | (RowKind::Change, false, true) => {
                (ADD_BG, ADD_BG_WORD)
            }
            _ => (BG, BG),
        };
        let line_bg = row_tint.unwrap_or(line_bg);

        let number = line
            .as_ref()
            .map(|l| l.number.to_string())
            .unwrap_or_default();
        let mut side = div()
            .w_1_2()
            .h_full()
            .flex()
            .items_center()
            .bg(rgb(line_bg))
            .child(
                div()
                    .w(px(46.0))
                    .flex_none()
                    .pr_1()
                    .text_color(rgb(if has_draft { ACCENT } else { FG_MUTED }))
                    .child(SharedString::from(if has_draft {
                        format!("\u{25cf}{number}")
                    } else {
                        number
                    })),
            );

        let Some(line) = line else {
            return side;
        };

        let changed = if old_side {
            &row.old_changed
        } else {
            &row.new_changed
        };
        let syntax = self
            .highlights
            .get(&(file_idx, old_side))
            .and_then(|h| h.as_ref())
            .map(|h| h.line(line.number))
            .unwrap_or(&[]);

        let mut text_area = div().flex_1().flex().overflow_hidden();
        for segment in line_segments(&line.text, syntax, changed) {
            let mut piece = div()
                .flex_none()
                .text_color(rgb(syntax_color(segment.kind)))
                .child(SharedString::from(segment.text));
            if segment.emphasized {
                piece = piece.bg(rgb(word_bg));
            }
            text_area = text_area.child(piece);
        }
        side = side.child(text_area);
        side
    }

    fn render_find_bar(&self) -> Div {
        let count = if self.find_matches.is_empty() {
            "0/0".to_string()
        } else {
            format!("{}/{}", self.find_current + 1, self.find_matches.len())
        };
        div()
            .h(px(30.0))
            .flex_none()
            .w_full()
            .bg(rgb(BG_HEADER))
            .border_t_1()
            .border_color(rgb(BORDER))
            .flex()
            .items_center()
            .px_3()
            .gap_2()
            .text_size(px(12.0))
            .child(div().text_color(rgb(ACCENT)).child("find:"))
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(FG))
                    .child(SharedString::from(format!("{}\u{2758}", self.find_query))),
            )
            .child(div().text_color(rgb(FG_MUTED)).child(SharedString::from(count)))
            .child(
                div()
                    .text_color(rgb(FG_MUTED))
                    .child("\u{2318}G next \u{00b7} \u{2318}\u{21e7}G prev \u{00b7} esc"),
            )
    }

    fn render_compose_bar(&self) -> Div {
        let anchor = self
            .selected_line_anchor()
            .map(|(path, line, _, _)| format!("{path}:{line}"))
            .unwrap_or_default();
        div()
            .h(px(30.0))
            .flex_none()
            .w_full()
            .bg(rgb(BG_HEADER))
            .border_t_1()
            .border_color(rgb(BORDER))
            .flex()
            .items_center()
            .px_3()
            .gap_2()
            .text_size(px(12.0))
            .child(div().text_color(rgb(GREEN)).child("comment:"))
            .child(div().text_color(rgb(FG_MUTED)).child(SharedString::from(anchor)))
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(FG))
                    .child(SharedString::from(format!("{}\u{2758}", self.compose_text))),
            )
            .child(
                div()
                    .text_color(rgb(FG_MUTED))
                    .child("enter save \u{00b7} esc cancel"),
            )
    }

    fn render_feedback_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let mut panel = div()
            .w(px(380.0))
            .flex_none()
            .h_full()
            .bg(rgb(BG_PANEL))
            .border_l_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_col()
            .text_size(px(12.0));

        // Header: PR identity + filter + copy-all.
        let title = match (&self.feedback, self.feedback_loading) {
            (_, true) => "fetching PR feedback\u{2026}".to_string(),
            (Some(feedback), _) => format!("PR #{} \u{00b7} {}", feedback.number, feedback.title),
            (None, _) => self
                .feedback_error
                .clone()
                .unwrap_or_else(|| "feedback".to_string()),
        };
        let filter_label = format!("filter: {}", self.feedback_filter.label());
        panel = panel.child(
            div()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(BORDER))
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_color(rgb(FG))
                        .overflow_hidden()
                        .child(SharedString::from(title)),
                )
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(
                            div()
                                .id("filter")
                                .cursor_pointer()
                                .text_color(rgb(ACCENT))
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.feedback_filter = this.feedback_filter.next();
                                    cx.notify();
                                }))
                                .child(SharedString::from(filter_label)),
                        )
                        .child(
                            div()
                                .id("copy-all")
                                .cursor_pointer()
                                .text_color(rgb(ACCENT))
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    let digest = this.combined_digest();
                                    if !digest.trim().is_empty() {
                                        cx.write_to_clipboard(ClipboardItem::new_string(digest));
                                    }
                                }))
                                .child("copy all"),
                        ),
                ),
        );

        let mut list = div().flex_1().flex().flex_col().overflow_hidden();

        // Drafts first: they are the notes the user is writing for the agent.
        if !self.drafts.is_empty() {
            list = list.child(section_label("drafts (local)"));
            for draft in self.drafts.clone() {
                let text = format_comment(&CommentContext {
                    path: &draft.path,
                    line: draft.line,
                    line_text: &draft.line_text,
                    body: &draft.body,
                    author: None,
                });
                let anchor_path = draft.path.clone();
                let anchor_line = draft.line;
                list = list.child(
                    div()
                        .id(("draft", draft.id))
                        .px_3()
                        .py_1()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(BG_HEADER)))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.reveal_anchor(&anchor_path, anchor_line);
                            cx.notify();
                        }))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_color(rgb(ACCENT))
                                        .child(SharedString::from(format!(
                                            "{}:{}",
                                            draft.path, draft.line
                                        ))),
                                )
                                .child(div().flex_1())
                                .child(copy_chip(("draft-copy", draft.id), text, cx))
                                .child(
                                    div()
                                        .id(("draft-del", draft.id))
                                        .cursor_pointer()
                                        .text_color(rgb(RED))
                                        .on_click(cx.listener(move |this, _e, _w, cx| {
                                            this.remove_draft(draft.id);
                                            cx.notify();
                                        }))
                                        .child("\u{2715}"),
                                ),
                        )
                        .child(
                            div()
                                .text_color(rgb(FG))
                                .overflow_hidden()
                                .child(SharedString::from(one_line(&draft.body))),
                        ),
                );
            }
        }

        if let Some(feedback) = self.feedback.clone() {
            list = list.child(section_label("github"));
            for (ix, comment) in feedback
                .comments
                .iter()
                .enumerate()
                .filter(|(_, c)| self.feedback_filter_allows(c.is_bot))
            {
                let text = format_comment(&comment.context());
                let anchor = comment.path.clone().zip(comment.line);
                let mut meta = div()
                    .flex()
                    .gap_2()
                    .child(div().text_color(rgb(YELLOW)).child(SharedString::from(
                        comment.author.clone(),
                    )));
                if comment.is_bot {
                    meta = meta.child(div().text_color(rgb(FG_MUTED)).child("bot"));
                }
                if let Some(state) = &comment.state {
                    meta = meta.child(
                        div()
                            .text_color(rgb(if state == "APPROVED" { GREEN } else { RED }))
                            .child(SharedString::from(state.clone())),
                    );
                }
                if comment.kind == FeedbackKind::ReviewComment {
                    if let (Some(path), Some(line)) = (&comment.path, comment.line) {
                        meta = meta.child(div().text_color(rgb(ACCENT)).child(
                            SharedString::from(format!("{path}:{line}")),
                        ));
                    }
                }
                if comment.outdated {
                    meta = meta.child(div().text_color(rgb(FG_MUTED)).child("outdated"));
                }
                meta = meta
                    .child(div().flex_1())
                    .child(copy_chip(("gh-copy", ix as u64), text, cx));

                list = list.child(
                    div()
                        .id(("gh", ix))
                        .px_3()
                        .py_1()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(BG_HEADER)))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            if let Some((path, line)) = anchor.clone() {
                                this.reveal_anchor(&path, line);
                            }
                            cx.notify();
                        }))
                        .flex()
                        .flex_col()
                        .child(meta)
                        .child(
                            div()
                                .text_color(rgb(FG))
                                .overflow_hidden()
                                .child(SharedString::from(one_line(&comment.body))),
                        ),
                );
            }
        }

        panel.child(list).child(
            div()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(rgb(BORDER))
                .text_color(rgb(FG_MUTED))
                .text_size(px(11.0))
                .child("click row \u{2192} jump \u{00b7} \u{2318}\u{21e7}M comment on selected line"),
        )
    }

    fn render_statusbar(&self) -> Div {
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
        let drafts = if self.drafts.is_empty() {
            String::new()
        } else {
            format!("{} drafts", self.drafts.len())
        };
        div()
            .h(px(26.0))
            .flex_none()
            .w_full()
            .bg(rgb(BG_PANEL))
            .border_t_1()
            .border_color(rgb(BORDER))
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
            .child(SharedString::from(drafts))
            .child(div().flex_1())
            .child(SharedString::from(self.totals_label()))
            .child(SharedString::from(
                "\u{2318}F find \u{00b7} \u{2318}\u{21e7}D scope \u{00b7} \u{2318}\u{21e7}C feedback",
            ))
    }

    fn totals_label(&self) -> String {
        self.changeset
            .as_ref()
            .map(|c| {
                format!(
                    "{} files  +{} \u{2212}{}",
                    c.files.len(),
                    c.total_added(),
                    c.total_removed()
                )
            })
            .unwrap_or_default()
    }
}

fn section_label(label: &'static str) -> Div {
    div()
        .px_3()
        .py_1()
        .text_size(px(11.0))
        .text_color(rgb(FG_MUTED))
        .child(label)
}

fn one_line(body: &str) -> String {
    let mut line: String = body.lines().next().unwrap_or_default().chars().take(90).collect();
    if line.len() < body.trim().len() {
        line.push('\u{2026}');
    }
    line
}

fn copy_chip(id: (&'static str, u64), text: String, cx: &mut Context<Cockpit>) -> impl IntoElement {
    div()
        .id(id)
        .cursor_pointer()
        .text_color(rgb(ACCENT))
        .on_click(cx.listener(move |_this, _event, _window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        }))
        .child("copy")
}
