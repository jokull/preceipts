//! One workspace, rendered: the jump list, the diff surface, and the verdict.
//!
//! Everything in this file is a view onto a single workspace, which is the
//! central object of docs/direction-2026-08.md's domain model. The window
//! holds one of these per open tab; nothing here knows that tabs exist.
//!
//! It loads eagerly and fallibly. A worktree whose HEAD is unreadable used to
//! kill the process before the window opened — with tabs that is one bad
//! worktree taking the whole cockpit down, so the load returns an error the
//! tab can render instead.

use crate::lab_panel::LabPanel;
use crate::surface_view::SurfaceView;
use gpui::prelude::*;
use gpui::{div, px, App, Context, Entity, SharedString, Window};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::divider::Divider;
use gpui_component::label::Label;
use gpui_component::resizable::{h_resizable, resizable_panel};
use gpui_component::tag::Tag;
use gpui_component::{h_flex, v_flex, ActiveTheme as _, Sizable as _};
use preceipts_core::checks::{CheckState, Status};
use preceipts_core::workspace::Workspace;
use preceipts_core::{load, Changeset, DiffScope};
use std::collections::HashMap;

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

pub struct WorkspacePane {
    lab: Entity<LabPanel>,
    /// `None` means "follow the environment": the panel appears when the
    /// workspace has services running and stays away when it does not, because
    /// a panel that only ever says "no environment up" is a panel you resent.
    /// Toggling it once takes the decision away from the environment for good
    /// — an explicit choice must not be undone by a dev server restarting.
    lab_open: Option<bool>,
    changeset_summary: (usize, usize, usize, String),
    status: Option<Status>,
    surface: Entity<SurfaceView>,
}

impl WorkspacePane {
    /// Load a workspace's diff and open a pane on it.
    ///
    /// Takes `&mut App` rather than a `Context`: the fallible part happens
    /// before there is an entity to fail inside of.
    pub fn open(workspace: Workspace, cx: &mut App) -> Result<Entity<Self>, String> {
        let changeset =
            load(&workspace.path, DiffScope::Branch, true).map_err(|error| format!("{error}"))?;
        // Checks degrade rather than fail: a project with no check config is
        // a project whose HUD says "no checks", not a tab that will not open.
        let status = preceipts_core::checks::status(&workspace.path, None).ok();
        Ok(cx.new(|cx| Self::new(workspace, changeset, status, cx)))
    }

    fn new(
        workspace: Workspace,
        changeset: Changeset,
        status: Option<Status>,
        cx: &mut Context<Self>,
    ) -> Self {
        let summary = (
            changeset.files.len(),
            changeset.total_added(),
            changeset.total_removed(),
            changeset.base_name.clone(),
        );
        let lab = cx.new(|cx| LabPanel::new(workspace, cx));
        cx.observe(&lab, |_, _, cx| cx.notify()).detach();
        let code_font = crate::theme::code_font(cx);
        let surface = cx.new(|cx| SurfaceView::new(changeset, code_font, cx));
        // The file list marks the file you are currently inside, so this view
        // has to repaint when the surface scrolls. Child entities do not
        // notify their parent on their own.
        cx.observe(&surface, |_, _, cx| cx.notify()).detach();
        Self {
            lab,
            lab_open: None,
            changeset_summary: summary,
            status,
            surface,
        }
    }

    /// What the tab and the window title say about this workspace.
    pub fn summary(&self) -> String {
        let (files, added, removed, _) = &self.changeset_summary;
        format!("{files} files · +{added} −{removed}")
    }

    /// The jump list: one row per changed file.
    ///
    /// Hand-built rows rather than `SidebarMenuItem`s. That component is ~36px
    /// tall and shows the label as a plain string, which for 202 files means
    /// fifteen of them on screen and no way to colour the status letter.
    /// Density is the whole point of a jump list.
    fn render_file_list(&self, cx: &Context<Self>) -> impl IntoElement {
        let surface = self.surface.read(cx);
        let current = surface.current_file();
        let entries = surface.file_entries();

        // A basename alone is ambiguous — this changeset has two `version.env`
        // and several `SKILL.md`. Only the ambiguous ones pay for the parent
        // directory; the rest stay short.
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for entry in &entries {
            *seen.entry(basename(&entry.path)).or_default() += 1;
        }

        let count = entries.len();
        let files: Vec<_> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                // The filename must never be the part that gets clipped, so
                // it is its own span that cannot shrink; the parent directory
                // sits beside it, dimmed, and absorbs the truncation. The
                // parent only appears when the basename is ambiguous.
                let name = basename(&entry.path);
                let parent = if seen.get(name).copied().unwrap_or(0) > 1 {
                    entry
                        .path
                        .rsplit_once('/')
                        .map(|(parent, _)| elide(basename(parent)))
                } else {
                    None
                };
                let handle = self.surface.clone();
                let active = current == Some(index);
                let status_color = match entry.status.as_ref() {
                    "A" => cx.theme().success,
                    "D" => cx.theme().danger,
                    _ => cx.theme().muted_foreground,
                };
                h_flex()
                    .id(index)
                    .h(px(22.0))
                    .w_full()
                    .px_2()
                    .gap_2()
                    .rounded_sm()
                    .text_xs()
                    .when(active, |this| this.bg(cx.theme().sidebar_accent))
                    .hover(|this| this.bg(cx.theme().sidebar_accent))
                    .child(
                        div()
                            .w(px(10.0))
                            .flex_none()
                            .text_color(status_color)
                            .child(entry.status.clone()),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            // Without this a long name wraps to a second line
                            // inside a 22px row and prints over its neighbour.
                            .whitespace_nowrap()
                            .when_some(parent, |this, parent| {
                                this.child(
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(SharedString::from(parent)),
                                )
                            })
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(cx.theme().sidebar_foreground)
                                    .child(SharedString::from(name.to_string())),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(cx.theme().success)
                            .child(SharedString::from(format!("+{}", entry.added))),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(cx.theme().danger)
                            .child(SharedString::from(format!("−{}", entry.removed))),
                    )
                    .on_click(move |_, _window, cx| {
                        handle.update(cx, |surface, cx| surface.jump_to_file(index, cx));
                    })
            })
            .collect();

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().sidebar)
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(format!("{count} files"))),
            )
            .child(
                v_flex()
                    .id("file-list")
                    .flex_1()
                    .min_h_0()
                    .px_1()
                    .overflow_y_scroll()
                    .children(files),
            )
    }

    fn lab_showing(&self, cx: &App) -> bool {
        self.lab_open
            .unwrap_or_else(|| self.lab.read(cx).has_services())
    }

    /// The footer: diff stats, and the receipt verdict.
    fn render_hud(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            // Pushes the lab toggle to the right edge, where a panel switch
            // belongs and where it is not in the way of the verdict.
            .child(div().flex_1())
            .child(
                Button::new("lab")
                    .ghost()
                    .xsmall()
                    .label("Lab")
                    .when(self.lab_showing(cx), |button| {
                        button.text_color(cx.theme().foreground)
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.lab_open = Some(!this.lab_showing(cx));
                        cx.notify();
                    })),
            )
    }
}

impl Render for WorkspacePane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(
                // `min_h_0` is what keeps the split from growing past the
                // window and pushing the HUD off the bottom edge.
                div().flex_1().min_h_0().overflow_hidden().child(
                    h_resizable("workspace-split")
                        .child(
                            resizable_panel()
                                .size(px(240.0))
                                .size_range(px(180.0)..px(420.0))
                                .child(self.render_file_list(cx)),
                        )
                        .child(resizable_panel().child(self.surface.clone()))
                        .when(self.lab_showing(cx), |split| {
                            split.child(
                                resizable_panel()
                                    .size(px(320.0))
                                    .size_range(px(240.0)..px(560.0))
                                    .child(self.lab.clone()),
                            )
                        }),
                ),
            )
            .child(self.render_hud(cx))
    }
}

/// Shorten a directory name from the left, keeping the end.
///
/// Clipping it with `overflow_hidden` alone leaves the cut edge flush against
/// the filename — `macos-spm-appLICENSE` — which reads as one word. An
/// explicit ellipsis says the name was shortened.
fn elide(name: &str) -> String {
    const MAX: usize = 14;
    if name.chars().count() <= MAX {
        return format!("{name}/");
    }
    let tail: String = name.chars().skip(name.chars().count() - MAX + 1).collect();
    format!("…{tail}/")
}

/// The last component of a path — what identifies a file in a narrow column,
/// most of the time.
fn basename(path: &str) -> &str {
    path.rsplit_once('/').map(|(_, name)| name).unwrap_or(path)
}
