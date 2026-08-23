//! The cockpit layout: title bar, workspace sidebar, diff surface, status HUD.
//!
//! The shape docs/direction-2026-08.md asks for, minus the tabs and env panel
//! that need the daemon. Glanceable is a hard requirement here rather than a
//! nicety — this window is meant to sit *beside* the agent conversation at half
//! screen width, not to be looked at full-screen.
//!
//! The chrome is gpui-component's: a `Sidebar` of `SidebarMenuItem`s, a real
//! `h_resizable` split between the list and the diff, `Tag`/`Label`/`Divider`
//! in the HUD, and a `TitleBar` drawn under the traffic lights. The diff itself
//! is still hand-drawn — a virtualized, character-selectable diff is not a
//! component anyone ships.

use crate::surface_view::SurfaceView;
use gpui::prelude::*;
use gpui::{div, px, Context, Entity, SharedString, Window};
use gpui_component::divider::Divider;
use gpui_component::label::Label;
use gpui_component::resizable::{h_resizable, resizable_panel};
use gpui_component::sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem};
use gpui_component::tag::Tag;
use gpui_component::{h_flex, v_flex, ActiveTheme as _, Sizable as _, TitleBar};
use preceipts_core::checks::{CheckState, Status};
use preceipts_core::workspace::Workspace;
use preceipts_core::Changeset;

/// How the verdict chip is painted. Not green covers two different facts, and
/// collapsing them is the one thing this HUD must not do: a check that failed
/// is a verdict, a check that never ran is an absence, and painting both red
/// tells a person their tree is broken when nobody has looked at it yet.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Green,
    Failed,
    /// Outstanding, but only because nobody has run it. Never red.
    NotRun,
    /// Nothing to say anything about.
    Absent,
}

pub struct Cockpit {
    workspaces: Vec<Workspace>,
    /// Which workspace this window is showing.
    current: String,
    changeset_summary: (usize, usize, usize, String),
    status: Option<Status>,
    surface: Entity<SurfaceView>,
    /// What the system would have drawn in the title bar, drawn by us instead.
    title: SharedString,
}

impl Cockpit {
    pub fn new(
        changeset: Changeset,
        workspaces: Vec<Workspace>,
        current: String,
        status: Option<Status>,
        title: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        let summary = (
            changeset.files.len(),
            changeset.total_added(),
            changeset.total_removed(),
            changeset.base_name.clone(),
        );
        let code_font = crate::theme::code_font(cx);
        let surface = cx.new(|cx| SurfaceView::new(changeset, code_font, cx));
        // The file list marks the file you are currently inside, so this view
        // has to repaint when the surface scrolls. Child entities do not
        // notify their parent on their own.
        cx.observe(&surface, |_, _, cx| cx.notify()).detach();
        Self {
            workspaces,
            current,
            changeset_summary: summary,
            status,
            surface,
            title,
        }
    }

    fn render_title_bar(&self) -> impl IntoElement {
        TitleBar::new().child(
            h_flex()
                .gap_2()
                .items_center()
                .text_sm()
                .child(self.title.clone()),
        )
    }

    fn render_sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let count = self.workspaces.len();
        let items = self.workspaces.iter().map(|workspace| {
            let branch = workspace
                .branch
                .clone()
                .unwrap_or_else(|| "(detached)".to_string());
            let mut item = SidebarMenuItem::new(SharedString::from(branch))
                .active(workspace.id == self.current);
            // The recorded intent is what makes eight worktrees legible next
            // week, so it rides along on the row rather than hiding behind a
            // hover. It truncates before it pushes the branch name off.
            if let Some(genesis) = &workspace.genesis {
                item = item.suffix(
                    div()
                        .max_w(px(110.0))
                        .flex_shrink()
                        .overflow_hidden()
                        .text_xs()
                        .child(SharedString::from(genesis.clone())),
                );
            }
            item
        });

        // The file list. A 202-file changeset scrolled as one stream is a
        // stream you get lost in: this says which file you are inside, and
        // jumps to any other. The sidebar was mostly empty space before, which
        // is a poor trade for 240 pixels.
        let surface = self.surface.read(cx);
        let current = surface.current_file();
        let files: Vec<_> = surface
            .file_entries()
            .into_iter()
            .enumerate()
            .map(|(index, entry)| {
                let handle = self.surface.clone();
                // The tail of a path is what identifies it in a narrow column;
                // the full path rides in the tooltip-free title attribute of
                // the row's own text instead of wrapping.
                let name = entry
                    .path
                    .rsplit_once('/')
                    .map(|(_, name)| name.to_string())
                    .unwrap_or_else(|| entry.path.to_string());
                SidebarMenuItem::new(SharedString::from(name))
                    .active(current == Some(index))
                    .suffix(
                        h_flex()
                            .gap_1()
                            .text_xs()
                            .child(
                                div()
                                    .text_color(cx.theme().success)
                                    .child(SharedString::from(format!("+{}", entry.added))),
                            )
                            .child(
                                div()
                                    .text_color(cx.theme().danger)
                                    .child(SharedString::from(format!("−{}", entry.removed))),
                            ),
                    )
                    .on_click(move |_, _window, cx| {
                        handle.update(cx, |surface, cx| surface.jump_to_file(index, cx));
                    })
            })
            .collect();
        let file_count = files.len();

        Sidebar::left()
            // The panel owns the width now; the sidebar's own fixed width and
            // right border would fight the resize handle for it.
            .w_full()
            .flex_shrink()
            .border_r_0()
            .collapsible(false)
            .header(h_flex().text_sm().child("Workspaces"))
            .child(
                SidebarGroup::new(SharedString::from(format!(
                    "{count} workspace{}",
                    if count == 1 { "" } else { "s" }
                )))
                .child(SidebarMenu::new().children(items)),
            )
            .child(
                SidebarGroup::new(SharedString::from(format!("{file_count} files")))
                    .child(SidebarMenu::new().children(files)),
            )
    }

    /// The footer: diff stats, and the receipt verdict.
    fn render_hud(&self, cx: &Context<Self>) -> impl IntoElement {
        let (files, added, removed, base) = &self.changeset_summary;

        let (verdict, kind) = match &self.status {
            None => ("no checks".to_string(), Verdict::Absent),
            Some(status) if status.green => ("green".to_string(), Verdict::Green),
            Some(status) => {
                let named = |wanted: &[CheckState]| -> Vec<&str> {
                    status
                        .rows
                        .iter()
                        .filter(|row| row.required && wanted.contains(&row.state))
                        .map(|row| row.check.as_str())
                        .collect()
                };
                let failed = named(&[CheckState::Fail, CheckState::StaleDefinition]);
                if failed.is_empty() {
                    // Everything outstanding is simply unrecorded.
                    let missing = named(&[CheckState::Missing]);
                    (format!("not run: {}", missing.join(", ")), Verdict::NotRun)
                } else {
                    (format!("failed: {}", failed.join(", ")), Verdict::Failed)
                }
            }
        };

        let chip = match kind {
            Verdict::Green => Tag::success(),
            Verdict::Failed => Tag::danger(),
            // Grey, and never red: `secondary` is the old `pending` colour.
            Verdict::NotRun => Tag::secondary(),
            // Nothing was claimed at all, so the chip barely claims a surface.
            Verdict::Absent => Tag::secondary().outline(),
        };

        let rule = || Divider::vertical().w(px(1.0)).h(px(12.0));

        h_flex()
            .h(px(26.0))
            .flex_none()
            .items_center()
            .gap_3()
            .px_3()
            .bg(cx.theme().title_bar)
            .border_t_1()
            .border_color(cx.theme().border)
            .text_size(px(11.0))
            .child(
                Label::new(format!("vs {base}"))
                    .text_size(px(11.0))
                    .text_color(cx.theme().muted_foreground),
            )
            .child(rule())
            .child(
                Label::new(format!("{files} files"))
                    .text_size(px(11.0))
                    .text_color(cx.theme().muted_foreground),
            )
            .child(rule())
            .child(Label::new(format!("+{added} −{removed}")).text_size(px(11.0)))
            .child(rule())
            .child(chip.small().child(SharedString::from(verdict)))
    }
}

impl Render for Cockpit {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(self.render_title_bar())
            .child(
                // `min_h_0` is what keeps the split from growing past the
                // window and pushing the HUD off the bottom edge.
                div().flex_1().min_h_0().overflow_hidden().child(
                    h_resizable("cockpit-split")
                        .child(
                            resizable_panel()
                                .size(px(240.0))
                                .size_range(px(180.0)..px(420.0))
                                .child(self.render_sidebar(cx)),
                        )
                        .child(resizable_panel().child(self.surface.clone())),
                ),
            )
            .child(self.render_hud(cx))
    }
}
